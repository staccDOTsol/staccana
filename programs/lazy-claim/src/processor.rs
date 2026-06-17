//! Claim instruction processor.
//!
//! Verification order — exactly as SPEC §4.3 prescribes:
//!
//! 1. Recompute the leaf hash from `(pubkey, lamports)`.
//! 2. Walk the Merkle proof; reject unless the recomputed root equals the embedded
//!    `claimable_root` from the config account.
//! 3. Inspect the prior instruction in the transaction via the `Instructions` sysvar; it
//!    must be an ed25519 precompile signing the SPEC §4.2 message with `pubkey`.
//! 4. The recipient account passed at index 0 must equal `pubkey`.
//! 5. The claimed-marker PDA must not yet exist.
//! 6. Credit `lamports` to the recipient account. **This step requires validator-side
//!    wiring** — see the inline comment at `credit_lamports` for what genesis must install.
//! 7. Initialize the claimed-marker PDA (sized minimally, owned by this program).

use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::entrypoint::ProgramResult;
use solana_program::hash::Hash;
use solana_program::msg;
use solana_program::program::invoke_signed;
use solana_program::program_error::ProgramError;
use solana_program::pubkey::Pubkey;
use solana_program::rent::Rent;
use solana_program::system_instruction;
use solana_program::sysvar::instructions::{
    self as instructions_sysvar, load_current_index_checked, load_instruction_at_checked,
};
use solana_program::sysvar::Sysvar;

use crate::error::LazyClaimError;
use crate::instruction::{
    ClaimArgs, ClaimFromBufferArgs, InitProofBufferArgs, LazyClaimInstruction, WriteProofBufferArgs,
};
use crate::merkle::{leaf_hash, verify_inclusion};
use crate::state::{
    find_claimed_marker_pda, find_proof_buffer_pda, ClaimedMarker, LazyClaimConfig,
    ProofBufferHeader, CLAIMED_MARKER_SEED, PROOF_BUFFER_SEED,
};

/// Hardcoded admin authority — staccana's BPF upgrade-authority key, shared with
/// `staccana_megadrop::ADMIN_AUTHORITY`. Gates the privileged `DrainTreasury` and
/// `AssignTreasuryOwner` ixs used to hand treasury custody to the Squads governance
/// multisig post-launch (the validator-subsidy program that previously consumed the
/// treasury was removed — no bridge, no yield engine).
pub const ADMIN_AUTHORITY: Pubkey =
    solana_program::pubkey!("HSwe2Y7i6CPuJGb27rBwUumt8HZ8sCpQvG4PBBiC5f4y");

/// Static prefix from SPEC §4.2 — exactly 17 bytes.
pub const CLAIM_MESSAGE_PREFIX: &[u8] = b"STACCANA_CLAIM_V1";

/// Length of the message a claimant must sign: prefix + pubkey + lamports + program_id.
pub const CLAIM_MESSAGE_LEN: usize = 17 + 32 + 8 + 32;

/// Top-level entrypoint dispatched by `lib.rs`.
pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    if data.is_empty() {
        return Err(LazyClaimError::BadInstructionData.into());
    }
    match LazyClaimInstruction::from_byte(data[0])? {
        LazyClaimInstruction::Claim => {
            let args = ClaimArgs::decode_body(&data[1..])?;
            process_claim(program_id, accounts, &args)
        }
        LazyClaimInstruction::InitProofBuffer => {
            let args = InitProofBufferArgs::decode_body(&data[1..])?;
            process_init_proof_buffer(program_id, accounts, &args)
        }
        LazyClaimInstruction::WriteProofBuffer => {
            let args = WriteProofBufferArgs::decode_body(&data[1..])?;
            process_write_proof_buffer(program_id, accounts, &args)
        }
        LazyClaimInstruction::ClaimFromBuffer => {
            let args = ClaimFromBufferArgs::decode_body(&data[1..])?;
            process_claim_from_buffer(program_id, accounts, &args)
        }
        LazyClaimInstruction::DrainTreasury => {
            if data.len() < 9 {
                return Err(LazyClaimError::BadInstructionData.into());
            }
            let mut amt_bytes = [0u8; 8];
            amt_bytes.copy_from_slice(&data[1..9]);
            let amount = u64::from_le_bytes(amt_bytes);
            process_drain_treasury(program_id, accounts, amount)
        }
        LazyClaimInstruction::AssignTreasuryOwner => {
            if data.len() < 33 {
                return Err(LazyClaimError::BadInstructionData.into());
            }
            let mut new_owner_bytes = [0u8; 32];
            new_owner_bytes.copy_from_slice(&data[1..33]);
            let new_owner = Pubkey::new_from_array(new_owner_bytes);
            process_assign_treasury_owner(program_id, accounts, &new_owner)
        }
    }
}

/// Privileged: re-assign treasury's `owner` field to `new_owner`.
///
/// Solana's account-modification invariants permit an owner-program to
/// change the `owner` of its own accounts when `data.len() == 0`. The
/// treasury PDA is zero-data (carries lamports only — see
/// `genesis-bake/src/accounts.rs::treasury_account`), so this is allowed.
///
/// Why this exists: genesis-bake sets `treasury.owner = lazy-claim` so the
/// gas-exempt claim path can direct-debit it during the claim window. Once
/// claims wind down, the admin hands custody to the post-launch treasury
/// custodian (the Squads governance multisig, or a future drawdown-distributor
/// program) — either by draining to it (`DrainTreasury`) or, for a program
/// custodian, reassigning ownership with this ix.
///
/// Idempotence: if treasury is already non-lazy-claim-owned, the
/// `treasury_ai.owner != program_id` check returns `IllegalOwner` and the
/// ix no-ops cleanly.
///
/// Accounts:
///   0. authority   [signer]     must equal `ADMIN_AUTHORITY`
///   1. treasury    [writable]   PDA owned by THIS program (= lazy-claim)
pub fn process_assign_treasury_owner(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    new_owner: &Pubkey,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let authority_ai = next_account_info(iter)?;
    let treasury_ai = next_account_info(iter)?;

    if !authority_ai.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if authority_ai.key != &ADMIN_AUTHORITY {
        return Err(ProgramError::IncorrectAuthority);
    }
    if treasury_ai.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    if !treasury_ai.data_is_empty() {
        // Defense-in-depth: the runtime would also reject the assign if
        // data is non-empty, but surface a clearer error here.
        return Err(LazyClaimError::BadInstructionData.into());
    }

    treasury_ai.assign(new_owner);

    msg!(
        "[assign_treasury_owner] {} owner -> {}",
        treasury_ai.key,
        new_owner
    );
    Ok(())
}

/// Privileged treasury-drain handler — the `ADMIN_AUTHORITY`-gated path that
/// moves lamports out of the genesis treasury.
///
/// Background: `tools/genesis-bake/src/accounts.rs::treasury_account` sets the
/// treasury PDA's `owner` to this program (lazy-claim) so the gas-exempt claim
/// path can `try_borrow_mut_lamports` on it during the claim window. The runtime
/// forbids debiting an account you don't own, so the treasury MUST stay
/// lazy-claim-owned while claims are live.
///
/// Post-launch, this ix is how the validator-subsidy **drawdown** is funded:
/// the admin drains treasury principal to the **Squads governance multisig
/// vault** (`recipient`), which then hand-distributes to validators (no yield,
/// no staking — see `docs/AUDIT_SCOPE.md`). Drain incrementally as drawdown is
/// needed, or move the whole residual once the claim window closes.
///
/// Idempotence: the `treasury_ai.owner != program_id` check returns
/// `IllegalOwner` if the treasury has already been reassigned away from
/// lazy-claim, so this can't double-spend after a custody handoff.
///
/// Accounts:
///   0. authority   [signer]     must equal `ADMIN_AUTHORITY`
///   1. treasury    [writable]   PDA owned by THIS program (= lazy-claim)
///   2. recipient   [writable]   destination for the lamports
pub fn process_drain_treasury(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    amount: u64,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let authority_ai = next_account_info(iter)?;
    let treasury_ai = next_account_info(iter)?;
    let recipient_ai = next_account_info(iter)?;

    if !authority_ai.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if authority_ai.key != &ADMIN_AUTHORITY {
        return Err(ProgramError::IncorrectAuthority);
    }
    if treasury_ai.owner != program_id {
        return Err(ProgramError::IllegalOwner);
    }
    let cur = treasury_ai.lamports();
    if amount > cur {
        return Err(ProgramError::InsufficientFunds);
    }

    **treasury_ai.try_borrow_mut_lamports()? = cur
        .checked_sub(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    let new_recipient = recipient_ai
        .lamports()
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    **recipient_ai.try_borrow_mut_lamports()? = new_recipient;

    msg!(
        "[drain_treasury] {} lamports: {} -> {}",
        amount,
        treasury_ai.key,
        recipient_ai.key,
    );
    Ok(())
}

/// Process one `Claim` instruction. See module doc comment for the verification order.
pub fn process_claim(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    args: &ClaimArgs,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let recipient_ai = next_account_info(iter)?;
    let config_ai = next_account_info(iter)?;
    let instructions_ai = next_account_info(iter)?;
    let treasury_ai = next_account_info(iter)?;
    let marker_ai = next_account_info(iter)?;
    let payer_ai = next_account_info(iter)?;
    let system_program_ai = next_account_info(iter)?;

    let proof_hashes: Vec<Hash> = args
        .proof
        .iter()
        .map(|b| Hash::new_from_array(*b))
        .collect();

    finalize_claim(
        program_id,
        recipient_ai,
        config_ai,
        instructions_ai,
        treasury_ai,
        marker_ai,
        payer_ai,
        system_program_ai,
        &args.pubkey,
        args.lamports,
        &proof_hashes,
        &args.proof_flags,
    )
}

/// Allocate (and zero-init the header of) the proof-buffer PDA at
/// `["proof_buffer", pubkey, payer]`.
///
/// Accounts:
/// 0. `proof_buffer_ai` — the PDA to create [writable]
/// 1. `payer_ai`        — pays rent + signs the create CPI [signer, writable]
/// 2. `system_program_ai`
pub fn process_init_proof_buffer(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    args: &InitProofBufferArgs,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let proof_buffer_ai = next_account_info(iter)?;
    let payer_ai = next_account_info(iter)?;
    let system_program_ai = next_account_info(iter)?;

    let pubkey_bytes = args.pubkey;
    let pubkey = Pubkey::new_from_array(pubkey_bytes);
    let (expected_pda, bump) = find_proof_buffer_pda(&pubkey, payer_ai.key, program_id);
    if proof_buffer_ai.key != &expected_pda {
        return Err(LazyClaimError::BadProofBufferPda.into());
    }
    if !payer_ai.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let space = ProofBufferHeader::HEADER_SIZE
        .checked_add(args.total_len as usize)
        .ok_or(ProgramError::from(LazyClaimError::ProofBufferOverflow))?;
    let rent = Rent::get()?;
    let lamports_for_rent = rent.minimum_balance(space);

    let create_ix = system_instruction::create_account(
        payer_ai.key,
        proof_buffer_ai.key,
        lamports_for_rent,
        space as u64,
        program_id,
    );
    invoke_signed(
        &create_ix,
        &[
            payer_ai.clone(),
            proof_buffer_ai.clone(),
            system_program_ai.clone(),
        ],
        &[&[
            PROOF_BUFFER_SEED,
            pubkey_bytes.as_ref(),
            payer_ai.key.as_ref(),
            &[bump],
        ]],
    )?;

    let header = ProofBufferHeader {
        total_len: args.total_len,
        bytes_written: 0,
    };
    let mut data = proof_buffer_ai.try_borrow_mut_data()?;
    header.pack_header(&mut data)?;
    Ok(())
}

/// Append `bytes` into the proof buffer at `offset`. Idempotent on offset — re-writing
/// the same span is fine. Updates the `bytes_written` high-water mark.
///
/// Accounts:
/// 0. `proof_buffer_ai` [writable]
pub fn process_write_proof_buffer(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    args: &WriteProofBufferArgs,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let proof_buffer_ai = next_account_info(iter)?;

    if proof_buffer_ai.owner != program_id {
        return Err(LazyClaimError::BadProofBuffer.into());
    }
    let mut data = proof_buffer_ai.try_borrow_mut_data()?;
    let mut header = ProofBufferHeader::unpack_header(&data)?;

    let off = args.offset as usize;
    let end = off
        .checked_add(args.bytes.len())
        .ok_or(ProgramError::from(LazyClaimError::ProofBufferOverflow))?;
    if end > header.total_len as usize {
        return Err(LazyClaimError::ProofBufferOverflow.into());
    }
    let abs_end = ProofBufferHeader::HEADER_SIZE
        .checked_add(end)
        .ok_or(ProgramError::from(LazyClaimError::ProofBufferOverflow))?;
    if abs_end > data.len() {
        return Err(LazyClaimError::ProofBufferOverflow.into());
    }
    let abs_off = ProofBufferHeader::HEADER_SIZE + off;
    data[abs_off..abs_end].copy_from_slice(&args.bytes);

    let new_written = core::cmp::max(header.bytes_written as usize, end) as u32;
    header.bytes_written = new_written;
    header.pack_header(&mut data)?;
    Ok(())
}

/// Final claim using a proof buffer instead of inline proof bytes. Same accounts as
/// `Claim` plus the proof-buffer PDA appended at the end. After successful claim, the
/// buffer is closed (lamports → payer, data zeroed, owner re-assigned to system).
pub fn process_claim_from_buffer(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    args: &ClaimFromBufferArgs,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let recipient_ai = next_account_info(iter)?;
    let config_ai = next_account_info(iter)?;
    let instructions_ai = next_account_info(iter)?;
    let treasury_ai = next_account_info(iter)?;
    let marker_ai = next_account_info(iter)?;
    let payer_ai = next_account_info(iter)?;
    let system_program_ai = next_account_info(iter)?;
    let proof_buffer_ai = next_account_info(iter)?;

    // Validate buffer ownership + PDA derivation against (pubkey, payer).
    if proof_buffer_ai.owner != program_id {
        return Err(LazyClaimError::BadProofBuffer.into());
    }
    let pubkey = Pubkey::new_from_array(args.pubkey);
    let (expected_buffer, _bump) = find_proof_buffer_pda(&pubkey, payer_ai.key, program_id);
    if proof_buffer_ai.key != &expected_buffer {
        return Err(LazyClaimError::BadProofBufferPda.into());
    }

    // Read the staged proof. Validate the buffer was fully populated for this proof_len.
    let proof_byte_len = (args.proof_len as usize)
        .checked_mul(32)
        .ok_or(ProgramError::from(LazyClaimError::ProofBufferOverflow))?;
    let proof_hashes = {
        let buf = proof_buffer_ai.try_borrow_data()?;
        let header = ProofBufferHeader::unpack_header(&buf)?;
        if (header.total_len as usize) < proof_byte_len {
            return Err(LazyClaimError::ProofBufferLengthMismatch.into());
        }
        if (header.bytes_written as usize) < proof_byte_len {
            return Err(LazyClaimError::ProofBufferIncomplete.into());
        }
        let payload_start = ProofBufferHeader::HEADER_SIZE;
        let payload_end = payload_start + proof_byte_len;
        if payload_end > buf.len() {
            return Err(LazyClaimError::ProofBufferOverflow.into());
        }
        let mut proof_hashes: Vec<Hash> = Vec::with_capacity(args.proof_len as usize);
        for i in 0..(args.proof_len as usize) {
            let off = payload_start + i * 32;
            let mut sibling = [0u8; 32];
            sibling.copy_from_slice(&buf[off..off + 32]);
            proof_hashes.push(Hash::new_from_array(sibling));
        }
        proof_hashes
    };

    finalize_claim(
        program_id,
        recipient_ai,
        config_ai,
        instructions_ai,
        treasury_ai,
        marker_ai,
        payer_ai,
        system_program_ai,
        &args.pubkey,
        args.lamports,
        &proof_hashes,
        &args.proof_flags,
    )?;

    // Close the proof buffer — return rent to payer, zero data, hand back to system.
    close_proof_buffer(proof_buffer_ai, payer_ai)?;
    Ok(())
}

/// Internal: do the actual claim verification + state mutation. Shared by `Claim` and
/// `ClaimFromBuffer`. The proof is already in `Hash` form so the caller can source it
/// from inline ix data or a staged buffer.
#[allow(clippy::too_many_arguments)]
fn finalize_claim<'a>(
    program_id: &Pubkey,
    recipient_ai: &AccountInfo<'a>,
    config_ai: &AccountInfo<'a>,
    instructions_ai: &AccountInfo<'a>,
    treasury_ai: &AccountInfo<'a>,
    marker_ai: &AccountInfo<'a>,
    payer_ai: &AccountInfo<'a>,
    system_program_ai: &AccountInfo<'a>,
    pubkey: &[u8; 32],
    lamports: u64,
    proof_hashes: &[Hash],
    proof_flags: &[u8],
) -> ProgramResult {
    // Step 0: validate the config account is owned by us and unpack the embedded root.
    if config_ai.owner != program_id {
        return Err(LazyClaimError::BadConfigAccount.into());
    }
    let config = {
        let data = config_ai.try_borrow_data()?;
        LazyClaimConfig::unpack(&data)?
    };

    // Step 1 & 2: Merkle inclusion against the embedded root.
    let leaf = leaf_hash(pubkey, lamports);
    if !verify_inclusion(leaf, proof_hashes, proof_flags, &config.claimable_root) {
        return Err(LazyClaimError::BadMerkleProof.into());
    }

    // Step 3: ed25519 precompile must immediately precede this ix.
    if instructions_ai.key != &instructions_sysvar::id() {
        return Err(LazyClaimError::BadInstructionsSysvar.into());
    }
    verify_prior_ed25519_signature(instructions_ai, program_id, pubkey, lamports)?;

    // Step 4: recipient pubkey check.
    if recipient_ai.key.to_bytes() != *pubkey {
        return Err(LazyClaimError::RecipientMismatch.into());
    }

    // Step 5: treasury PDA must match config; claimed-marker PDA must not yet exist.
    if treasury_ai.key != &config.treasury_pda {
        return Err(LazyClaimError::BadTreasuryAccount.into());
    }
    let (expected_marker, _bump) = find_claimed_marker_pda(recipient_ai.key, program_id);
    if marker_ai.key != &expected_marker {
        return Err(LazyClaimError::BadClaimedMarkerPda.into());
    }
    if marker_ai.lamports() != 0 || !marker_ai.data_is_empty() {
        return Err(LazyClaimError::AlreadyClaimed.into());
    }

    // Step 6: credit lamports.
    credit_lamports(treasury_ai, recipient_ai, lamports)?;

    // Step 7: initialize the claimed-marker PDA.
    init_claimed_marker(
        marker_ai,
        payer_ai,
        system_program_ai,
        program_id,
        recipient_ai.key,
        lamports,
    )?;

    msg!("staccana lazy-claim: materialized {}", recipient_ai.key);
    Ok(())
}

/// Close the proof-buffer PDA: return rent to payer, zero out data. Owner stays this
/// program but the account becomes lamport-zero so the runtime garbage-collects it
/// once the tx commits.
fn close_proof_buffer(buffer_ai: &AccountInfo, payer_ai: &AccountInfo) -> Result<(), ProgramError> {
    let mut buffer_lamports = buffer_ai.try_borrow_mut_lamports()?;
    let mut payer_lamports = payer_ai.try_borrow_mut_lamports()?;
    let amount = **buffer_lamports;
    **buffer_lamports = 0;
    **payer_lamports = payer_lamports
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;

    let mut data = buffer_ai.try_borrow_mut_data()?;
    for b in data.iter_mut() {
        *b = 0;
    }
    Ok(())
}

/// Inspect the immediately preceding instruction via the `Instructions` sysvar; verify it
/// is an ed25519 precompile signing the SPEC §4.2 message with `expected_pubkey`.
///
/// This relies on the ed25519 precompile having already verified the signature at the
/// runtime layer — we only re-check that the precompile's claimed (pubkey, message) match
/// what we expect for this claim. If the precompile would have failed verification, the
/// transaction would have been rejected before reaching this program.
fn verify_prior_ed25519_signature(
    instructions_ai: &AccountInfo,
    program_id: &Pubkey,
    expected_pubkey: &[u8; 32],
    expected_lamports: u64,
) -> Result<(), ProgramError> {
    let current_index = load_current_index_checked(instructions_ai)? as usize;
    if current_index == 0 {
        return Err(LazyClaimError::MissingEd25519Precompile.into());
    }
    let prev = load_instruction_at_checked(current_index - 1, instructions_ai)?;
    if prev.program_id != solana_program::ed25519_program::id() {
        return Err(LazyClaimError::MissingEd25519Precompile.into());
    }
    let (signed_pubkey, signed_msg) = parse_ed25519_precompile_data(&prev.data)?;

    if signed_pubkey != *expected_pubkey {
        return Err(LazyClaimError::SignerPubkeyMismatch.into());
    }

    let expected_msg = build_claim_message(expected_pubkey, expected_lamports, program_id);
    if signed_msg != expected_msg {
        return Err(LazyClaimError::SignedMessageMismatch.into());
    }
    Ok(())
}

/// Build the canonical claim message per SPEC §4.2.
pub fn build_claim_message(pubkey: &[u8; 32], lamports: u64, program_id: &Pubkey) -> Vec<u8> {
    let mut msg = Vec::with_capacity(CLAIM_MESSAGE_LEN);
    msg.extend_from_slice(CLAIM_MESSAGE_PREFIX);
    msg.extend_from_slice(pubkey);
    msg.extend_from_slice(&lamports.to_le_bytes());
    msg.extend_from_slice(program_id.as_ref());
    msg
}

/// Parse the ed25519 precompile instruction data layout. Returns `(pubkey, message)` for
/// the first signature entry — single-signature precompile use is what we expect here.
///
/// Layout (from the runtime ed25519 precompile, single signature variant):
/// * `[0]`     count = 1
/// * `[1]`     padding
/// * `[2..4]`  signature_offset (u16 LE)
/// * `[4..6]`  signature_instruction_index (u16 LE) — `0xFFFF` means "this ix"
/// * `[6..8]`  public_key_offset
/// * `[8..10]` public_key_instruction_index
/// * `[10..12]` message_data_offset
/// * `[12..14]` message_data_size (u16 LE)
/// * `[14..16]` message_instruction_index
/// * remainder: signature, pubkey, message at the offsets above
fn parse_ed25519_precompile_data(data: &[u8]) -> Result<([u8; 32], Vec<u8>), ProgramError> {
    if data.len() < 16 {
        return Err(LazyClaimError::BadEd25519Precompile.into());
    }
    let count = data[0];
    if count != 1 {
        // Multi-signature precompiles are valid in general but not what we expect here;
        // simpler to require the conventional single-sig layout.
        return Err(LazyClaimError::BadEd25519Precompile.into());
    }

    let pk_offset = u16::from_le_bytes([data[6], data[7]]) as usize;
    let pk_ix_idx = u16::from_le_bytes([data[8], data[9]]);
    let msg_offset = u16::from_le_bytes([data[10], data[11]]) as usize;
    let msg_size = u16::from_le_bytes([data[12], data[13]]) as usize;
    let msg_ix_idx = u16::from_le_bytes([data[14], data[15]]);

    // We only inspect inline data (offsets pointing back into this same instruction's data).
    // The precompile sentinel `0xFFFF` denotes "current instruction"; anything else means
    // the precompile referenced data from another instruction in the transaction, which we
    // can't reach from here.
    const CURRENT_IX: u16 = u16::MAX;
    if pk_ix_idx != CURRENT_IX || msg_ix_idx != CURRENT_IX {
        return Err(LazyClaimError::BadEd25519Precompile.into());
    }

    if pk_offset.saturating_add(32) > data.len()
        || msg_offset.saturating_add(msg_size) > data.len()
    {
        return Err(LazyClaimError::BadEd25519Precompile.into());
    }

    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(&data[pk_offset..pk_offset + 32]);
    let message = data[msg_offset..msg_offset + msg_size].to_vec();
    Ok((pubkey, message))
}

/// Credit `amount` lamports to `recipient`.
///
/// **Genesis wiring required.** Lazy-claim manufactures lamports out of thin air at run
/// time — those lamports were never in any pre-existing account. The validator must, at
/// genesis, install a privileged-mint capability scoped exclusively to
/// [`crate::id()`](crate::id). Two acceptable mechanisms:
///
/// * **Treasury-debit path (preferred):** lazy-claim debits a pre-credited treasury PDA
///   that holds `sum(claimable.lamports)` from genesis. SOL conservation is preserved
///   (invariant I1). The treasury PDA is passed in at account index 3; we debit it here.
/// * **Privileged mint:** validator recognizes a special syscall from this program and
///   mints fresh lamports. Less surgical, but works for chains that don't want a stash PDA.
///
/// This implementation takes the treasury-debit path. The treasury PDA must be writable
/// and we must be able to mutate its lamports — which means it must be owned by this
/// program OR the validator must install a special-cased rule that lets us debit it. The
/// genesis builder MUST set the owner accordingly. Without that wiring, the lamport
/// transfer below will fail at runtime.
fn credit_lamports(
    treasury_ai: &AccountInfo,
    recipient_ai: &AccountInfo,
    amount: u64,
) -> Result<(), ProgramError> {
    let mut treasury_lamports = treasury_ai.try_borrow_mut_lamports()?;
    let mut recipient_lamports = recipient_ai.try_borrow_mut_lamports()?;

    let new_treasury = treasury_lamports
        .checked_sub(amount)
        .ok_or(ProgramError::InsufficientFunds)?;
    let new_recipient = recipient_lamports
        .checked_add(amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;

    **treasury_lamports = new_treasury;
    **recipient_lamports = new_recipient;
    Ok(())
}

/// Initialize the claimed-marker PDA via a `system_program::create_account` CPI signed
/// with the marker PDA's seeds, then stamp the marker contents into the data buffer.
///
/// Pre-state expectations on `marker_ai`: zero lamports, empty data, owned by the system
/// program (the standard "uninitialized PDA" shape). The CPI allocates the right size,
/// transfers rent-exempt lamports from `payer_ai`, and assigns the account to this
/// program. After the CPI returns we pack the [`ClaimedMarker`] payload into the new
/// data buffer.
fn init_claimed_marker<'a>(
    marker_ai: &AccountInfo<'a>,
    payer_ai: &AccountInfo<'a>,
    system_program_ai: &AccountInfo<'a>,
    program_id: &Pubkey,
    pubkey: &Pubkey,
    lamports: u64,
) -> Result<(), ProgramError> {
    let (expected_marker, bump) = find_claimed_marker_pda(pubkey, program_id);
    if marker_ai.key != &expected_marker {
        return Err(LazyClaimError::BadClaimedMarkerPda.into());
    }

    let rent = Rent::get()?;
    let space = ClaimedMarker::SIZE as u64;
    let lamports_for_rent = rent.minimum_balance(space as usize);

    let create_ix = system_instruction::create_account(
        payer_ai.key,
        marker_ai.key,
        lamports_for_rent,
        space,
        program_id,
    );
    invoke_signed(
        &create_ix,
        &[payer_ai.clone(), marker_ai.clone(), system_program_ai.clone()],
        &[&[CLAIMED_MARKER_SEED, pubkey.as_ref(), &[bump]]],
    )?;

    let mut data = marker_ai.try_borrow_mut_data()?;
    let marker = ClaimedMarker {
        pubkey: *pubkey,
        lamports,
    };
    marker.pack(&mut data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::pubkey::Pubkey;

    #[test]
    fn build_claim_message_layout_matches_spec() {
        let pubkey = [0xAB; 32];
        let lamports: u64 = 0x0102030405060708;
        let program_id = Pubkey::new_from_array([0xCD; 32]);
        let msg = build_claim_message(&pubkey, lamports, &program_id);
        assert_eq!(msg.len(), CLAIM_MESSAGE_LEN);
        assert_eq!(&msg[0..17], CLAIM_MESSAGE_PREFIX);
        assert_eq!(&msg[17..49], &pubkey);
        assert_eq!(&msg[49..57], &lamports.to_le_bytes());
        assert_eq!(&msg[57..89], program_id.as_ref());
    }

    #[test]
    fn parse_ed25519_data_extracts_pubkey_and_message() {
        // Hand-craft a minimal single-sig precompile data buffer.
        let pubkey = [0x42u8; 32];
        let message: Vec<u8> = (0..40u8).collect();
        let mut data = vec![0u8; 16];
        data[0] = 1; // count
        data[1] = 0; // padding

        let sig_offset: u16 = 16;
        let pk_offset: u16 = 16 + 64;
        let msg_offset: u16 = 16 + 64 + 32;
        let msg_size: u16 = message.len() as u16;
        let here: u16 = u16::MAX;

        data[2..4].copy_from_slice(&sig_offset.to_le_bytes());
        data[4..6].copy_from_slice(&here.to_le_bytes());
        data[6..8].copy_from_slice(&pk_offset.to_le_bytes());
        data[8..10].copy_from_slice(&here.to_le_bytes());
        data[10..12].copy_from_slice(&msg_offset.to_le_bytes());
        data[12..14].copy_from_slice(&msg_size.to_le_bytes());
        data[14..16].copy_from_slice(&here.to_le_bytes());

        data.extend_from_slice(&[0u8; 64]); // signature placeholder
        data.extend_from_slice(&pubkey);
        data.extend_from_slice(&message);

        let (got_pk, got_msg) = parse_ed25519_precompile_data(&data).unwrap();
        assert_eq!(got_pk, pubkey);
        assert_eq!(got_msg, message);
    }

    #[test]
    fn parse_ed25519_data_rejects_multi_sig_count() {
        let mut data = vec![0u8; 16];
        data[0] = 2; // multi-sig
        assert!(parse_ed25519_precompile_data(&data).is_err());
    }

    #[test]
    fn parse_ed25519_data_rejects_non_inline_indices() {
        let mut data = vec![0u8; 16];
        data[0] = 1;
        // Point pk at instruction 0 (not "current"). Should reject.
        data[8..10].copy_from_slice(&0u16.to_le_bytes());
        data[14..16].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(parse_ed25519_precompile_data(&data).is_err());
    }
}

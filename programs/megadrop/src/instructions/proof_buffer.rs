//! Proof-buffer staging instructions for megadrop.
//!
//! For deep Merkle proofs (~27 levels = 864 bytes of siblings), the inline-proof
//! `claim_megadrop` ix data overflows the 1232-byte transaction ceiling. This module
//! adds a 2-tx flow:
//!
//! 1. `init_megadrop_proof_buffer` — allocate a PDA at
//!    `["megadrop_proof_buffer", holder, payer]` sized to fit the proof bytes.
//! 2. `write_megadrop_proof_buffer` — append `chunk_bytes` at `offset` (idempotent).
//!    Issue this 1+ times across 1+ txs until the buffer is full.
//! 3. `claim_megadrop_from_buffer` — same checks as `claim_megadrop`, but the
//!    `proof: Vec<[u8; 32]>` argument is replaced by reading siblings from the buffer
//!    PDA. The buffer is closed (rent → payer) after a successful claim.
//!
//! Buffer layout (header 16 B + raw bytes):
//!
//! ```text
//! [0]    discriminator = 0x03
//! [1]    version = 0x01
//! [2..4] reserved
//! [4..8] total_len (LE u32)
//! [8..12] bytes_written (LE u32)
//! [12..16] reserved
//! [16..] raw proof bytes (32 B each, sibling order)
//! ```

use crate::calendar::month_from_unix_timestamp;
use crate::ed25519::{parse_ed25519_at, require_instructions_sysvar};
use crate::error::MegadropError;
use crate::megadrop::{
    build_claim_message, compute_claim_amount, is_tranche_claimed, is_tranche_unlocked,
    set_tranche_claimed, validate_and_pack_tranches,
};
use crate::merkle::{leaf_hash, verify_inclusion};
use crate::state::{
    find_megadrop_proof_buffer_pda, ClaimedMegadrop, MegadropConfig, CLAIMED_MEGADROP_SEED,
    MEGADROP_CONFIG_SEED, MEGADROP_PROOF_BUFFER_SEED, PROOF_BUFFER_DISCRIMINATOR,
    PROOF_BUFFER_HEADER_SIZE, PROOF_BUFFER_VERSION,
};
use anchor_lang::prelude::*;
use anchor_lang::system_program;
use solana_program::hash::Hash;

// ---------------------------------------------------------------------------
// init_megadrop_proof_buffer
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct InitMegadropProofBufferArgs {
    /// Holder identity — the leaf pubkey this proof will eventually verify against.
    pub holder_pubkey: [u8; 32],
    /// Total bytes the buffer will hold (sum of sibling bytes; `proof_len * 32`).
    pub total_len: u32,
}

#[derive(Accounts)]
#[instruction(args: InitMegadropProofBufferArgs)]
pub struct InitMegadropProofBuffer<'info> {
    /// Pays rent + signs the create CPI. Used as part of the PDA seeds — multiple
    /// concurrent stagings for the same holder by different payers don't collide.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: address validated against `find_megadrop_proof_buffer_pda` in handler;
    /// allocated via system_program create_account CPI.
    #[account(mut)]
    pub proof_buffer: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

pub fn init_proof_buffer_handler(
    ctx: Context<InitMegadropProofBuffer>,
    args: InitMegadropProofBufferArgs,
) -> Result<()> {
    let program_id = crate::ID;
    let holder_pk = Pubkey::new_from_array(args.holder_pubkey);
    let (expected, bump) =
        find_megadrop_proof_buffer_pda(&holder_pk, ctx.accounts.payer.key, &program_id);
    require_keys_eq!(
        ctx.accounts.proof_buffer.key(),
        expected,
        MegadropError::BadProofBufferPda
    );

    let space = PROOF_BUFFER_HEADER_SIZE
        .checked_add(args.total_len as usize)
        .ok_or(error!(MegadropError::ProofBufferOverflow))?;
    let rent = Rent::get()?;
    let lamports_for_rent = rent.minimum_balance(space);

    let payer_key = ctx.accounts.payer.key();
    let bump_arr = [bump];
    let seeds: &[&[u8]] = &[
        MEGADROP_PROOF_BUFFER_SEED,
        args.holder_pubkey.as_ref(),
        payer_key.as_ref(),
        &bump_arr,
    ];
    let signer_seeds: &[&[&[u8]]] = &[seeds];

    system_program::create_account(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.key(),
            system_program::CreateAccount {
                from: ctx.accounts.payer.to_account_info(),
                to: ctx.accounts.proof_buffer.to_account_info(),
            },
            signer_seeds,
        ),
        lamports_for_rent,
        space as u64,
        &program_id,
    )?;

    // Write the header. `bytes_written = 0`.
    let mut data = ctx.accounts.proof_buffer.try_borrow_mut_data()?;
    if data.len() < PROOF_BUFFER_HEADER_SIZE {
        return Err(error!(MegadropError::BadProofBuffer));
    }
    data[0] = PROOF_BUFFER_DISCRIMINATOR;
    data[1] = PROOF_BUFFER_VERSION;
    data[2] = 0;
    data[3] = 0;
    data[4..8].copy_from_slice(&args.total_len.to_le_bytes());
    data[8..12].copy_from_slice(&0u32.to_le_bytes());
    data[12..16].copy_from_slice(&[0u8; 4]);
    Ok(())
}

// ---------------------------------------------------------------------------
// write_megadrop_proof_buffer
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct WriteMegadropProofBufferArgs {
    /// Byte offset within the buffer payload.
    pub offset: u32,
    /// Bytes to copy into `[offset, offset+len)`.
    pub bytes: Vec<u8>,
}

#[derive(Accounts)]
pub struct WriteMegadropProofBuffer<'info> {
    /// CHECK: ownership + header validated in the handler. Anyone can write — staking
    /// the proof against an existing buffer doesn't grant any state mutation; the
    /// final `claim_megadrop_from_buffer` re-verifies the proof against the embedded
    /// root before crediting tranches.
    #[account(mut)]
    pub proof_buffer: AccountInfo<'info>,
}

pub fn write_proof_buffer_handler(
    ctx: Context<WriteMegadropProofBuffer>,
    args: WriteMegadropProofBufferArgs,
) -> Result<()> {
    let program_id = crate::ID;
    require_keys_eq!(
        *ctx.accounts.proof_buffer.owner,
        program_id,
        MegadropError::BadProofBuffer
    );

    let mut data = ctx.accounts.proof_buffer.try_borrow_mut_data()?;
    if data.len() < PROOF_BUFFER_HEADER_SIZE {
        return Err(error!(MegadropError::BadProofBuffer));
    }
    if data[0] != PROOF_BUFFER_DISCRIMINATOR || data[1] != PROOF_BUFFER_VERSION {
        return Err(error!(MegadropError::BadProofBuffer));
    }
    let total_len = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let bytes_written =
        u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;

    let off = args.offset as usize;
    let end = off
        .checked_add(args.bytes.len())
        .ok_or(error!(MegadropError::ProofBufferOverflow))?;
    if end > total_len {
        return Err(error!(MegadropError::ProofBufferOverflow));
    }
    let abs_end = PROOF_BUFFER_HEADER_SIZE
        .checked_add(end)
        .ok_or(error!(MegadropError::ProofBufferOverflow))?;
    if abs_end > data.len() {
        return Err(error!(MegadropError::ProofBufferOverflow));
    }
    let abs_off = PROOF_BUFFER_HEADER_SIZE + off;
    data[abs_off..abs_end].copy_from_slice(&args.bytes);

    let new_written = core::cmp::max(bytes_written, end) as u32;
    data[8..12].copy_from_slice(&new_written.to_le_bytes());
    Ok(())
}

// ---------------------------------------------------------------------------
// claim_megadrop_from_buffer
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ClaimMegadropFromBufferArgs {
    pub holder_pubkey: [u8; 32],
    pub total_allocation: u64,
    pub tranche_indices: Vec<u8>,
    /// Sibling count. Total proof bytes = `proof_len * 32`. Read from `proof_buffer`.
    pub proof_len: u16,
    pub proof_flags: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: ClaimMegadropFromBufferArgs)]
pub struct ClaimMegadropFromBuffer<'info> {
    /// Pays for the per-holder claimed-megadrop PDA the first time the holder claims.
    /// Also the `payer` keyed in the proof-buffer PDA seeds — must match the address
    /// that called `init_megadrop_proof_buffer`.
    #[account(mut)]
    pub relayer: Signer<'info>,

    #[account(
        seeds = [MEGADROP_CONFIG_SEED],
        bump = megadrop_config.bump,
    )]
    pub megadrop_config: Account<'info, MegadropConfig>,

    #[account(
        init_if_needed,
        payer = relayer,
        space = ClaimedMegadrop::SPACE,
        seeds = [CLAIMED_MEGADROP_SEED, args.holder_pubkey.as_ref()],
        bump,
    )]
    pub claimed_megadrop: Account<'info, ClaimedMegadrop>,

    /// CHECK: pubkey verified against `megadrop_config.treasury_authority` in handler.
    #[account(mut)]
    pub treasury: AccountInfo<'info>,

    /// CHECK: pubkey verified against `args.holder_pubkey` in handler.
    #[account(mut)]
    pub recipient: AccountInfo<'info>,

    /// CHECK: validated against the canonical sysvar pubkey in the handler.
    pub instructions_sysvar: AccountInfo<'info>,

    pub system_program: Program<'info, System>,

    /// CHECK: address validated against `find_megadrop_proof_buffer_pda(holder, relayer)`
    /// in the handler. Closed after successful claim (lamports → relayer).
    #[account(mut)]
    pub proof_buffer: AccountInfo<'info>,
}

pub fn claim_megadrop_from_buffer_handler(
    ctx: Context<ClaimMegadropFromBuffer>,
    args: ClaimMegadropFromBufferArgs,
) -> Result<()> {
    require_instructions_sysvar(&ctx.accounts.instructions_sysvar)?;

    // Validate proof buffer PDA + ownership.
    let program_id = crate::ID;
    let holder_pk = Pubkey::new_from_array(args.holder_pubkey);
    let (expected_buffer, _bump) =
        find_megadrop_proof_buffer_pda(&holder_pk, ctx.accounts.relayer.key, &program_id);
    require_keys_eq!(
        ctx.accounts.proof_buffer.key(),
        expected_buffer,
        MegadropError::BadProofBufferPda
    );
    require_keys_eq!(
        *ctx.accounts.proof_buffer.owner,
        program_id,
        MegadropError::BadProofBuffer
    );

    // Read the staged proof bytes into a Vec<Hash>.
    let proof_byte_len = (args.proof_len as usize)
        .checked_mul(32)
        .ok_or(error!(MegadropError::ProofBufferOverflow))?;
    let proof_hashes = {
        let buf = ctx.accounts.proof_buffer.try_borrow_data()?;
        if buf.len() < PROOF_BUFFER_HEADER_SIZE {
            return Err(error!(MegadropError::BadProofBuffer));
        }
        if buf[0] != PROOF_BUFFER_DISCRIMINATOR || buf[1] != PROOF_BUFFER_VERSION {
            return Err(error!(MegadropError::BadProofBuffer));
        }
        let total_len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        let bytes_written =
            u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]) as usize;
        if total_len < proof_byte_len {
            return Err(error!(MegadropError::ProofBufferLengthMismatch));
        }
        if bytes_written < proof_byte_len {
            return Err(error!(MegadropError::ProofBufferIncomplete));
        }
        let payload_start = PROOF_BUFFER_HEADER_SIZE;
        let payload_end = payload_start + proof_byte_len;
        if payload_end > buf.len() {
            return Err(error!(MegadropError::ProofBufferOverflow));
        }
        let mut hashes: Vec<Hash> = Vec::with_capacity(args.proof_len as usize);
        for i in 0..(args.proof_len as usize) {
            let off = payload_start + i * 32;
            let mut sibling = [0u8; 32];
            sibling.copy_from_slice(&buf[off..off + 32]);
            hashes.push(Hash::new_from_array(sibling));
        }
        hashes
    };

    // -- Mirror the standard claim_megadrop flow ---------------------------

    let (sorted_tranches, requested_bits) =
        validate_and_pack_tranches(&args.tranche_indices)?;

    let expected_flag_bytes = (args.proof_len as usize + 7) / 8;
    require!(
        args.proof_flags.len() >= expected_flag_bytes,
        MegadropError::ProofLengthMismatch
    );

    let cfg = &ctx.accounts.megadrop_config;
    let leaf = leaf_hash(&args.holder_pubkey, args.total_allocation);
    let expected_root = Hash::new_from_array(cfg.claimable_root);
    if !verify_inclusion(leaf, &proof_hashes, &args.proof_flags, &expected_root) {
        return Err(error!(MegadropError::BadMerkleProof));
    }

    let sysvar = &ctx.accounts.instructions_sysvar;
    let current_ix_index = solana_instructions_sysvar::load_current_index_checked(sysvar)
        .map_err(|_| MegadropError::BadInstructionsSysvar)?;
    if current_ix_index == 0 {
        return Err(error!(MegadropError::MissingEd25519Precompile));
    }
    let prev_ix_index = (current_ix_index as usize) - 1;
    let parsed = parse_ed25519_at(sysvar, prev_ix_index)?;
    if parsed.pubkey != args.holder_pubkey {
        return Err(error!(MegadropError::SignerPubkeyMismatch));
    }
    let program_id_bytes = crate::ID.to_bytes();
    let expected_msg = build_claim_message(
        &args.holder_pubkey,
        args.total_allocation,
        &sorted_tranches,
        &program_id_bytes,
    );
    if parsed.message != expected_msg {
        return Err(error!(MegadropError::SignedMessageMismatch));
    }

    require!(
        ctx.accounts.recipient.key.to_bytes() == args.holder_pubkey,
        MegadropError::RecipientMismatch
    );
    require!(
        *ctx.accounts.treasury.key == cfg.treasury_authority,
        MegadropError::BadTreasuryAccount
    );

    let clock = Clock::get()?;
    let current_month = month_from_unix_timestamp(clock.unix_timestamp)?;
    for &tranche_idx in &sorted_tranches {
        let unlocked = is_tranche_unlocked(cfg.genesis_month, current_month, tranche_idx)?;
        require!(unlocked, MegadropError::TrancheNotUnlocked);
    }

    let claimed = &mut ctx.accounts.claimed_megadrop;
    let pre_existing_bits = claimed.tranches_claimed;
    for &tranche_idx in &sorted_tranches {
        if is_tranche_claimed(pre_existing_bits, tranche_idx)? {
            return Err(error!(MegadropError::TrancheAlreadyClaimed));
        }
    }

    if claimed.holder == Pubkey::default() {
        claimed.holder = Pubkey::new_from_array(args.holder_pubkey);
        claimed.total_allocation = args.total_allocation;
        claimed.tranches_claimed = 0;
        claimed.total_claimed_lamports = 0;
        claimed.bump = ctx.bumps.claimed_megadrop;
    } else {
        require!(
            claimed.total_allocation == args.total_allocation,
            MegadropError::TotalAllocationMismatch
        );
    }

    let mut new_bits = claimed.tranches_claimed;
    for &tranche_idx in &sorted_tranches {
        new_bits = set_tranche_claimed(new_bits, tranche_idx)?;
    }
    claimed.tranches_claimed = new_bits;

    let claim_amount = compute_claim_amount(args.total_allocation, requested_bits)?;
    let treasury_lamports = ctx.accounts.treasury.lamports();
    require!(
        treasury_lamports >= claim_amount,
        MegadropError::InsufficientTreasuryBalance
    );
    **ctx.accounts.treasury.try_borrow_mut_lamports()? = treasury_lamports
        .checked_sub(claim_amount)
        .ok_or(MegadropError::InsufficientTreasuryBalance)?;
    **ctx.accounts.recipient.try_borrow_mut_lamports()? = ctx
        .accounts
        .recipient
        .lamports()
        .checked_add(claim_amount)
        .ok_or(MegadropError::ClaimAmountOverflow)?;

    claimed.total_claimed_lamports = claimed
        .total_claimed_lamports
        .checked_add(claim_amount)
        .ok_or(MegadropError::ClaimAmountOverflow)?;

    // Close the proof buffer — return rent to the relayer/payer; zero data.
    {
        let relayer_ai = ctx.accounts.relayer.to_account_info();
        let mut buffer_lamports = ctx.accounts.proof_buffer.try_borrow_mut_lamports()?;
        let mut payer_lamports = relayer_ai.try_borrow_mut_lamports()?;
        let amount = **buffer_lamports;
        **buffer_lamports = 0;
        **payer_lamports = payer_lamports
            .checked_add(amount)
            .ok_or(MegadropError::ClaimAmountOverflow)?;

        let mut data = ctx.accounts.proof_buffer.try_borrow_mut_data()?;
        for b in data.iter_mut() {
            *b = 0;
        }
    }

    Ok(())
}

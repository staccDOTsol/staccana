//! `claim_megadrop` — user-facing claim instruction.
//!
//! Anyone can submit (the holder, or a relayer on their behalf — but the holder must
//! have produced a fresh ed25519 signature on the canonical message). Lamports always
//! land at the holder's pubkey.
//!
//! Verification order — exactly as `docs/MEGADROP.md` "Claim instruction" prescribes:
//!
//! 1. Validate `tranche_indices` (non-empty, in `[1, 10]`, no duplicates) and pack
//!    into a 16-bit bitmap of "newly requested" tranches.
//! 2. Verify the Merkle proof against `MegadropConfig.claimable_root` for
//!    `(holder_pubkey, total_allocation)` — same hashing as
//!    `staccana_genesis::merkle`.
//! 3. Inspect the prior instruction in the transaction via the Instructions sysvar;
//!    it must be an ed25519 precompile signing the canonical claim message with
//!    `holder_pubkey`.
//! 4. The recipient account must equal `holder_pubkey`.
//! 5. Each requested tranche must be unlocked at the current calendar month.
//! 6. Each requested tranche must NOT already be set in
//!    `ClaimedMegadrop.tranches_claimed`.
//!
//! Effects:
//!
//! 1. Mark requested tranche bits in `ClaimedMegadrop.tranches_claimed`.
//! 2. Compute `claim_amount = popcount(newly_requested_bits) × (total / 10)`.
//! 3. Debit `claim_amount` lamports from the treasury PDA → holder pubkey.
//! 4. Update `ClaimedMegadrop.total_claimed_lamports`.
//!
//! ## Treasury debit
//!
//! The treasury PDA is owned by THIS program (per the genesis-wiring requirement
//! documented in `state.rs`). The handler mutates lamports directly via
//! `try_borrow_mut_lamports` — no CPI needed because the runtime trusts a program to
//! mutate lamports of accounts it owns. This mirrors the
//! `staccana_validator_subsidy::distribute_yield` lamport-shuffle pattern.

use crate::calendar::month_from_unix_timestamp;
use crate::ed25519::{parse_ed25519_at, require_instructions_sysvar};
use crate::error::MegadropError;
use crate::megadrop::{
    build_claim_message, compute_claim_amount, is_tranche_claimed, is_tranche_unlocked,
    set_tranche_claimed, validate_and_pack_tranches,
};
use crate::merkle::{leaf_hash, verify_inclusion};
use crate::state::{
    ClaimedMegadrop, MegadropConfig, CLAIMED_MEGADROP_SEED, MEGADROP_CONFIG_SEED,
};
use anchor_lang::prelude::*;
use solana_program::hash::Hash;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ClaimMegadropArgs {
    /// Holder identity. The lamports land here; the ed25519 sig must be from this key.
    pub holder_pubkey: [u8; 32],

    /// Total lamport allocation for this holder. Must match the `(holder, total)`
    /// pair committed to in the leaf — verified via the Merkle proof.
    pub total_allocation: u64,

    /// 1-indexed tranche numbers to claim in this ix (1..=10). Multiple supported
    /// for back-claims; duplicates rejected.
    pub tranche_indices: Vec<u8>,

    /// Merkle inclusion proof — siblings from leaf-level upward.
    pub proof: Vec<[u8; 32]>,

    /// Packed sibling-side bit flags. Bit `i` controls level `i`:
    /// `0` ⇒ sibling on left, `1` ⇒ sibling on right. See `merkle.rs` doc.
    pub proof_flags: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: ClaimMegadropArgs)]
pub struct ClaimMegadrop<'info> {
    /// Pays for the per-holder claimed-megadrop PDA the first time the holder claims.
    /// Anyone can fill this slot — typically the holder themselves, occasionally a
    /// relayer (the holder still must have signed the claim message).
    #[account(mut)]
    pub relayer: Signer<'info>,

    #[account(
        seeds = [MEGADROP_CONFIG_SEED],
        bump = megadrop_config.bump,
    )]
    pub megadrop_config: Account<'info, MegadropConfig>,

    /// Per-holder claim state. Lazily initialized on the holder's first claim;
    /// re-used in place on subsequent claims.
    #[account(
        init_if_needed,
        payer = relayer,
        space = ClaimedMegadrop::SPACE,
        seeds = [CLAIMED_MEGADROP_SEED, args.holder_pubkey.as_ref()],
        bump,
    )]
    pub claimed_megadrop: Account<'info, ClaimedMegadrop>,

    /// Treasury PDA — lamport source. Must be the configured treasury authority.
    /// CHECK: pubkey verified against `megadrop_config.treasury_authority` in the
    /// handler. Owned by THIS program (genesis wiring) so direct lamport mutation is
    /// permitted.
    #[account(mut)]
    pub treasury: AccountInfo<'info>,

    /// Recipient = holder. Lamports land here.
    /// CHECK: pubkey verified against `args.holder_pubkey` in the handler.
    #[account(mut)]
    pub recipient: AccountInfo<'info>,

    /// CHECK: validated against the canonical sysvar pubkey in the handler.
    pub instructions_sysvar: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

/// Handler — see module doc for the full verification order.
pub fn handler(ctx: Context<ClaimMegadrop>, args: ClaimMegadropArgs) -> Result<()> {
    require_instructions_sysvar(&ctx.accounts.instructions_sysvar)?;

    // Step 1: validate + pack the tranche indices into a 16-bit bitmap of "newly
    // requested" tranches. Sorted indices feed into the canonical signed message so
    // the order in `args.tranche_indices` doesn't matter for sig validity.
    let (sorted_tranches, requested_bits) =
        validate_and_pack_tranches(&args.tranche_indices)?;

    // Step 2: Merkle inclusion against the embedded root for the (holder, total)
    // leaf. Reject early if the proof flags are sized wrong — defense-in-depth before
    // hitting the bit-indexed walk.
    let expected_flag_bytes = (args.proof.len() + 7) / 8;
    require!(
        args.proof_flags.len() >= expected_flag_bytes,
        MegadropError::ProofLengthMismatch
    );

    let cfg = &ctx.accounts.megadrop_config;
    let leaf = leaf_hash(&args.holder_pubkey, args.total_allocation);
    let proof_hashes: Vec<Hash> = args
        .proof
        .iter()
        .map(|b| Hash::new_from_array(*b))
        .collect();
    let expected_root = Hash::new_from_array(cfg.claimable_root);
    if !verify_inclusion(leaf, &proof_hashes, &args.proof_flags, &expected_root) {
        return Err(error!(MegadropError::BadMerkleProof));
    }

    // Step 3: ed25519 precompile must immediately precede this ix and sign the
    // canonical claim message with `holder_pubkey`. Mirrors the lazy-claim pattern.
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

    // Step 4: recipient must equal holder.
    require!(
        ctx.accounts.recipient.key.to_bytes() == args.holder_pubkey,
        MegadropError::RecipientMismatch
    );

    // Step 5: treasury must equal the configured authority.
    require!(
        *ctx.accounts.treasury.key == cfg.treasury_authority,
        MegadropError::BadTreasuryAccount
    );

    // Step 6: per-tranche unlock check using the Clock sysvar's Unix timestamp.
    let clock = Clock::get()?;
    let current_month = month_from_unix_timestamp(clock.unix_timestamp)?;
    for &tranche_idx in &sorted_tranches {
        let unlocked = is_tranche_unlocked(cfg.genesis_month, current_month, tranche_idx)?;
        require!(unlocked, MegadropError::TrancheNotUnlocked);
    }

    // Step 7: per-tranche freshness check against the existing bitmap. Done after
    // the unlock gate so callers see "not yet unlocked" rather than "already
    // claimed" when both are technically true (impossible state — already claimed
    // implies previously unlocked — but the error-precedence is clearer this way).
    let claimed = &mut ctx.accounts.claimed_megadrop;
    let pre_existing_bits = claimed.tranches_claimed;
    for &tranche_idx in &sorted_tranches {
        if is_tranche_claimed(pre_existing_bits, tranche_idx)? {
            return Err(error!(MegadropError::TrancheAlreadyClaimed));
        }
    }

    // -- All checks passed; commit state -----------------------------------------

    // First-claim path: stamp the holder identity and the leaf's total allocation
    // into the freshly allocated PDA. Subsequent calls must produce the same
    // `total_allocation` (since the leaf is immutable); otherwise something is
    // very wrong with the caller (or the program data has been corrupted).
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

    // Mark the requested tranches in the bitmap.
    let mut new_bits = claimed.tranches_claimed;
    for &tranche_idx in &sorted_tranches {
        new_bits = set_tranche_claimed(new_bits, tranche_idx)?;
    }
    claimed.tranches_claimed = new_bits;

    // Compute the lamport amount payable for the newly requested tranches only.
    let claim_amount = compute_claim_amount(args.total_allocation, requested_bits)?;

    // Treasury debit (direct lamport mutation; treasury is owned by this program).
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

    emit!(ClaimMegadropEvent {
        holder: Pubkey::new_from_array(args.holder_pubkey),
        tranche_bits_set: requested_bits,
        claim_amount,
        total_claimed_lamports: claimed.total_claimed_lamports,
    });

    Ok(())
}

/// Emitted on successful claim. Off-chain indexers consume this to reconstruct
/// per-holder claim history without scanning every PDA.
#[event]
pub struct ClaimMegadropEvent {
    pub holder: Pubkey,
    pub tranche_bits_set: u16,
    pub claim_amount: u64,
    pub total_claimed_lamports: u64,
}

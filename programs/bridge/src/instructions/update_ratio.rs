//! `update_ratio` — federation publishes a new R for an asset.
//!
//! The federation produces a `STACCANA_RATIO_V1` attestation (SPEC.md §5.3) and M
//! members co-sign it with separate ed25519 precompile instructions, all preceding the
//! `update_ratio` ix in the same transaction.
//!
//! Verification flow:
//! 1. Inspect the prior M ed25519 precompile ixs via the Instructions sysvar.
//! 2. Each precompile ix MUST sign the exact bytes returned by
//!    [`crate::attestation::build_ratio_message`] for the supplied args.
//! 3. Each precompile ix MUST be signed by a distinct registered federation member,
//!    chosen by `federation_indices[i]`.
//! 4. Slot must be `>= last_published_slot + R_PUBLISH_INTERVAL_SLOTS`.
//! 5. Recompute R from `(vault_value, mint_supply)` and store it.
//!
//! Re-derivation of R from the attested `(vault_value, mint_supply)` (rather than
//! trusting a pre-computed R from the federation) means a buggy federation client
//! can't poison the bridge with a wrong ratio: the worst it can do is move R off by
//! the underlying valuation it claims, which is the same trust assumption as honest
//! reporting in the first place.

use crate::attestation::{
    build_ratio_message, check_unique_indices, compute_r_q64,
};
use crate::ed25519::{parse_ed25519_at, require_instructions_sysvar};
use crate::error::BridgeError;
use crate::state::{AssetConfig, FederationSet, RatioState};
use anchor_lang::prelude::*;

/// `R_PUBLISH_INTERVAL_SLOTS` — minimum gap between successive R updates per asset.
/// Pinned here from SPEC §2.3 so the constant lives next to the consumer.
pub const R_PUBLISH_INTERVAL_SLOTS: u64 = 150;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct UpdateRatioArgs {
    pub asset_id: u32,
    /// Total underlying value held by the mainnet vault, expressed in the underlying's
    /// native units (e.g. lamports for stSOL/pSYRUP).
    pub vault_value: u64,
    /// Current staccana mint supply at the slot the federation observed. The bridge
    /// program does NOT verify against the on-chain mint supply — staleness is fine
    /// as long as the (value, supply) pair was observed atomically by the federation.
    pub mint_supply: u64,
    /// Slot at which the federation observed `(vault_value, mint_supply)`.
    pub slot: u64,
    /// Strictly increasing per-asset attestation nonce.
    pub nonce: u64,
    /// Indices into `FederationSet::members`, one per signature. Length M; duplicates
    /// rejected.
    pub federation_indices: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: UpdateRatioArgs)]
pub struct UpdateRatio<'info> {
    /// Anyone can submit the ratio update — the verification is in the M signatures.
    /// Charging the relayer is appropriate since the federation may have many willing
    /// relayers.
    pub relayer: Signer<'info>,

    #[account(
        seeds = [b"asset", args.asset_id.to_le_bytes().as_ref()],
        bump = asset_config.bump,
    )]
    pub asset_config: Account<'info, AssetConfig>,

    #[account(
        mut,
        seeds = [b"ratio", args.asset_id.to_le_bytes().as_ref()],
        bump = ratio_state.bump,
    )]
    pub ratio_state: Account<'info, RatioState>,

    #[account(
        seeds = [b"federation"],
        bump = federation_set.bump,
    )]
    pub federation_set: Account<'info, FederationSet>,

    /// CHECK: validated against the canonical sysvar pubkey in the handler.
    pub instructions_sysvar: UncheckedAccount<'info>,
}

/// Handler — verify M federation signatures, recompute R, store it.
pub fn handler(ctx: Context<UpdateRatio>, args: UpdateRatioArgs) -> Result<()> {
    require_instructions_sysvar(&ctx.accounts.instructions_sysvar)?;

    // R-locked assets (wSOL) reject ALL ratio updates — R is structurally pinned at 1.0.
    // Verified BEFORE signature checks so no work is wasted on a structurally-invalid
    // attestation. See `docs/BRIDGE.md` §"Native SOL ↔ mainnet SOL via the bridge".
    require!(
        !ctx.accounts.asset_config.is_r_locked(),
        BridgeError::RatioLocked
    );

    let fed = &ctx.accounts.federation_set;
    require!(fed.n > 0 && fed.m > 0, BridgeError::BadFederationSet);
    require!(
        args.federation_indices.len() == fed.m as usize,
        BridgeError::InsufficientFederationSignatures
    );
    check_unique_indices(&args.federation_indices, fed.n)?;

    let expected_msg = build_ratio_message(
        args.asset_id,
        args.vault_value,
        args.mint_supply,
        args.slot,
        args.nonce,
    );

    // The M ed25519 precompile ixs MUST be the M instructions IMMEDIATELY preceding
    // this `update_ratio` ix. We discover the current ix index via the sysvar and walk
    // backward, parsing each precompile and matching the expected message + signer.
    let sysvar = &ctx.accounts.instructions_sysvar;
    let current_ix_index = solana_instructions_sysvar::load_current_index_checked(sysvar)
        .map_err(|_| BridgeError::BadInstructionsSysvar)?;
    let m = fed.m as usize;
    require!(
        (current_ix_index as usize) >= m,
        BridgeError::InsufficientFederationSignatures
    );

    for (i, &member_idx) in args.federation_indices.iter().enumerate() {
        // Walk backward: the i-th precompile (0-indexed) is at `current - m + i`.
        let ix_index = (current_ix_index as usize) - m + i;
        let parsed = parse_ed25519_at(sysvar, ix_index)?;
        require!(
            parsed.message == expected_msg,
            BridgeError::BadAttestationMessage
        );
        let expected_member = fed.members[member_idx as usize];
        require!(
            parsed.pubkey == expected_member.to_bytes(),
            BridgeError::BadFederationSigner
        );
    }

    let ratio = &mut ctx.accounts.ratio_state;

    // Asset-id binding: PDA seed already enforces `args.asset_id` matches
    // `ratio.asset_id`, but make it explicit so a future seed refactor doesn't silently
    // unbind.
    require!(
        ratio.asset_id == args.asset_id,
        BridgeError::AssetIdMismatch
    );

    // Slot interval gate — first update (last_published_slot == 0) lands any time;
    // subsequent updates must wait the full interval.
    if ratio.last_published_slot != 0 {
        require!(
            args.slot >= ratio.last_published_slot + R_PUBLISH_INTERVAL_SLOTS,
            BridgeError::RatioUpdateTooSoon
        );
    }
    require!(args.nonce > ratio.last_nonce, BridgeError::BadInstructionData);

    let new_r = compute_r_q64(args.vault_value, args.mint_supply)?;
    ratio.r_q64 = new_r;
    ratio.last_published_slot = args.slot;
    ratio.last_nonce = args.nonce;

    Ok(())
}

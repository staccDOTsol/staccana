//! `set_bootstrap_per_epoch` — governance-gated knob for the per-epoch
//! bootstrap-reserve drip rate.
//!
//! Why this exists
//! ---------------
//!
//! `init_subsidy` sets `bootstrap_reserve_initial = treasury_total *
//! TREASURY_BOOTSTRAP_BPS / 10_000` (default 2%). With genesis treasury
//! at 485M SOL that's 9.7M SOL → 161,730 SOL/epoch over 60 epochs. Split
//! across only 3 registered validators that paid out ~54K SOL each in
//! epoch 2 — which is fine at scale (50+ validators dilutes the share)
//! but absurd for the boot-up window.
//!
//! This ix lets governance retune the per-epoch rate after init_subsidy
//! lands — useful when the registered-validator count is far from the
//! steady-state assumption baked into the original sizing.
//!
//! Mechanics
//! ---------
//!
//! `bootstrap_distribute` reads `bootstrap_per_epoch(reserve_initial) =
//! reserve_initial / BOOTSTRAP_EPOCHS` to decide each epoch's allotment.
//! So setting `reserve_initial = target * BOOTSTRAP_EPOCHS` reproduces
//! `target` lamports per epoch.
//!
//! We also clamp `reserve_remaining` at the new `reserve_initial`. If the
//! old reserve had more lamports earmarked than the new sizing implies,
//! that surplus stops being earmarked for bootstrap (it stays in treasury
//! and becomes available for the regular yield-distribution path post-
//! bootstrap). If the old reserve had less remaining (we've already
//! distributed for some epochs), we keep the smaller value.
//!
//! Idempotent. Reversible — call again with a different `target_per_epoch`
//! to retune.
//!
//! Authorization
//! -------------
//!
//! Same gate as `register_validator` / `stake_to_productive`:
//! `authority.key() == subsidy_config.governance`.

use crate::error::SubsidyError;
use crate::state::{SubsidyConfig, BOOTSTRAP_EPOCHS};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SetBootstrapPerEpochArgs {
    /// New per-epoch lamports allotment for `bootstrap_distribute`.
    /// Set to 0 to halt bootstrap distributions entirely (then governance
    /// can re-enable later by calling again with a non-zero value).
    pub target_per_epoch: u64,
}

#[derive(Accounts)]
pub struct SetBootstrapPerEpoch<'info> {
    #[account(
        mut,
        constraint = authority.key() == subsidy_config.governance
            @ SubsidyError::BadInstructionData,
    )]
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [b"subsidy_config"],
        bump = subsidy_config.bump,
    )]
    pub subsidy_config: Account<'info, SubsidyConfig>,
}

pub fn handler(
    ctx: Context<SetBootstrapPerEpoch>,
    args: SetBootstrapPerEpochArgs,
) -> Result<()> {
    let cfg = &mut ctx.accounts.subsidy_config;

    // New initial-reserve sizing: target_per_epoch × BOOTSTRAP_EPOCHS.
    // Saturating-mul guards against governance accidentally requesting a
    // value that overflows u64 (target ≈ u64::MAX / 60). The saturating
    // path means the cap silently caps at u64::MAX, which is a fine
    // failure mode — the cluster will never see numbers that large.
    let new_initial = args
        .target_per_epoch
        .saturating_mul(BOOTSTRAP_EPOCHS);

    let old_initial = cfg.bootstrap_reserve_initial;
    let old_remaining = cfg.bootstrap_reserve_remaining;

    cfg.bootstrap_reserve_initial = new_initial;
    cfg.bootstrap_reserve_remaining = old_remaining.min(new_initial);

    msg!(
        "[set_bootstrap_per_epoch] target={} initial: {} -> {}, remaining: {} -> {}",
        args.target_per_epoch,
        old_initial,
        new_initial,
        old_remaining,
        cfg.bootstrap_reserve_remaining,
    );
    Ok(())
}

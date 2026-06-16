//! `admin_set_validator_metrics` — bootstrap-only escape hatch for setting
//! validator metrics without federation attestation.
//!
//! Why this exists
//! ---------------
//!
//! `update_validator_metrics` requires M-of-N ed25519 precompile signatures
//! from the federation pubkeys baked into `SubsidyConfig`. The federation
//! attestor isn't running yet (`tools/federation-attestor` only handles
//! bridge mint / release messages today — no validator-metrics signer
//! loop). Until that lands, this ix lets `ADMIN_AUTHORITY` directly set
//! per-validator `uptime_bps` / `delegated_stake` / `votes_cast` so
//! `bootstrap_distribute` has non-zero `total_weight` to divvy up the
//! reserve.
//!
//! Once the federation attestor handles validator metrics, this ix can be
//! removed (or left in place but ignored — admin still owns upgrade auth).
//!
//! Authorization
//! -------------
//!
//! Gated on `ADMIN_AUTHORITY` (= `HSwe2Y7i…`, the BPF upgrade-authority key)
//! NOT on `subsidy_config.governance` — keeping it tied to the upgrade-auth
//! makes the bootstrap-only nature explicit. Same gating pattern as
//! `init_subsidy`.

use crate::error::SubsidyError;
use crate::state::{SubsidyConfig, ValidatorRecord, ValidatorRegistry};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct AdminSetMetricsArgs {
    /// Validator identity pubkey whose record is being updated.
    pub validator: Pubkey,
    /// Uptime in basis points (10_000 == 100%). `update_validator_metrics`
    /// rejects > 10_000; we mirror the same check.
    pub uptime_bps: u16,
    /// Delegated stake (lamports).
    pub delegated_stake: u64,
    /// Votes cast in the prior epoch.
    pub votes_cast: u64,
}

#[derive(Accounts)]
#[instruction(args: AdminSetMetricsArgs)]
pub struct AdminSetMetrics<'info> {
    /// Must equal `crate::ADMIN_AUTHORITY` — staccana's BPF upgrade-auth.
    #[account(
        mut,
        constraint = authority.key() == crate::ADMIN_AUTHORITY
            @ SubsidyError::BadInstructionData,
    )]
    pub authority: Signer<'info>,

    #[account(
        seeds = [b"subsidy_config"],
        bump = subsidy_config.bump,
    )]
    pub subsidy_config: Account<'info, SubsidyConfig>,

    /// Registry — sanity check the validator is registered.
    #[account(
        seeds = [b"validator_registry"],
        bump,
    )]
    pub validator_registry: AccountLoader<'info, ValidatorRegistry>,

    /// Per-validator record at `["validator", validator_pubkey]`.
    #[account(
        mut,
        seeds = [b"validator", args.validator.as_ref()],
        bump = validator_record.bump,
        constraint = validator_record.validator == args.validator
            @ SubsidyError::BadValidatorRecordPda,
    )]
    pub validator_record: Account<'info, ValidatorRecord>,
}

pub fn handler(ctx: Context<AdminSetMetrics>, args: AdminSetMetricsArgs) -> Result<()> {
    require!(args.uptime_bps <= 10_000, SubsidyError::BadInstructionData);

    // Sanity: validator must be in the registry. Linear scan over `count`
    // entries (≤ MAX_VALIDATORS = 256) — cheap.
    let registry = ctx.accounts.validator_registry.load()?;
    let n = registry.count as usize;
    let mut found = false;
    for k in 0..n {
        if registry.validators[k] == args.validator {
            found = true;
            break;
        }
    }
    require!(found, SubsidyError::ValidatorNotRegistered);

    let rec = &mut ctx.accounts.validator_record;
    rec.uptime_bps = args.uptime_bps;
    rec.delegated_stake = args.delegated_stake;
    rec.votes_cast = args.votes_cast;
    rec.last_metrics_slot = Clock::get()?.slot;
    rec.last_metrics_nonce = rec.last_metrics_nonce.saturating_add(1);

    msg!(
        "[admin_set_metrics] {} uptime={} stake={} votes={}",
        args.validator,
        args.uptime_bps,
        args.delegated_stake,
        args.votes_cast,
    );

    Ok(())
}

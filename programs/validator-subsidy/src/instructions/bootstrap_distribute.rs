//! `bootstrap_distribute` — permissionless per-epoch distribution from the bootstrap
//! reserve.
//!
//! Replaces [`distribute_yield`] for the first `BOOTSTRAP_EPOCHS` (60) epochs while the
//! productive position has not yet earned anything. SPEC §7.2 / §7.3.
//!
//! Per-epoch payout is `bootstrap_reserve_initial / BOOTSTRAP_EPOCHS` lamports
//! (see [`crate::subsidy::bootstrap_per_epoch`]). The truncation residue (up to
//! `BOOTSTRAP_EPOCHS - 1` lamports) stays in the reserve and rolls into yield-only
//! accounting after epoch 60.
//!
//! Validation:
//! - `args.epoch < BOOTSTRAP_EPOCHS`. After epoch 60, callers must use
//!   `distribute_yield` instead.
//! - Reserve has not been fully drained (`bootstrap_reserve_remaining > 0`).
//! - Same idempotency / total-weight gates as `distribute_yield`.
//!
//! Account-passing convention is identical to `distribute_yield`: 2 accounts per
//! validator (`ValidatorRecord`, then identity address) in registry order.
//!
//! ## Why a separate ix vs gating inside `distribute_yield`?
//!
//! Splitting them keeps each handler's invariants narrow. `distribute_yield` requires
//! `yield_observed > 0` (oracle has attested); `bootstrap_distribute` requires
//! `epoch < BOOTSTRAP_EPOCHS` (reserve still active). Mixing the two would force a
//! state-machine inside one handler that's harder to reason about and audit.

use crate::error::SubsidyError;
use crate::state::{
    EpochAccrual, SubsidyConfig, ValidatorRecord, ValidatorRegistry, BOOTSTRAP_EPOCHS,
};
use crate::subsidy::{bootstrap_per_epoch, compute_validator_share, compute_validator_weight};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct BootstrapDistributeArgs {
    /// Epoch to distribute. Must be `< BOOTSTRAP_EPOCHS`.
    pub epoch: u64,
}

#[derive(Accounts)]
#[instruction(args: BootstrapDistributeArgs)]
pub struct BootstrapDistribute<'info> {
    /// Anyone can submit. Pays for the `EpochAccrual` PDA allocation if not pre-init'd.
    #[account(mut)]
    pub relayer: Signer<'info>,

    #[account(
        mut,
        seeds = [b"subsidy_config"],
        bump = subsidy_config.bump,
    )]
    pub subsidy_config: Account<'info, SubsidyConfig>,

    #[account(
        seeds = [b"validator_registry"],
        bump,
    )]
    pub validator_registry: AccountLoader<'info, ValidatorRegistry>,

    /// `EpochAccrual` PDA. `init_if_needed` so the relayer can allocate it inline if
    /// the attestor hasn't pre-created it (which is the common case for bootstrap
    /// epochs since there's no oracle yield to attest to).
    #[account(
        init_if_needed,
        payer = relayer,
        space = EpochAccrual::SPACE,
        seeds = [b"accrual", args.epoch.to_le_bytes().as_ref()],
        bump,
    )]
    pub epoch_accrual: Account<'info, EpochAccrual>,

    /// Treasury PDA. See `distribute_yield.rs` notes on PDA ownership.
    /// CHECK: PDA seeds bind. Plain SystemProgram-style account holding lamports.
    #[account(
        mut,
        seeds = [b"treasury"],
        bump,
    )]
    pub treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
    // remaining_accounts: 2 per validator (record + identity), in registry order.
}

/// Handler — gate to bootstrap window, compute per-validator shares of the per-epoch
/// allotment, transfer.
///
/// Per-record reads / writes use manual `try_deserialize` / `try_serialize` rather than
/// `Account::try_from` for the same lifetime reason described in `distribute_yield.rs`.
pub fn handler(
    ctx: Context<BootstrapDistribute>,
    args: BootstrapDistributeArgs,
) -> Result<()> {
    require!(
        args.epoch < BOOTSTRAP_EPOCHS,
        SubsidyError::BootstrapEpochExpired
    );

    // Ensure the accrual has not already been distributed. The PDA may have been
    // freshly allocated by `init_if_needed` (in which case all fields are Default,
    // including `distributed = false`) or pre-allocated by the off-chain attestor for
    // a prior population of `yield_observed`. We bind/verify the epoch field
    // unconditionally so the field accurately reflects the PDA's seed regardless of
    // allocation path.
    let accrual_distributed = ctx.accounts.epoch_accrual.distributed;
    require!(!accrual_distributed, SubsidyError::EpochAlreadyDistributed);
    if ctx.accounts.epoch_accrual.epoch == 0 {
        // Either freshly allocated or pre-allocated for epoch 0 — both are fine since
        // PDA seeds bind `args.epoch.to_le_bytes()` to the account location. Just write
        // the epoch field to keep the on-chain shape consistent.
        ctx.accounts.epoch_accrual.epoch = args.epoch;
    } else {
        require!(
            ctx.accounts.epoch_accrual.epoch == args.epoch,
            SubsidyError::EpochMismatch
        );
    }

    let cfg_initial_reserve = ctx.accounts.subsidy_config.bootstrap_reserve_initial;
    let cfg_remaining = ctx.accounts.subsidy_config.bootstrap_reserve_remaining;
    require!(cfg_remaining > 0, SubsidyError::BootstrapReserveExhausted);

    // Per-epoch allotment is computed off the INITIAL reserve so the per-epoch amount
    // is a constant for the entire bootstrap window. Final epoch may underdraw if the
    // remaining reserve is less than the nominal allotment.
    let nominal_per_epoch = bootstrap_per_epoch(cfg_initial_reserve);
    let to_distribute = nominal_per_epoch.min(cfg_remaining);
    require!(to_distribute > 0, SubsidyError::BootstrapReserveExhausted);

    let registry_loader = &ctx.accounts.validator_registry;
    let registry = registry_loader.load()?;
    let registry_count = registry.count as usize;
    let expected_remaining = registry_count.checked_mul(2).ok_or(SubsidyError::BadInstructionData)?;
    require!(
        ctx.remaining_accounts.len() == expected_remaining,
        SubsidyError::RemainingAccountsMismatch
    );

    // First pass: total weight.
    let mut total_weight: u128 = 0;
    let mut weights: Vec<u128> = Vec::with_capacity(registry_count);

    for k in 0..registry_count {
        let record_ai = &ctx.remaining_accounts[2 * k];
        let identity_ai = &ctx.remaining_accounts[2 * k + 1];

        require_keys_eq!(
            *record_ai.owner,
            crate::ID,
            SubsidyError::BadValidatorRecordPda
        );
        let expected_validator = registry.validators[k];
        require_keys_eq!(
            *identity_ai.key,
            expected_validator,
            SubsidyError::RemainingAccountsMismatch
        );

        let record = ValidatorRecord::read_from(record_ai)
            .map_err(|_| error!(SubsidyError::BadValidatorRecordPda))?;
        require_keys_eq!(
            record.validator,
            expected_validator,
            SubsidyError::RemainingAccountsMismatch
        );

        let w = compute_validator_weight(
            record.uptime_bps,
            record.delegated_stake,
            record.votes_cast,
        );
        weights.push(w);
        total_weight = total_weight.saturating_add(w);
    }

    require!(total_weight > 0, SubsidyError::ZeroTotalWeight);

    // Second pass: pay out shares of `to_distribute` (NOT the entire remaining reserve;
    // each epoch only releases the per-epoch allotment).
    let treasury_ai = ctx.accounts.treasury.to_account_info();
    let mut distributed_total: u64 = 0;

    for k in 0..registry_count {
        let identity_ai = &ctx.remaining_accounts[2 * k + 1];
        let record_ai = &ctx.remaining_accounts[2 * k];

        let share = compute_validator_share(to_distribute, weights[k], total_weight)?;
        if share == 0 {
            continue;
        }

        let treasury_lamports = treasury_ai.lamports();
        require!(
            treasury_lamports >= share,
            SubsidyError::InsufficientTreasuryBalance
        );

        **treasury_ai.try_borrow_mut_lamports()? = treasury_lamports
            .checked_sub(share)
            .ok_or(SubsidyError::InsufficientTreasuryBalance)?;
        **identity_ai.try_borrow_mut_lamports()? = identity_ai
            .lamports()
            .checked_add(share)
            .ok_or(SubsidyError::ShareOverflow)?;

        let mut record = ValidatorRecord::read_from(record_ai)
            .map_err(|_| error!(SubsidyError::BadValidatorRecordPda))?;
        record.total_subsidy_received = record
            .total_subsidy_received
            .checked_add(share)
            .ok_or(SubsidyError::ShareOverflow)?;
        record.last_distribution_epoch = args.epoch;
        record
            .write_to(record_ai)
            .map_err(|_| error!(SubsidyError::BadValidatorRecordPda))?;

        distributed_total = distributed_total
            .checked_add(share)
            .ok_or(SubsidyError::ShareOverflow)?;
    }

    // Decrement the reserve by the amount actually distributed (sum of per-validator
    // shares), not by the nominal `to_distribute` — the truncation residue stays in
    // the reserve and may roll into the next epoch's allotment.
    let accrual = &mut ctx.accounts.epoch_accrual;
    accrual.epoch = args.epoch;
    accrual.distributed = true;
    accrual.total_weight = total_weight;
    accrual.distributed_total = distributed_total;
    accrual.bump = ctx.bumps.epoch_accrual;

    let cfg = &mut ctx.accounts.subsidy_config;
    cfg.bootstrap_reserve_remaining = cfg
        .bootstrap_reserve_remaining
        .checked_sub(distributed_total)
        .ok_or(SubsidyError::BootstrapReserveExhausted)?;
    cfg.last_distributed_epoch = args.epoch;

    emit!(BootstrapDistributeEvent {
        epoch: args.epoch,
        nominal_per_epoch,
        distributed_total,
        bootstrap_reserve_remaining: cfg.bootstrap_reserve_remaining,
        validator_count: registry_count as u32,
    });

    Ok(())
}

/// Emitted on successful bootstrap distribution.
#[event]
pub struct BootstrapDistributeEvent {
    pub epoch: u64,
    pub nominal_per_epoch: u64,
    pub distributed_total: u64,
    pub bootstrap_reserve_remaining: u64,
    pub validator_count: u32,
}

//! `distribute_yield` — permissionless per-epoch yield distribution.
//!
//! Reads the [`EpochAccrual`] PDA for `args.epoch` (which an oracle/federation has
//! already populated with the observed `yield_observed` from the productive position
//! over that epoch — that population is a v1.1 attestor task, NOT implemented in this
//! crate), iterates the [`ValidatorRegistry`], computes each validator's pro-rata share
//! using the weight formula `uptime × stake × votes`, and transfers SOL lamports from
//! the treasury PDA to each validator's identity address.
//!
//! Idempotency: marks `EpochAccrual.distributed = true` on success. Re-calling rejects
//! with `EpochAlreadyDistributed`. Off-chain tooling can therefore re-submit the
//! distribution ix freely without worrying about double-payments.
//!
//! Account passing convention: the caller passes the validators in `remaining_accounts`
//! in two slots per validator, matching the registry order:
//!
//! - even index `2k`:   `ValidatorRecord` PDA for `validators[k]` (writable)
//! - odd index `2k+1`:  validator identity address (writable, lamport recipient)
//!
//! The handler verifies each `ValidatorRecord` matches the registry entry at the same
//! index. Order is enforced; out-of-order passes reject with
//! `RemainingAccountsMismatch`.
//!
//! ## Treasury PDA
//!
//! The treasury PDA is `["treasury"]` owned by THIS program. SPEC §2.1 lists
//! `TREASURY_PROGRAM_ID = TBD`; this crate is the on-chain consumer of treasury
//! lamports for the validator-subsidy flow, so we own the PDA. Other treasury
//! operations (grant disbursements, AMM pool seeding) live in a future
//! `TreasuryOperations` ix set, not in this crate.

use crate::error::SubsidyError;
use crate::state::{EpochAccrual, SubsidyConfig, ValidatorRecord, ValidatorRegistry};
use crate::subsidy::{compute_validator_share, compute_validator_weight};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct DistributeYieldArgs {
    /// Epoch to distribute. The `EpochAccrual` PDA at `["accrual", epoch_le]` must
    /// already exist with `yield_observed` populated.
    pub epoch: u64,
}

#[derive(Accounts)]
#[instruction(args: DistributeYieldArgs)]
pub struct DistributeYield<'info> {
    /// Anyone can submit. Pays for any rent (none expected — `EpochAccrual` is pre-init).
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

    #[account(
        mut,
        seeds = [b"accrual", args.epoch.to_le_bytes().as_ref()],
        bump = epoch_accrual.bump,
        constraint = epoch_accrual.epoch == args.epoch @ SubsidyError::EpochMismatch,
    )]
    pub epoch_accrual: Account<'info, EpochAccrual>,

    /// Treasury PDA — lamport source. Owned by THIS program; signed for via
    /// `["treasury"]` seeds during the per-validator transfer.
    /// CHECK: PDA seeds bind. Has no Anchor account type — it's a plain SystemProgram
    /// account holding lamports.
    #[account(
        mut,
        seeds = [b"treasury"],
        bump,
    )]
    pub treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
    // remaining_accounts: 2 per validator (record + identity), in registry order.
}

/// Handler — compute total weight, then per-validator shares, then transfer.
///
/// Per-record reads / writes use manual `try_deserialize` / `try_serialize` rather than
/// `Account::try_from` because the latter requires `&'a AccountInfo<'a>` where both
/// lifetimes match — `ctx.remaining_accounts` gives us `&'c [AccountInfo<'info>]` with
/// `'c != 'info`, which doesn't satisfy `Account::try_from`'s signature. Manual
/// (de)serialization sidesteps the lifetime gymnastics without losing any semantics
/// (we still validate the discriminator via `try_deserialize`).
pub fn handler(
    ctx: Context<DistributeYield>,
    args: DistributeYieldArgs,
) -> Result<()> {
    let accrual_distributed = ctx.accounts.epoch_accrual.distributed;
    let yield_observed = ctx.accounts.epoch_accrual.yield_observed;

    require!(!accrual_distributed, SubsidyError::EpochAlreadyDistributed);
    require!(yield_observed > 0, SubsidyError::YieldNotPopulated);

    let registry_loader = &ctx.accounts.validator_registry;
    let registry = registry_loader.load()?;
    let registry_count = registry.count as usize;
    let expected_remaining = registry_count
        .checked_mul(2)
        .ok_or(SubsidyError::BadInstructionData)?;
    require!(
        ctx.remaining_accounts.len() == expected_remaining,
        SubsidyError::RemainingAccountsMismatch
    );

    // First pass: compute total weight by deserializing each ValidatorRecord.
    let mut total_weight: u128 = 0;
    let mut weights: Vec<u128> = Vec::with_capacity(registry_count);

    for k in 0..registry_count {
        let record_ai = &ctx.remaining_accounts[2 * k];
        let identity_ai = &ctx.remaining_accounts[2 * k + 1];

        // Verify each record's owner is this program (PDA must be owned by us) and
        // that the identity matches the registry entry at the same position.
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

    // Second pass: pay each validator and update their record totals.
    let treasury_ai = ctx.accounts.treasury.to_account_info();
    let mut distributed_total: u64 = 0;

    for k in 0..registry_count {
        let identity_ai = &ctx.remaining_accounts[2 * k + 1];
        let record_ai = &ctx.remaining_accounts[2 * k];

        let share = compute_validator_share(yield_observed, weights[k], total_weight)?;
        if share == 0 {
            continue;
        }

        let treasury_lamports = treasury_ai.lamports();
        require!(
            treasury_lamports >= share,
            SubsidyError::InsufficientTreasuryBalance
        );

        // Direct lamport-shuffle. Permitted because both accounts are writable and
        // owned by accounts the program has access to (treasury is owned by THIS
        // program; the identity address is system-owned). System-owned accounts
        // accept incoming lamports from any caller; the program-owned source needs
        // no signer because the runtime trusts a program to mutate lamports of
        // accounts it owns.
        **treasury_ai.try_borrow_mut_lamports()? = treasury_lamports
            .checked_sub(share)
            .ok_or(SubsidyError::InsufficientTreasuryBalance)?;
        **identity_ai.try_borrow_mut_lamports()? = identity_ai
            .lamports()
            .checked_add(share)
            .ok_or(SubsidyError::ShareOverflow)?;

        // Re-load the record (still cheap — the data lives in the account info we
        // already hold), bump the lifetime totals, write back.
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

    let accrual = &mut ctx.accounts.epoch_accrual;
    accrual.distributed = true;
    accrual.total_weight = total_weight;
    accrual.distributed_total = distributed_total;

    let cfg = &mut ctx.accounts.subsidy_config;
    cfg.last_distributed_epoch = args.epoch;

    emit!(DistributeYieldEvent {
        epoch: args.epoch,
        yield_observed,
        total_weight,
        distributed_total,
        validator_count: registry_count as u32,
    });

    Ok(())
}


/// Emitted on successful yield distribution. Off-chain indexers consume this to
/// reconstruct per-epoch payouts without re-deriving weights.
#[event]
pub struct DistributeYieldEvent {
    pub epoch: u64,
    pub yield_observed: u64,
    pub total_weight: u128,
    pub distributed_total: u64,
    pub validator_count: u32,
}

//! `register_validator` — governance adds a validator to the registry.
//!
//! v1 has no self-registration: the governance multisig is the only path. Initializes a
//! [`ValidatorRecord`] PDA at `["validator", validator_pubkey]` with all metrics zeroed,
//! and appends the pubkey to the [`ValidatorRegistry`].
//!
//! Idempotency: re-registering the same pubkey rejects with `ValidatorAlreadyRegistered`
//! (Anchor's `init` constraint catches the duplicate PDA).

use crate::error::SubsidyError;
use crate::state::{SubsidyConfig, ValidatorRecord, ValidatorRegistry, MAX_VALIDATORS};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct RegisterValidatorArgs {
    /// Validator identity address (Solana vote account's identity, NOT the vote
    /// account itself). Distributions land on this address.
    pub validator: Pubkey,
}

#[derive(Accounts)]
#[instruction(args: RegisterValidatorArgs)]
pub struct RegisterValidator<'info> {
    /// Must equal `subsidy_config.governance`. Pays for the new `ValidatorRecord` PDA.
    #[account(
        mut,
        constraint = authority.key() == subsidy_config.governance
            @ SubsidyError::BadInstructionData,
    )]
    pub authority: Signer<'info>,

    #[account(
        seeds = [b"subsidy_config"],
        bump = subsidy_config.bump,
    )]
    pub subsidy_config: Account<'info, SubsidyConfig>,

    /// `realloc` here grows the existing on-chain registry to the current
    /// `ValidatorRegistry::SPACE`. Was needed once when MAX_VALIDATORS jumped
    /// from 8 → 256 — a no-op on subsequent calls (Anchor short-circuits
    /// when `data.len() == new_space`). `realloc::zero = true` zero-fills
    /// the newly-allocated tail so any stale bytes from the old layout
    /// (e.g. the cached `bump` byte that previously sat at offset 268) are
    /// scrubbed.
    #[account(
        mut,
        seeds = [b"validator_registry"],
        bump,
        realloc = ValidatorRegistry::SPACE,
        realloc::payer = authority,
        realloc::zero = true,
    )]
    pub validator_registry: AccountLoader<'info, ValidatorRegistry>,

    #[account(
        init,
        payer = authority,
        space = ValidatorRecord::SPACE,
        seeds = [b"validator", args.validator.as_ref()],
        bump,
    )]
    pub validator_record: Account<'info, ValidatorRecord>,

    pub system_program: Program<'info, System>,
}

/// Handler — append to the registry, init the record with zero metrics.
pub fn handler(ctx: Context<RegisterValidator>, args: RegisterValidatorArgs) -> Result<()> {
    let mut reg = ctx.accounts.validator_registry.load_mut()?;
    require!(
        (reg.count as usize) < MAX_VALIDATORS,
        SubsidyError::ValidatorRegistryFull
    );

    // Defense-in-depth: scan for an existing entry. The `init` constraint on the
    // ValidatorRecord PDA already catches the on-chain duplicate, but checking the
    // registry contents explicitly catches the (unlikely) race where the registry
    // is somehow out of sync.
    let count = reg.count as usize;
    for i in 0..count {
        if reg.validators[i] == args.validator {
            return Err(SubsidyError::ValidatorAlreadyRegistered.into());
        }
    }

    reg.validators[count] = args.validator;
    reg.count = (count as u32)
        .checked_add(1)
        .ok_or(SubsidyError::BadInstructionData)?;

    let rec = &mut ctx.accounts.validator_record;
    rec.validator = args.validator;
    rec.uptime_bps = 0;
    rec.delegated_stake = 0;
    rec.votes_cast = 0;
    rec.last_metrics_slot = 0;
    rec.last_metrics_nonce = 0;
    rec.last_distribution_epoch = 0;
    rec.total_subsidy_received = 0;
    rec.bump = ctx.bumps.validator_record;

    Ok(())
}

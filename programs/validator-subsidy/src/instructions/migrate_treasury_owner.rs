//! `migrate_treasury_owner` — one-shot ix to fix the genesis-bake treasury
//! custody bug.
//!
//! Background
//! ----------
//!
//! `tools/genesis-bake/src/accounts.rs::treasury_account` intentionally set
//! the treasury PDA's `owner` field to `LAZY_CLAIM_PROGRAM_ID`
//! (= `68fnSf8CZjxLM2xHmswktgz3a77KLQT2nbhjWbpKWsYU`, the ASCII bytes of
//! "LAZY_CLAIM_PROGRAM_PLACEHOLDER11") so that `lazy_claim::credit_lamports`
//! could `try_borrow_mut_lamports` directly on the treasury during a claim.
//! The accompanying source comment explicitly notes:
//!
//!   "Subsidy disbursement path is broken until that wiring lands."
//!
//! This ix is the wiring. It fires `system_program::assign(treasury, self)`
//! with the treasury PDA seed-signed by THIS program — flipping ownership
//! to the validator-subsidy program. After this lands:
//!
//!   - `distribute_yield`'s `try_borrow_mut_lamports` works (it calls into
//!     `subsidy::pay_validator_share` which assumes self-ownership).
//!   - `bootstrap_distribute` likewise.
//!   - any future `delegate_treasury_stake` (when native stake CPI is
//!     re-enabled) can fund from treasury.
//!
//! Runtime sanity
//! --------------
//!
//! `system_program::assign` does NOT require the account to currently be
//! system-owned. Per `agave-3.1.14 programs/system/src/system_processor.rs::assign`
//! it only checks the account is a signer. PDAs sign via their deriving
//! program — that's us. So this works even though treasury is currently
//! owned by `LAZY_CLAIM_PLACEHOLDER`.
//!
//! Authorization
//! -------------
//!
//! Gated on `subsidy_config.governance` (same gate as `register_validator`,
//! etc.). This is a privileged migration; once executed, treasury is
//! forever owned by validator-subsidy and lazy-claim's `credit_lamports`
//! direct-debit path is dead. That path is unused on this chain anyway —
//! lazy-claim hasn't been part of the live disbursement flow.
//!
//! Idempotence: Anchor's `address = …` constraint on `treasury` lets this
//! run multiple times safely. `system_program::assign` no-ops when the
//! current owner equals the requested new owner (per agave's `if account.get_owner() == owner { return Ok(()) }`).

use crate::error::SubsidyError;
use crate::state::SubsidyConfig;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;

#[derive(Accounts)]
pub struct MigrateTreasuryOwner<'info> {
    /// Governance signer. Must equal `subsidy_config.governance`.
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

    /// Treasury PDA. Currently owned by `LAZY_CLAIM_PLACEHOLDER`; this ix
    /// flips it to the validator-subsidy program ID.
    /// CHECK: PDA derivation enforced by Anchor seeds.
    #[account(
        mut,
        seeds = [b"treasury"],
        bump,
    )]
    pub treasury: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<MigrateTreasuryOwner>) -> Result<()> {
    let treasury_bump = ctx.bumps.treasury;
    let treasury_seeds: &[&[u8]] = &[b"treasury", core::slice::from_ref(&treasury_bump)];

    let assign_ix = system_instruction::assign(&ctx.accounts.treasury.key(), &crate::ID);

    invoke_signed(
        &assign_ix,
        &[
            ctx.accounts.treasury.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
        &[treasury_seeds],
    )?;

    msg!(
        "[migrate_treasury_owner] treasury {} now owned by {}",
        ctx.accounts.treasury.key(),
        crate::ID
    );

    Ok(())
}

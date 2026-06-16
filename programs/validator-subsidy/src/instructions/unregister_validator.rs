//! `unregister_validator` — governance removes a validator from the registry.
//!
//! Symmetric counterpart to [`register_validator`]: takes the same identity
//! pubkey, removes it from `ValidatorRegistry.validators[..count]` (swap-with-
//! last + decrement count), and closes the per-validator [`ValidatorRecord`]
//! PDA, refunding its rent to the governance authority.
//!
//! Idempotency: removing a non-registered pubkey rejects with
//! `ValidatorNotRegistered`. (The Anchor `close` constraint also catches it
//! at the account level — the seeds-derived PDA won't have the right
//! discriminator.)
//!
//! Authorization: signer must equal `subsidy_config.governance`. Same gate
//! as `register_validator` / `stake_to_productive` / `unstake_from_productive`.

use crate::error::SubsidyError;
use crate::state::{SubsidyConfig, ValidatorRecord, ValidatorRegistry};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct UnregisterValidatorArgs {
    /// Validator identity address to remove. Must currently be in the registry.
    pub validator: Pubkey,
}

#[derive(Accounts)]
#[instruction(args: UnregisterValidatorArgs)]
pub struct UnregisterValidator<'info> {
    /// Governance signer + rent recipient (lamports from the closed
    /// `ValidatorRecord` land here).
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

    #[account(
        mut,
        seeds = [b"validator_registry"],
        bump,
    )]
    pub validator_registry: AccountLoader<'info, ValidatorRegistry>,

    /// The per-validator record, closed and rent-refunded to `authority`.
    /// Anchor's `close` constraint zeros the discriminator so the PDA can be
    /// re-`init`'d by a future `register_validator` call (`init` is one-shot
    /// per address but `init_if_needed` would also work).
    #[account(
        mut,
        close = authority,
        seeds = [b"validator", args.validator.as_ref()],
        bump = validator_record.bump,
        constraint = validator_record.validator == args.validator
            @ SubsidyError::ValidatorNotRegistered,
    )]
    pub validator_record: Account<'info, ValidatorRecord>,
}

/// Handler — find the slot, swap-with-last, decrement count, zero the freed
/// slot. Closing is handled by Anchor's `close` constraint above.
pub fn handler(
    ctx: Context<UnregisterValidator>,
    args: UnregisterValidatorArgs,
) -> Result<()> {
    let mut reg = ctx.accounts.validator_registry.load_mut()?;
    let count = reg.count as usize;
    let mut found_idx: Option<usize> = None;
    for i in 0..count {
        if reg.validators[i] == args.validator {
            found_idx = Some(i);
            break;
        }
    }
    let idx = found_idx.ok_or(SubsidyError::ValidatorNotRegistered)?;

    // Swap-with-last keeps the live slice contiguous in O(1). Order isn't
    // semantically meaningful — `distribute_yield` iterates by index but
    // each validator's per-epoch payout is independent of position.
    let last = count - 1;
    if idx != last {
        reg.validators[idx] = reg.validators[last];
    }
    reg.validators[last] = Pubkey::default();
    reg.count = (count - 1) as u32;

    Ok(())
}

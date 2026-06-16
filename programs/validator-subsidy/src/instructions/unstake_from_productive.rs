//! `unstake_from_productive` — governance-gated CPI into the bridge `burn` ix.
//!
//! Inverse of `stake_to_productive`: burns `args.amount` of the productive-position
//! wrapper token (e.g. pSYRUP) from a treasury-owned ATA. The bridge emits a `Burn`
//! event for the federation to relay to the mainnet vault, which releases the
//! corresponding underlying SOL.
//!
//! Authorization: the `authority` signer must equal `subsidy_config.governance`.
//!
//! Accounting: decrements `subsidy_config.productive_deposit_total` by
//! `args.amount`. Note that the wrapper-token amount is what's tracked here, not the
//! underlying lamports value released; off-chain accounting must consult the bridge's
//! current R to reconstruct the SOL value.
//!
//! See `stake_to_productive.rs` for the rationale on hand-rolling the CPI rather than
//! relying on `staccana_bridge::cpi::*` typed helpers.

use crate::error::SubsidyError;
use crate::state::SubsidyConfig;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
// `InstructionData::data()` packs `discriminator || borsh(args)` into wire bytes.
// Not in the Anchor prelude as of 0.30, so import explicitly.
use anchor_lang::InstructionData;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct UnstakeFromProductiveArgs {
    /// Amount of wrapper tokens (e.g. pSYRUP) to burn.
    pub amount: u64,

    /// Mainnet destination address (32 bytes). The bridge re-emits this in the `Burn`
    /// event for the federation to forward to the mainnet vault.
    pub mainnet_dest: [u8; 32],
}

#[derive(Accounts)]
pub struct UnstakeFromProductive<'info> {
    /// Must equal `subsidy_config.governance`. Signs the bridge `burn` ix as the user
    /// authority over the treasury-owned ATA.
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

    /// Bridge program. Must equal `subsidy_config.bridge_program_id`.
    /// CHECK: address-verified in the handler against the stored bridge program id.
    pub bridge_program: AccountInfo<'info>,

    /// CHECK: validated by the bridge's `burn` handler.
    pub bridge_asset_config: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `burn` handler.
    pub bridge_ratio_state: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `burn` handler.
    #[account(mut)]
    pub bridge_staccana_mint: AccountInfo<'info>,
    /// Treasury-owned ATA holding the wrapper tokens to burn.
    /// CHECK: validated by the bridge's `burn` handler (token::mint and token::authority).
    #[account(mut)]
    pub treasury_user_ata: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `burn` handler.
    #[account(mut)]
    pub bridge_nonce_out: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `burn` handler.
    pub bridge_token_program: AccountInfo<'info>,
}

/// Handler — sanity-check args, build the bridge `burn` ix, CPI it.
pub fn handler(
    ctx: Context<UnstakeFromProductive>,
    args: UnstakeFromProductiveArgs,
) -> Result<()> {
    require!(args.amount > 0, SubsidyError::ZeroUnstakeAmount);

    // Snapshot the immutable fields off the config so the mutable re-borrow at the
    // bottom of this function doesn't fight the earlier read.
    let bridge_program_id = ctx.accounts.subsidy_config.bridge_program_id;
    let productive_asset_id = ctx.accounts.subsidy_config.productive_asset_id;
    require_keys_eq!(
        ctx.accounts.bridge_program.key(),
        bridge_program_id,
        SubsidyError::BadInstructionData
    );

    // Construct the bridge's `Burn` ix data via Anchor's generated wrapper struct.
    // Same rationale as `stake_to_productive`: rely on the canonical Anchor codegen path
    // for ix data so any future bridge-side ix-arg evolution stays binary-compatible
    // here without manual byte tracking.
    let burn_args = staccana_bridge::instructions::burn::BurnArgs {
        asset_id: productive_asset_id,
        amount: args.amount,
        mainnet_dest: args.mainnet_dest,
    };
    let ix_struct = staccana_bridge::instruction::Burn { args: burn_args };
    let data = ix_struct.data();

    // Account list MUST match the bridge `BridgeBurn` account context order:
    //   user, asset_config, ratio_state, staccana_mint, user_ata, nonce_out,
    //   token_program.
    let accounts = vec![
        AccountMeta::new_readonly(ctx.accounts.authority.key(), true),
        AccountMeta::new_readonly(ctx.accounts.bridge_asset_config.key(), false),
        AccountMeta::new_readonly(ctx.accounts.bridge_ratio_state.key(), false),
        AccountMeta::new(ctx.accounts.bridge_staccana_mint.key(), false),
        AccountMeta::new(ctx.accounts.treasury_user_ata.key(), false),
        AccountMeta::new(ctx.accounts.bridge_nonce_out.key(), false),
        AccountMeta::new_readonly(ctx.accounts.bridge_token_program.key(), false),
    ];

    let ix = Instruction {
        program_id: ctx.accounts.bridge_program.key(),
        accounts,
        data,
    };

    let account_infos = [
        ctx.accounts.authority.to_account_info(),
        ctx.accounts.bridge_asset_config.to_account_info(),
        ctx.accounts.bridge_ratio_state.to_account_info(),
        ctx.accounts.bridge_staccana_mint.to_account_info(),
        ctx.accounts.treasury_user_ata.to_account_info(),
        ctx.accounts.bridge_nonce_out.to_account_info(),
        ctx.accounts.bridge_token_program.to_account_info(),
    ];

    // Bridge-side errors propagate through the `?`.
    invoke_signed(&ix, &account_infos, &[])?;

    let cfg = &mut ctx.accounts.subsidy_config;
    cfg.productive_deposit_total = cfg
        .productive_deposit_total
        .saturating_sub(args.amount);

    Ok(())
}

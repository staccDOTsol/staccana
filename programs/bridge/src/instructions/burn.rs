//! `burn` — user burns wrapper tokens to redeem underlying on the mainnet vault.
//!
//! On-chain effects (SPEC.md §5.5):
//! 1. Read `R_q64`, compute `release_amount = (amount * R_q64) >> 64`.
//! 2. Apply staccana-side `burn_fee_bps` to the release amount.
//! 3. CPI into Token-22 to burn `amount` from the user's ATA.
//! 4. Increment the per-asset outbound nonce counter.
//! 5. Emit `Burn` event for the federation to relay to the mainnet vault.
//!
//! There is no federation signature on the burn side — burning is the user's own action.
//! The federation observes the emitted event and produces a release attestation for the
//! mainnet vault to consume; that attestation is verified mainnet-side and is out of
//! scope for this crate.

use crate::attestation::{apply_bps_fee, release_amount_for_burn};
use crate::error::BridgeError;
use crate::state::{AssetConfig, NonceOutCounter, RatioState};
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    self, Burn, Mint, TokenAccount, TokenInterface,
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct BurnArgs {
    pub asset_id: u32,
    /// Number of mint tokens to burn from the user's ATA.
    pub amount: u64,
    /// Mainnet pubkey to credit on the vault side. Opaque to the staccana program;
    /// re-emitted in the `Burn` event for the federation to forward.
    pub mainnet_dest: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: BurnArgs)]
pub struct BridgeBurn<'info> {
    /// Owner of the ATA being burned from. Must sign — this is the only authorization
    /// required (no federation involvement on the burn-out side).
    pub user: Signer<'info>,

    #[account(
        seeds = [b"asset", args.asset_id.to_le_bytes().as_ref()],
        bump = asset_config.bump,
    )]
    pub asset_config: Account<'info, AssetConfig>,

    #[account(
        seeds = [b"ratio", args.asset_id.to_le_bytes().as_ref()],
        bump = ratio_state.bump,
    )]
    pub ratio_state: Account<'info, RatioState>,

    #[account(
        mut,
        address = asset_config.staccana_mint,
    )]
    pub staccana_mint: InterfaceAccount<'info, Mint>,

    /// User ATA being burned from. Token-22 enforces `user_ata.owner == user.key()`.
    #[account(
        mut,
        token::mint = staccana_mint,
        token::authority = user,
    )]
    pub user_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"nonce_out", args.asset_id.to_le_bytes().as_ref()],
        bump = nonce_out.bump,
    )]
    pub nonce_out: Account<'info, NonceOutCounter>,

    /// Token program — `Interface<TokenInterface>` accepts either SPL Token or
    /// Token-2022. The asset's mint pins which one is actually used.
    pub token_program: Interface<'info, TokenInterface>,
}

/// Handler — compute release amount, burn mint tokens, increment nonce, emit event.
pub fn handler(ctx: Context<BridgeBurn>, args: BurnArgs) -> Result<()> {
    require!(args.amount > 0, BridgeError::ZeroBurnAmount);

    let cfg = &ctx.accounts.asset_config;
    let ratio = &ctx.accounts.ratio_state;

    // Release amount = amount * R, then deduct burn fee. Note the order: fee is on the
    // released underlying, not on the mint amount. This matches the typical LST
    // redemption-fee model.
    let gross_release = release_amount_for_burn(args.amount, ratio.r_q64)?;
    let net_release = apply_bps_fee(gross_release, cfg.burn_fee_bps);

    // CPI into the token program to burn from the user's ATA. Authority is the user
    // signer; no PDA seeds needed.
    //
    // Anchor 1.0: `CpiContext::new` takes the program id (`Pubkey`) instead of an
    // `AccountInfo`.
    let cpi_ctx = CpiContext::new(
        ctx.accounts.token_program.key(),
        Burn {
            mint: ctx.accounts.staccana_mint.to_account_info(),
            from: ctx.accounts.user_ata.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        },
    );
    token_interface::burn(cpi_ctx, args.amount)?;

    // Allocate the next outbound nonce. Stored as `next_nonce`; we read-and-increment
    // so the emitted event carries the value just allocated.
    let nonce_out = &mut ctx.accounts.nonce_out;
    let assigned_nonce = nonce_out.next_nonce;
    nonce_out.next_nonce = assigned_nonce
        .checked_add(1)
        .ok_or(BridgeError::BadInstructionData)?;

    emit!(BurnEvent {
        asset_id: args.asset_id,
        user: ctx.accounts.user.key(),
        amount: args.amount,
        gross_release,
        net_release,
        r_q64: ratio.r_q64,
        mainnet_dest: args.mainnet_dest,
        nonce_out: assigned_nonce,
        chain_id: CHAIN_ID_MAINNET,
    });

    Ok(())
}

/// `chain_id` discriminator embedded in burn events so the mainnet-side relayer can
/// distinguish staccana → mainnet attestations from any future flow. Opaque value;
/// just must be globally unique across (staccana, mainnet, future-chain).
pub const CHAIN_ID_MAINNET: u32 = 0x6D61_696E; // ASCII "main"

/// Emitted on every successful burn. The federation watches for these and produces a
/// signed release attestation for the mainnet vault to consume.
#[event]
pub struct BurnEvent {
    pub asset_id: u32,
    pub user: Pubkey,
    pub amount: u64,
    pub gross_release: u64,
    pub net_release: u64,
    pub r_q64: u128,
    pub mainnet_dest: [u8; 32],
    pub nonce_out: u64,
    pub chain_id: u32,
}

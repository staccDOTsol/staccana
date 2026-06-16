//! `sell` instruction: swap the curve's token for SOL along the constant-product curve.
//!
//! Flow:
//!
//! 1. Quote the trade via [`crate::curve::quote_sell`]. Slippage check trips here.
//! 2. CPI Token-22 to transfer `tokens_in` from `seller_token_account` → `curve_vault`,
//!    signed by the seller.
//! 3. Move `sol_to_seller` lamports from curve PDA → seller via direct lamport mutation
//!    (`try_borrow_mut_lamports`). System CPI cannot move lamports out of a program-owned
//!    data account; we mutate the balances directly instead.
//! 4. Move `sol_fee` lamports from curve PDA → treasury PDA the same way.
//! 5. Update curve state, emit [`crate::state::SellEvent`].
//!
//! Direct lamport manipulation is the canonical pattern for SOL outflow from a program-
//! owned PDA. See <https://solana.com/docs/programs/lamports#transferring-sol-from-pda>.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    self, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::curve::quote_sell;
use crate::error::SecretPumpError;
use crate::state::{BondingCurve, SellEvent};
use crate::TREASURY_PUBKEY_PLACEHOLDER;

#[derive(Accounts)]
pub struct Sell<'info> {
    pub mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        seeds = [BondingCurve::SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.mint == mint.key() @ SecretPumpError::BondingCurveMintMismatch,
    )]
    pub bonding_curve: Account<'info, BondingCurve>,

    #[account(
        mut,
        seeds = [BondingCurve::VAULT_SEED, mint.key().as_ref()],
        bump = bonding_curve.vault_bump,
        constraint = curve_vault.mint == mint.key() @ SecretPumpError::BondingCurveMintMismatch,
        constraint = curve_vault.owner == bonding_curve.key() @ SecretPumpError::BadCurveVault,
    )]
    pub curve_vault: InterfaceAccount<'info, TokenAccount>,

    /// Seller's source token account.
    #[account(
        mut,
        constraint = seller_token_account.mint == mint.key() @ SecretPumpError::BondingCurveMintMismatch,
    )]
    pub seller_token_account: InterfaceAccount<'info, TokenAccount>,

    /// Seller giving up tokens, receiving SOL. Signs the Token-22 transfer authority.
    #[account(mut)]
    pub seller: Signer<'info>,

    #[account(mut, address = TREASURY_PUBKEY_PLACEHOLDER @ SecretPumpError::BadTreasuryAccount)]
    /// CHECK: lamport-only destination; address constraint enforces identity.
    pub treasury: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

pub fn handler(ctx: Context<Sell>, tokens_in: u64, min_sol_out: u64) -> Result<()> {
    let mint_key = ctx.accounts.mint.key();

    // 1. Quote the trade.
    let reserves = ctx.accounts.bonding_curve.reserves();
    let graduated = ctx.accounts.bonding_curve.graduated;
    let quote = quote_sell(reserves, tokens_in, min_sol_out, graduated)
        .map_err(SecretPumpError::from)?;

    // 2. CPI Token-22 transfer_checked: seller → curve_vault, signed by seller.
    let cpi_accounts = TransferChecked {
        from: ctx.accounts.seller_token_account.to_account_info(),
        mint: ctx.accounts.mint.to_account_info(),
        to: ctx.accounts.curve_vault.to_account_info(),
        authority: ctx.accounts.seller.to_account_info(),
    };
    // Anchor 1.0: `CpiContext::new` takes the program id (`Pubkey`) instead of an
    // `AccountInfo`.
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.key(), cpi_accounts);
    token_interface::transfer_checked(cpi_ctx, tokens_in, ctx.accounts.mint.decimals)?;

    // 3. Move sol_to_seller from curve PDA → seller via direct lamport mutation.
    //    System CPI is illegal from a program-owned data account; we mutate balances directly.
    //
    // The borrow checker on the new toolchain rejects calling `try_borrow_mut_lamports`
    // directly on a temporary `to_account_info()` value — bind the AccountInfo to a local
    // first so the RefCell guard outlives the inner borrow.
    {
        let curve_ai = ctx.accounts.bonding_curve.to_account_info();
        let seller_ai = ctx.accounts.seller.to_account_info();
        let mut curve_lamports = curve_ai.try_borrow_mut_lamports()?;
        let mut seller_lamports = seller_ai.try_borrow_mut_lamports()?;
        **curve_lamports = curve_lamports
            .checked_sub(quote.sol_to_seller)
            .ok_or(SecretPumpError::InsufficientReserves)?;
        **seller_lamports = seller_lamports
            .checked_add(quote.sol_to_seller)
            .ok_or(SecretPumpError::Overflow)?;
    }

    // 4. Move sol_fee from curve PDA → treasury PDA.
    if quote.sol_fee > 0 {
        let curve_ai = ctx.accounts.bonding_curve.to_account_info();
        let mut curve_lamports = curve_ai.try_borrow_mut_lamports()?;
        let mut treasury_lamports = ctx.accounts.treasury.try_borrow_mut_lamports()?;
        **curve_lamports = curve_lamports
            .checked_sub(quote.sol_fee)
            .ok_or(SecretPumpError::InsufficientReserves)?;
        **treasury_lamports = treasury_lamports
            .checked_add(quote.sol_fee)
            .ok_or(SecretPumpError::Overflow)?;
    }

    // 5. Persist updated state.
    let curve = &mut ctx.accounts.bonding_curve;
    curve.apply_reserves(quote.new_reserves);
    curve.total_fees_collected = curve.total_fees_collected.saturating_add(quote.sol_fee);

    emit!(SellEvent {
        mint: mint_key,
        seller: ctx.accounts.seller.key(),
        tokens_in,
        sol_out_gross: quote.sol_out_gross,
        sol_fee: quote.sol_fee,
        sol_to_seller: quote.sol_to_seller,
        real_sol_reserves: curve.real_sol_reserves,
        real_token_reserves: curve.real_token_reserves,
    });

    Ok(())
}

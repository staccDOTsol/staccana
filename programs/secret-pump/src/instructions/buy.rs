//! `buy` instruction: swap SOL for the curve's token along the constant-product curve.
//!
//! Flow:
//!
//! 1. Quote the trade via [`crate::curve::quote_buy`]. Slippage check trips here.
//! 2. Move `sol_into_curve` lamports from buyer → curve PDA via System CPI.
//! 3. Move `sol_fee` lamports from buyer → treasury PDA via System CPI.
//! 4. CPI Token-22 to transfer `tokens_out` from `curve_vault` → `buyer_token_account`.
//! 5. Update curve state, emit [`crate::state::BuyEvent`], emit
//!    [`crate::state::GraduationEvent`] if the trade graduates the curve.
//!
//! The Token-22 transfer uses the **public-amount** path (`transfer_checked`), not the
//! confidential transfer ix: the curve cannot operate confidentially because it must
//! know the trade size to update its plaintext reserves. **What's confidential is the
//! buyer's downstream movement of those tokens** between confidential accounts; the
//! purchase from the curve is necessarily revealed.

use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::token_interface::{
    self, Mint, TokenAccount, TokenInterface, TransferChecked,
};

use crate::curve::quote_buy;
use crate::error::SecretPumpError;
use crate::state::{BondingCurve, BuyEvent, GraduationEvent};
use crate::TREASURY_PUBKEY_PLACEHOLDER;

#[derive(Accounts)]
pub struct Buy<'info> {
    /// The token mint. Used as a curve-PDA seed and the transfer's mint anchor (Token-22
    /// `transfer_checked` requires the mint as an account argument).
    pub mint: InterfaceAccount<'info, Mint>,

    /// Curve state PDA.
    #[account(
        mut,
        seeds = [BondingCurve::SEED, mint.key().as_ref()],
        bump = bonding_curve.bump,
        constraint = bonding_curve.mint == mint.key() @ SecretPumpError::BondingCurveMintMismatch,
    )]
    pub bonding_curve: Account<'info, BondingCurve>,

    /// Curve's token vault. Authority = bonding_curve PDA.
    #[account(
        mut,
        seeds = [BondingCurve::VAULT_SEED, mint.key().as_ref()],
        bump = bonding_curve.vault_bump,
        constraint = curve_vault.mint == mint.key() @ SecretPumpError::BondingCurveMintMismatch,
        constraint = curve_vault.owner == bonding_curve.key() @ SecretPumpError::BadCurveVault,
    )]
    pub curve_vault: InterfaceAccount<'info, TokenAccount>,

    /// Buyer's destination token account. Caller is responsible for ensuring this is a
    /// valid Token-22 account (with the Confidential Transfer extension active if they
    /// want to retain confidentiality on subsequent transfers).
    #[account(
        mut,
        constraint = buyer_token_account.mint == mint.key() @ SecretPumpError::BondingCurveMintMismatch,
    )]
    pub buyer_token_account: InterfaceAccount<'info, TokenAccount>,

    /// Buyer paying SOL and receiving tokens.
    #[account(mut)]
    pub buyer: Signer<'info>,

    /// Staccana treasury PDA. Address validated against [`TREASURY_PUBKEY_PLACEHOLDER`]
    /// for v0; production wires the real treasury PDA derivation.
    #[account(mut, address = TREASURY_PUBKEY_PLACEHOLDER @ SecretPumpError::BadTreasuryAccount)]
    /// CHECK: lamport-only destination; address constraint enforces identity.
    pub treasury: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<Buy>, sol_in: u64, min_tokens_out: u64) -> Result<()> {
    let mint_key = ctx.accounts.mint.key();
    let curve_bump = ctx.accounts.bonding_curve.bump;

    // 1. Quote the trade and verify slippage / non-graduation.
    let reserves = ctx.accounts.bonding_curve.reserves();
    let graduated = ctx.accounts.bonding_curve.graduated;
    let quote = quote_buy(reserves, sol_in, min_tokens_out, graduated)
        .map_err(SecretPumpError::from)?;

    // 2. Move sol_into_curve from buyer → curve PDA.
    //
    // Anchor 1.0: `CpiContext::new` takes the program id (`Pubkey`) instead of an
    // `AccountInfo`.
    if quote.sol_into_curve > 0 {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.buyer.to_account_info(),
                    to: ctx.accounts.bonding_curve.to_account_info(),
                },
            ),
            quote.sol_into_curve,
        )?;
    }

    // 3. Move sol_fee from buyer → treasury PDA.
    if quote.sol_fee > 0 {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.buyer.to_account_info(),
                    to: ctx.accounts.treasury.to_account_info(),
                },
            ),
            quote.sol_fee,
        )?;
    }

    // 4. CPI Token-22 transfer_checked: curve_vault → buyer_token_account, signed by curve PDA.
    let bump_arr = [curve_bump];
    let signer_seeds_owned = BondingCurve::signer_seeds(&mint_key, &bump_arr);
    let signer_seeds: &[&[&[u8]]] = &[&signer_seeds_owned];

    let cpi_accounts = TransferChecked {
        from: ctx.accounts.curve_vault.to_account_info(),
        mint: ctx.accounts.mint.to_account_info(),
        to: ctx.accounts.buyer_token_account.to_account_info(),
        authority: ctx.accounts.bonding_curve.to_account_info(),
    };
    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.key(),
        cpi_accounts,
        signer_seeds,
    );
    token_interface::transfer_checked(cpi_ctx, quote.tokens_out, ctx.accounts.mint.decimals)?;

    // 5. Persist updated state.
    let curve = &mut ctx.accounts.bonding_curve;
    curve.apply_reserves(quote.new_reserves);
    curve.total_tokens_dispensed = curve
        .total_tokens_dispensed
        .saturating_add(quote.tokens_out);
    curve.total_fees_collected = curve.total_fees_collected.saturating_add(quote.sol_fee);

    // 6. Latch graduation if crossed for the first time. The flag prevents subsequent
    //    trades from re-emitting the event.
    let just_graduated = quote.graduates && !curve.graduated;
    if just_graduated {
        curve.graduated = true;
        curve.graduation_slot = Clock::get()?.slot;
        emit!(GraduationEvent {
            mint: mint_key,
            real_sol_reserves: curve.real_sol_reserves,
            real_token_reserves: curve.real_token_reserves,
            slot: curve.graduation_slot,
        });
    }

    emit!(BuyEvent {
        mint: mint_key,
        buyer: ctx.accounts.buyer.key(),
        sol_in,
        sol_fee: quote.sol_fee,
        tokens_out: quote.tokens_out,
        real_sol_reserves: curve.real_sol_reserves,
        real_token_reserves: curve.real_token_reserves,
        graduated: curve.graduated,
    });

    Ok(())
}

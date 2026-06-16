//! `create` instruction: spin up a new bonding curve over a pre-initialized Token-22 mint.
//!
//! ## Contract change (BREAKING)
//!
//! Previously this instruction created the Token-22 mint inline, took caller-supplied
//! `name`/`symbol`/`uri` as fixed-byte arrays (32 / 10 / 200) and *discarded* them — the
//! 200-byte URI cap meant that any inline-image metadata blob (≈90KB+ once the image was
//! base64-encoded) would overflow the on-chain arg before the tx ever reached the runtime.
//!
//! The frontend now creates the mint with the Token-22 **MetadataPointer + TokenMetadata**
//! extensions in earlier instructions of the same transaction, pointing the metadata at the
//! mint itself. This handler now consumes a *pre-initialized* mint and only:
//!
//! 1. Validates that mint authority == curve PDA and decimals == 9.
//! 2. Allocates the [`crate::state::BondingCurve`] PDA.
//! 3. Allocates + initializes the vault token account (PDA, owner = curve PDA).
//! 4. Mints the full virtual token allocation into the vault.
//!
//! The mint is no longer initialized here — the caller supplies an already-initialized
//! Token-22 mint (signed by its keypair, mint_authority = curve PDA, freeze_authority unset).
//! Existing curves created against the old contract are NOT migrated; their PDA layout is
//! identical (the URI was never persisted on the curve PDA), so reads continue to work,
//! but new launches MUST use the new client flow.

#![allow(deprecated)]

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::system_program;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{self, MintTo};
use spl_token_2022::extension::StateWithExtensions;
use spl_token_2022::instruction as token_2022_ix;
use spl_token_2022::state::Mint as Token22Mint;

use crate::curve::{VIRTUAL_SOL, VIRTUAL_TOKENS};
use crate::error::SecretPumpError;
use crate::state::{BondingCurve, CurveCreatedEvent};

/// Caller-supplied args for `create`. The old `name`/`symbol`/`uri` fixed-byte fields are
/// gone — the frontend embeds metadata on the mint via Token-22's MetadataPointer +
/// TokenMetadata extensions before invoking this ix, so the on-chain handler does not
/// need (or accept) any metadata payload.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct CreateArgs {}

#[derive(Accounts)]
pub struct CreateCurve<'info> {
    /// Pre-initialized Token-22 mint. The caller is responsible for creating + initializing
    /// this mint (with MetadataPointer + TokenMetadata + ConfidentialTransfer extensions and
    /// `mint_authority = curve PDA`) in earlier instructions of the same transaction. The
    /// keypair must still sign the tx because Anchor's `init` of the curve PDA below uses
    /// the mint key as a seed and mints to the vault require the mint to be writable.
    #[account(mut, signer)]
    /// CHECK: validated in handler — owner must be Token-22, mint_authority must be the
    /// curve PDA, decimals must be 9.
    pub mint: AccountInfo<'info>,

    /// Per-mint bonding curve PDA. Owns the mint authority and the vault.
    #[account(
        init,
        payer = creator,
        space = BondingCurve::SPACE,
        seeds = [BondingCurve::SEED, mint.key().as_ref()],
        bump,
    )]
    pub bonding_curve: Account<'info, BondingCurve>,

    /// Vault token account that holds the curve's token reserves. PDA owned by the curve
    /// account itself (i.e. authority = `bonding_curve`).
    #[account(mut)]
    /// CHECK: validated and initialized in handler.
    pub curve_vault: AccountInfo<'info>,

    /// Curve creator. Pays rent for the curve PDA + vault. (Mint rent was already paid by
    /// the caller in the earlier mint-creation ixs.) No protocol authority is granted to
    /// this address.
    #[account(mut)]
    pub creator: Signer<'info>,

    /// SPL Token-22 program.
    pub token_program: Program<'info, Token2022>,

    /// System program (for rent-paying CPIs).
    pub system_program: Program<'info, System>,

    /// Rent sysvar.
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(ctx: Context<CreateCurve>, _args: CreateArgs) -> Result<()> {
    let mint_key = ctx.accounts.mint.key();
    let curve_key = ctx.accounts.bonding_curve.key();
    let creator_key = ctx.accounts.creator.key();
    let curve_bump = ctx.bumps.bonding_curve;

    // ---- 1. Validate the pre-initialized mint ----
    if ctx.accounts.mint.owner != &spl_token_2022::id() {
        return err!(SecretPumpError::MintMissingConfidentialTransfer);
    }
    {
        let mint_data = ctx.accounts.mint.try_borrow_data()?;
        let mint_state = StateWithExtensions::<Token22Mint>::unpack(&mint_data)
            .map_err(|_| ProgramError::InvalidAccountData)?;
        if !mint_state.base.is_initialized {
            return err!(SecretPumpError::MintMissingConfidentialTransfer);
        }
        if mint_state.base.decimals != 9 {
            return err!(SecretPumpError::BadInitialTokenAllocation);
        }
        let configured_authority: Option<Pubkey> = mint_state.base.mint_authority.into();
        match configured_authority {
            Some(auth) if auth == curve_key => {}
            _ => return err!(SecretPumpError::MintMissingConfidentialTransfer),
        }
        // The mint must hold zero supply on entry — the frontend creates it fresh and only
        // this ix is allowed to mint into the vault.
        if mint_state.base.supply != 0 {
            return err!(SecretPumpError::BadInitialTokenAllocation);
        }
    }

    // ---- 2. Allocate + initialize the curve's vault token account ----
    let (vault_key, vault_bump) = Pubkey::find_program_address(
        &[BondingCurve::VAULT_SEED, mint_key.as_ref()],
        ctx.program_id,
    );
    if vault_key != ctx.accounts.curve_vault.key() {
        return err!(SecretPumpError::BadCurveVault);
    }
    create_vault_token_account(
        &ctx.accounts.curve_vault,
        &ctx.accounts.mint,
        &ctx.accounts.bonding_curve.to_account_info(),
        &ctx.accounts.creator,
        &ctx.accounts.system_program,
        &ctx.accounts.token_program,
        &ctx.accounts.rent,
        vault_bump,
    )?;

    // ---- 3. Mint the full virtual token allocation into the vault ----
    let bump_arr = [curve_bump];
    let signer_seeds_owned = BondingCurve::signer_seeds(&mint_key, &bump_arr);
    let signer_seeds: &[&[&[u8]]] = &[&signer_seeds_owned];

    let cpi_accounts = MintTo {
        mint: ctx.accounts.mint.clone(),
        to: ctx.accounts.curve_vault.clone(),
        authority: ctx.accounts.bonding_curve.to_account_info(),
    };
    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.key(),
        cpi_accounts,
        signer_seeds,
    );
    token_interface::mint_to(cpi_ctx, VIRTUAL_TOKENS)?;

    // ---- 4. Initialize the BondingCurve PDA fields ----
    let curve = &mut ctx.accounts.bonding_curve;
    curve.mint = mint_key;
    curve.creator = creator_key;
    curve.real_sol_reserves = 0;
    curve.real_token_reserves = VIRTUAL_TOKENS;
    curve.total_tokens_dispensed = 0;
    curve.total_fees_collected = 0;
    curve.graduated = false;
    curve.graduation_slot = 0;
    curve.bump = curve_bump;
    curve.vault_bump = vault_bump;

    // Sanity: the vault's deserialized balance must match what we just minted.
    let vault_data = ctx.accounts.curve_vault.try_borrow_data()?;
    let vault_state =
        spl_token_2022::extension::StateWithExtensions::<spl_token_2022::state::Account>::unpack(
            &vault_data,
        )
        .map_err(|_| ProgramError::InvalidAccountData)?;
    if vault_state.base.amount != VIRTUAL_TOKENS {
        return err!(SecretPumpError::BadInitialTokenAllocation);
    }
    drop(vault_data);

    emit!(CurveCreatedEvent {
        mint: mint_key,
        creator: creator_key,
        virtual_sol: VIRTUAL_SOL,
        virtual_tokens: VIRTUAL_TOKENS,
    });

    Ok(())
}

/// Allocate the curve's vault token account at the PDA `[VAULT_SEED, mint]` with
/// `owner = curve_pda`. Account itself is a Token-22 account (no extra extensions on the
/// vault — the confidentiality lives at the mint level / user-side accounts).
#[allow(clippy::too_many_arguments)]
fn create_vault_token_account<'info>(
    vault: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    curve_pda: &AccountInfo<'info>,
    payer: &Signer<'info>,
    system_program: &Program<'info, System>,
    token_program: &Program<'info, Token2022>,
    rent: &Sysvar<'info, Rent>,
    vault_bump: u8,
) -> Result<()> {
    let space = spl_token_2022::state::Account::LEN;
    let lamports = rent.minimum_balance(space);

    let mint_key = mint.key();
    let bump_arr = [vault_bump];
    let seeds: &[&[u8]] = &[BondingCurve::VAULT_SEED, mint_key.as_ref(), &bump_arr];
    let signer_seeds: &[&[&[u8]]] = &[seeds];

    system_program::create_account(
        CpiContext::new_with_signer(
            system_program.key(),
            system_program::CreateAccount {
                from: payer.to_account_info(),
                to: vault.clone(),
            },
            signer_seeds,
        ),
        lamports,
        space as u64,
        &spl_token_2022::id(),
    )?;

    let init_ix = token_2022_ix::initialize_account3(
        &spl_token_2022::id(),
        &vault.key(),
        &mint.key(),
        &curve_pda.key(),
    )
    .map_err(|_| ProgramError::InvalidArgument)?;
    invoke(
        &init_ix,
        &[
            vault.clone(),
            mint.clone(),
            curve_pda.clone(),
            token_program.to_account_info(),
        ],
    )?;

    Ok(())
}

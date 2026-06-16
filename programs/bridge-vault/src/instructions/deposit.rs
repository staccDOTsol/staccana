//! `deposit` — user locks underlying into the vault to mint on staccana.
//!
//! Off-chain choreography (mirror of staccana-side `mint`):
//! 1. User calls this ix on mainnet, supplying `amount` and `dest_pubkey_on_staccana`.
//! 2. The vault transfers `amount` underlying from the user (or `amount` lamports for
//!    wSOL) into the vault account, deducts `deposit_fee_bps`, increments the per-asset
//!    nonce counter, emits `DepositEvent`.
//! 3. The federation observes the event, signs an attestation
//!    `(asset_id, value_after_fee, recipient, nonce)` with M-of-N ed25519 sigs.
//! 4. User (or relayer) submits the attestation to the staccana-side `mint` ix.
//!
//! Two transfer paths, switched by `VaultConfig::is_native_sol()`:
//! - **wSOL (native SOL)**: `system_program::transfer(user → vault_config PDA)`.
//! - **SPL (stSOL/ssUSDC)**: `token_interface::transfer_checked(user_token_account →
//!   vault_token_account)`.
//!
//! The fee is implicit — the user transfers the gross `amount`, but the emitted
//! `value_after_fee` is `amount * (10_000 - deposit_fee_bps) / 10_000`. The fee
//! component stays in the vault and accrues to R via the staccana-side ratio updates
//! (or sits as profit for the wSOL vault since R is locked at 1.0).

use crate::attestation::{apply_bps_fee, CHAIN_ID_STACCANA};
use crate::error::VaultError;
use crate::state::{NonceInCounter, VaultConfig};
use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::token_interface::{self, TokenInterface, TransferChecked};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct DepositArgs {
    pub asset_id: u32,
    pub amount: u64,
    /// Destination pubkey on staccana — the recipient of the bridge mint. Opaque to
    /// the mainnet vault; re-emitted in `DepositEvent` for the federation to attest.
    pub dest_pubkey_on_staccana: [u8; 32],
}

#[derive(Accounts)]
#[instruction(args: DepositArgs)]
pub struct Deposit<'info> {
    /// User funding the deposit. Pays the underlying / native SOL.
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", args.asset_id.to_le_bytes().as_ref()],
        bump = vault_config.bump,
    )]
    pub vault_config: Account<'info, VaultConfig>,

    #[account(
        mut,
        seeds = [b"nonce_in", args.asset_id.to_le_bytes().as_ref()],
        bump = nonce_in.bump,
    )]
    pub nonce_in: Account<'info, NonceInCounter>,

    /// Optional underlying mint (for SPL-backed assets). `None` for wSOL.
    ///
    /// Anchor doesn't support real `Option<Account>` as an account, so we use the
    /// presence/absence pattern: for wSOL the caller passes the system program in this
    /// slot (or any pubkey) and the handler skips the SPL path entirely. To keep the
    /// IDL stable across asset kinds we always include this account but allow it to be
    /// arbitrary when `is_native_sol()`.
    ///
    /// CHECK: validated against `vault_config.underlying_mint` in the SPL branch only.
    pub underlying_mint: UncheckedAccount<'info>,

    /// User's source SPL token account (for the SPL branch). Ignored for wSOL.
    /// CHECK: validated by the token program at CPI time in the SPL branch only.
    #[account(mut)]
    pub user_token_account: UncheckedAccount<'info>,

    /// Vault's token account (for the SPL branch). Ignored for wSOL.
    /// CHECK: address constraint validates it matches `vault_config.vault_token_account`
    /// in the SPL branch only.
    #[account(mut)]
    pub vault_token_account: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<Deposit>, args: DepositArgs) -> Result<()> {
    require!(args.amount > 0, VaultError::ZeroDepositAmount);

    let cfg = &ctx.accounts.vault_config;
    let is_native = cfg.is_native_sol();

    // Apply deposit fee: emitted `value_after_fee` is the post-fee amount the staccana
    // side will receive credit for. The fee stays in the vault.
    let value_after_fee = apply_bps_fee(args.amount, cfg.deposit_fee_bps);

    if is_native {
        // wSOL path: native SOL transfer user → VaultConfig PDA. Anchor 1.0 takes
        // program-id `Pubkey` for `CpiContext::new`.
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.user.to_account_info(),
                    to: ctx.accounts.vault_config.to_account_info(),
                },
            ),
            args.amount,
        )?;
    } else {
        // SPL path: validate the supplied accounts match what was registered, then
        // CPI `transfer_checked`. We bind:
        //   - underlying_mint.key() == cfg.underlying_mint
        //   - vault_token_account.key() == cfg.vault_token_account
        // The token program will reject if `user_token_account.mint != mint` or
        // ownership is wrong, so we don't need a separate ownership check here.
        require_keys_eq!(
            ctx.accounts.underlying_mint.key(),
            cfg.underlying_mint,
            VaultError::AssetKindMismatch
        );
        require_keys_eq!(
            ctx.accounts.vault_token_account.key(),
            cfg.vault_token_account,
            VaultError::BadVaultTokenAccount
        );

        // Use the cached `decimals` from VaultConfig (set at init_vault from the underlying
        // mint) so we don't have to deserialize the mint account on the hot path. The
        // SPL token program will still verify mint/decimals at CPI time via
        // `transfer_checked`, so this is safe.
        let decimals = cfg.decimals;

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.user_token_account.to_account_info(),
            mint: ctx.accounts.underlying_mint.to_account_info(),
            to: ctx.accounts.vault_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        // Anchor 1.x: `CpiContext::new` takes the program id (Pubkey), NOT
        // AccountInfo. Verified by the failing compile when AccountInfo was
        // tried — `.key()` is the right form.
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.key(), cpi_accounts);
        token_interface::transfer_checked(cpi_ctx, args.amount, decimals)?;
    }

    // Allocate the next deposit nonce — read-then-increment so the emitted event
    // carries the value just allocated.
    let nonce_in = &mut ctx.accounts.nonce_in;
    let assigned_nonce = nonce_in.next_nonce;
    nonce_in.next_nonce = assigned_nonce
        .checked_add(1)
        .ok_or(VaultError::BadInstructionData)?;

    // Track total locked for off-chain solvency checks.
    let cfg_mut = &mut ctx.accounts.vault_config;
    cfg_mut.total_locked = cfg_mut
        .total_locked
        .checked_add(args.amount)
        .ok_or(VaultError::BadInstructionData)?;

    emit!(DepositEvent {
        asset_id: args.asset_id,
        user: ctx.accounts.user.key(),
        amount: args.amount,
        amount_after_fee: value_after_fee,
        dest: args.dest_pubkey_on_staccana,
        nonce: assigned_nonce,
        chain_id: CHAIN_ID_STACCANA,
    });

    Ok(())
}

/// Emitted on every successful deposit. Federation watches for these and produces a
/// signed mint attestation for the staccana-side bridge to consume.
///
/// Fields chosen to match exactly what the staccana-side `mint` ix needs:
/// `(asset_id, amount_after_fee, dest, nonce)` — same shape as
/// `attestation::build_mint_message` on the staccana side.
#[event]
pub struct DepositEvent {
    pub asset_id: u32,
    pub user: Pubkey,
    /// Gross deposit amount (pre-fee). Off-chain audit uses this to reconcile total
    /// vault inflow against `total_locked`.
    pub amount: u64,
    /// Post-fee amount that the federation should attest in the mint message. The
    /// staccana-side bridge then divides by R to compute mint amount.
    pub amount_after_fee: u64,
    /// Destination on staccana (recipient of the bridge mint).
    pub dest: [u8; 32],
    /// Per-asset deposit-direction nonce (mainnet → staccana).
    pub nonce: u64,
    /// `chain_id` discriminator — `CHAIN_ID_STACCANA` (ASCII "stac").
    pub chain_id: u32,
}

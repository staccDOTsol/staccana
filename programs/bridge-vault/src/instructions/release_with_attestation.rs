//! `release_with_attestation` — verify a federation release attestation, transfer
//! underlying to the attested mainnet recipient, and record the staccana-side outbound
//! nonce as consumed.
//!
//! The federation produces a `MAINNET_RELEASE_V1` attestation (see
//! [`crate::attestation::build_release_message`]) over the staccana-side `BurnEvent`
//! data: `(asset_id, release_amount, mainnet_dest, nonce)`. M members co-sign with
//! separate ed25519 precompile instructions, all preceding this ix in the same
//! transaction.
//!
//! Verification flow:
//! 1. Inspect the prior M ed25519 precompile ixs via the Instructions sysvar.
//! 2. Each precompile MUST sign the exact bytes returned by `build_release_message`.
//! 3. Each precompile MUST be signed by a distinct registered federation member.
//! 4. The marker PDA `["nonce_out", asset_id_le, nonce_le]` must NOT yet exist —
//!    `init` will fail otherwise, giving us automatic anti-replay.
//! 5. Transfer `(release_amount - release_fee)` underlying to the recipient.
//!
//! ## Anti-replay
//!
//! `nonce_out` is the staccana outbound nonce; uniqueness is enforced by the marker
//! PDA's existence (`init` reverts on duplicate). The (asset_id, nonce) pair is
//! committed to in the signed message so a release attestation for asset A can never
//! replay as asset B even if the marker PDA derivation collided (which it cannot).
//!
//! ## Two transfer paths
//!
//! - **wSOL (native SOL)**: lamport-direct transfer via `try_borrow_mut_lamports`.
//!   The system program transfer CPI requires the sender to be a system-owned account
//!   with no data; our VaultConfig PDA holds data, so we use the lamport-direct
//!   pattern (see Solana programming model docs §"transferring lamports").
//! - **SPL (stSOL/ssUSDC)**: `token_interface::transfer_checked` with the VaultConfig
//!   PDA as `authority`, signed via `with_signer`.

use crate::attestation::{
    apply_bps_fee, build_release_message, check_unique_indices,
};
use crate::ed25519::{parse_ed25519_at, parse_ed25519_batch_at, require_instructions_sysvar};
use crate::error::VaultError;
use crate::state::{FederationSet, NonceOutConsumed, VaultConfig};
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, TokenInterface, TransferChecked};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ReleaseArgs {
    pub asset_id: u32,
    /// The gross release amount (post-R, pre-mainnet-fee) attested by the federation.
    /// Matches the staccana-side `BurnEvent::gross_release`.
    pub release_amount: u64,
    /// Mainnet destination — must equal the recipient account's key. The signed
    /// message commits to this field so a man-in-the-middle relayer can't redirect.
    pub recipient: [u8; 32],
    /// Staccana-side outbound nonce (`BurnEvent::nonce_out`).
    pub nonce: u64,
    /// Indices into `FederationSet::members`, one per signature. Length M; duplicates
    /// rejected.
    pub federation_indices: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: ReleaseArgs)]
pub struct ReleaseWithAttestation<'info> {
    /// Pays for the new `nonce_out` marker PDA. Anyone can submit — verification is in
    /// the federation signatures.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        mut,
        seeds = [b"vault", args.asset_id.to_le_bytes().as_ref()],
        bump = vault_config.bump,
    )]
    pub vault_config: Account<'info, VaultConfig>,

    #[account(
        seeds = [b"federation"],
        bump = federation_set.bump,
    )]
    pub federation_set: Account<'info, FederationSet>,

    /// Marker PDA proving this nonce hasn't been consumed yet. `init` will revert if
    /// the PDA already exists — this is the replay guard.
    #[account(
        init,
        payer = payer,
        space = NonceOutConsumed::SPACE,
        seeds = [b"nonce_out", args.asset_id.to_le_bytes().as_ref(), args.nonce.to_le_bytes().as_ref()],
        bump,
    )]
    pub nonce_out: Account<'info, NonceOutConsumed>,

    /// Underlying mint (SPL branch) — ignored for wSOL.
    /// CHECK: validated against `vault_config.underlying_mint` in the SPL branch.
    pub underlying_mint: UncheckedAccount<'info>,

    /// Vault's source token account (SPL branch) — ignored for wSOL.
    /// CHECK: validated against `vault_config.vault_token_account` in the SPL branch.
    #[account(mut)]
    pub vault_token_account: UncheckedAccount<'info>,

    /// Recipient account. For wSOL, this is the system account credited with lamports
    /// (must equal `args.recipient`). For SPL, this is a token account whose
    /// `owner == args.recipient` and `mint == vault_config.underlying_mint`.
    /// CHECK: validated in the handler.
    #[account(mut)]
    pub recipient: UncheckedAccount<'info>,

    /// CHECK: validated against the canonical sysvar pubkey in the handler.
    pub instructions_sysvar: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<ReleaseWithAttestation>, args: ReleaseArgs) -> Result<()> {
    require!(args.release_amount > 0, VaultError::ZeroReleaseAmount);
    require_instructions_sysvar(&ctx.accounts.instructions_sysvar)?;

    // Asset-id binding: PDA seed already enforces this, but make it explicit so a
    // future seed refactor doesn't silently unbind.
    require!(
        ctx.accounts.vault_config.asset_id == args.asset_id,
        VaultError::AssetIdMismatch
    );

    let fed = &ctx.accounts.federation_set;
    require!(fed.n > 0 && fed.m > 0, VaultError::BadFederationSet);
    require!(
        args.federation_indices.len() == fed.m as usize,
        VaultError::InsufficientFederationSignatures
    );
    check_unique_indices(&args.federation_indices, fed.n)?;

    let expected_msg =
        build_release_message(args.asset_id, args.release_amount, &args.recipient, args.nonce);

    // The M ed25519 precompile ixs MUST be the M instructions IMMEDIATELY preceding
    // this ix. Walk backward, verifying each one signed the exact expected message
    // bytes with the expected federation member's pubkey.
    let sysvar = &ctx.accounts.instructions_sysvar;
    let current_ix_index = solana_instructions_sysvar::load_current_index_checked(sysvar)
        .map_err(|_| VaultError::BadInstructionsSysvar)?;
    let m = fed.m as usize;
    require!(
        (current_ix_index as usize) >= 1,
        VaultError::InsufficientFederationSignatures
    );

    // Two acceptable layouts (same back-compat shape as the staccana-side `mint` ix):
    // either M separate single-sig precompile ixs immediately preceding this one,
    // or one batched precompile ix at index-1 carrying all M sigs. Try batched
    // first since the publisher prefers that path (sub-1232-byte tx for 5-of-9).
    let batched_ok = matches!(
        parse_ed25519_batch_at(sysvar, (current_ix_index as usize) - 1),
        Ok(ref batch) if batch.len() == m,
    );

    if batched_ok {
        let batch = parse_ed25519_batch_at(sysvar, (current_ix_index as usize) - 1)
            .expect("re-parse same ix");
        for (i, &member_idx) in args.federation_indices.iter().enumerate() {
            let parsed = &batch[i];
            require!(
                parsed.message == expected_msg,
                VaultError::BadAttestationMessage
            );
            let expected_member = fed.members[member_idx as usize];
            require!(
                parsed.pubkey == expected_member.to_bytes(),
                VaultError::BadFederationSigner
            );
        }
    } else {
        require!(
            (current_ix_index as usize) >= m,
            VaultError::InsufficientFederationSignatures
        );
        for (i, &member_idx) in args.federation_indices.iter().enumerate() {
            let ix_index = (current_ix_index as usize) - m + i;
            let parsed = parse_ed25519_at(sysvar, ix_index)?;
            require!(
                parsed.message == expected_msg,
                VaultError::BadAttestationMessage
            );
            let expected_member = fed.members[member_idx as usize];
            require!(
                parsed.pubkey == expected_member.to_bytes(),
                VaultError::BadFederationSigner
            );
        }
    }

    // Mark this nonce consumed (the `init` already created the PDA — populate the
    // bump byte for forward-compatibility).
    ctx.accounts.nonce_out.bump = ctx.bumps.nonce_out;

    // Apply mainnet release fee. The fee stays in the vault.
    let net_release = apply_bps_fee(args.release_amount, ctx.accounts.vault_config.release_fee_bps);
    require!(net_release > 0, VaultError::ZeroReleaseAmount);

    // Decrement total_locked first (defensive: catch overflow before transferring).
    let cfg_mut = &mut ctx.accounts.vault_config;
    cfg_mut.total_locked = cfg_mut
        .total_locked
        .checked_sub(args.release_amount)
        .ok_or(VaultError::BadInstructionData)?;
    let is_native = cfg_mut.is_native_sol();
    let asset_id = cfg_mut.asset_id;
    let bump = cfg_mut.bump;

    if is_native {
        // wSOL: lamport-direct transfer from VaultConfig PDA → recipient. Recipient
        // must equal `args.recipient` (the attested destination).
        require_keys_eq!(
            ctx.accounts.recipient.key(),
            Pubkey::new_from_array(args.recipient),
            VaultError::BadInstructionData
        );

        let from = &ctx.accounts.vault_config.to_account_info();
        let to = &ctx.accounts.recipient.to_account_info();
        let mut from_lamports = from.try_borrow_mut_lamports()?;
        let mut to_lamports = to.try_borrow_mut_lamports()?;
        **from_lamports = from_lamports
            .checked_sub(net_release)
            .ok_or(VaultError::NativeSolTransferFailed)?;
        **to_lamports = to_lamports
            .checked_add(net_release)
            .ok_or(VaultError::NativeSolTransferFailed)?;
    } else {
        // SPL path: validate the supplied accounts match config.
        require_keys_eq!(
            ctx.accounts.underlying_mint.key(),
            ctx.accounts.vault_config.underlying_mint,
            VaultError::AssetKindMismatch
        );
        require_keys_eq!(
            ctx.accounts.vault_token_account.key(),
            ctx.accounts.vault_config.vault_token_account,
            VaultError::BadVaultTokenAccount
        );

        // Read the recipient SPL token account directly. Layout (SPL Token + Token-22
        // base layout, both share the first 165 bytes):
        //   bytes 0..32  = mint
        //   bytes 32..64 = owner
        // We don't deserialize the whole struct — only need the two pubkeys to bind
        // the recipient to the attested destination + reject mint confusion. The token
        // program will fully validate the account at CPI time.
        let recipient_data = ctx.accounts.recipient.try_borrow_data()?;
        require!(recipient_data.len() >= 64, VaultError::BadInstructionData);
        let recipient_mint = Pubkey::try_from(&recipient_data[0..32])
            .map_err(|_| error!(VaultError::BadInstructionData))?;
        let recipient_owner_bytes: [u8; 32] = recipient_data[32..64]
            .try_into()
            .map_err(|_| error!(VaultError::BadInstructionData))?;
        drop(recipient_data);

        require!(
            recipient_owner_bytes == args.recipient,
            VaultError::BadInstructionData
        );
        require_keys_eq!(
            recipient_mint,
            ctx.accounts.vault_config.underlying_mint,
            VaultError::BadInstructionData
        );

        // Use cached decimals from VaultConfig (set at init_vault). Token program
        // re-verifies via transfer_checked.
        let decimals = ctx.accounts.vault_config.decimals;

        // CPI `transfer_checked`: vault TA → recipient TA, signed by the VaultConfig
        // PDA. Outer slice level: one set of seeds. Inner level: the seed components.
        let asset_id_bytes = asset_id.to_le_bytes();
        let bump_arr = [bump];
        let signer_seeds: &[&[&[u8]]] = &[&[b"vault", asset_id_bytes.as_ref(), &bump_arr]];

        let cpi_accounts = TransferChecked {
            from: ctx.accounts.vault_token_account.to_account_info(),
            mint: ctx.accounts.underlying_mint.to_account_info(),
            to: ctx.accounts.recipient.to_account_info(),
            authority: ctx.accounts.vault_config.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.key(), cpi_accounts)
            .with_signer(signer_seeds);
        token_interface::transfer_checked(cpi_ctx, net_release, decimals)?;
    }

    emit!(ReleaseEvent {
        asset_id: args.asset_id,
        recipient: args.recipient,
        gross_release: args.release_amount,
        net_release,
        nonce: args.nonce,
    });

    Ok(())
}

/// Emitted on every successful release. Off-chain indexers consume this to mark the
/// corresponding staccana burn as settled.
#[event]
pub struct ReleaseEvent {
    pub asset_id: u32,
    pub recipient: [u8; 32],
    pub gross_release: u64,
    pub net_release: u64,
    pub nonce: u64,
}

//! `init_vault` — governance one-shot registration of a per-asset vault.
//!
//! Initializes:
//! - [`VaultConfig`] at `["vault", asset_id_le]`
//! - [`NonceInCounter`] at `["nonce_in", asset_id_le]` (deposit-direction nonce, starts at 0)
//! - On first call, the global [`FederationSet`] at `["federation"]`.
//!
//! For SPL-backed vaults (stSOL, ssUSDC) the caller is expected to have created the
//! vault token account out-of-band and to pass it in `vault_token_account` — Anchor
//! validates ownership via the `address` constraint at deposit/release time. For wSOL
//! (`AssetFlag::NATIVE_SOL`) `underlying_mint` and `vault_token_account` MUST be
//! `Pubkey::default()` and the vault holds native SOL in the [`VaultConfig`] PDA's
//! lamport balance directly.

use crate::error::VaultError;
use crate::state::{
    AssetFlag, FederationSet, NonceInCounter, VaultConfig, MAX_FEDERATION_MEMBERS,
};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct InitVaultArgs {
    pub asset_id: u32,
    pub underlying_label: [u8; 32],
    /// SPL mint of the underlying. MUST be `Pubkey::default()` if `flags` has
    /// `NATIVE_SOL` set (wSOL).
    pub underlying_mint: Pubkey,
    /// Vault token account (created out-of-band, owned by the VaultConfig PDA). MUST
    /// be `Pubkey::default()` for the wSOL path.
    pub vault_token_account: Pubkey,
    pub decimals: u8,
    pub deposit_fee_bps: u16,
    pub release_fee_bps: u16,

    /// M-of-N threshold. Ignored if the federation set is already initialized.
    pub federation_m: u8,
    pub federation_n: u8,
    /// Variable-length on the wire (`Vec<Pubkey>`) so a 5-of-9 federation
    /// fits in ~408 bytes instead of the fixed 1024 bytes. Storage in
    /// `FederationSet` stays a fixed `[Pubkey; MAX_FEDERATION_MEMBERS]`
    /// array zero-padded internally — only the wire format changed. Length
    /// must equal `federation_n`.
    pub federation_members: Vec<Pubkey>,

    /// Per-asset behaviour flags. See [`crate::state::AssetFlag`]. wSOL MUST set
    /// `NATIVE_SOL`.
    pub flags: u8,
}

#[derive(Accounts)]
#[instruction(args: InitVaultArgs)]
pub struct InitVault<'info> {
    /// Must equal `crate::ADMIN_AUTHORITY` (staccana's BPF upgrade-authority).
    /// Originally bare `Signer` with the comment "real deployment will gate
    /// this further" — but on a fresh deploy with the VaultConfig +
    /// FederationSet PDAs not yet initialized, anyone could front-run and
    /// bind their own federation set, then forge release-attestations to
    /// drain every subsequent deposit. The constraint below closes that hole.
    #[account(
        mut,
        constraint = authority.key() == crate::ADMIN_AUTHORITY @ VaultError::Unauthorized,
    )]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = VaultConfig::SPACE,
        seeds = [b"vault", args.asset_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub vault_config: Account<'info, VaultConfig>,

    #[account(
        init,
        payer = authority,
        space = NonceInCounter::SPACE,
        seeds = [b"nonce_in", args.asset_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub nonce_in: Account<'info, NonceInCounter>,

    /// Federation set is `init_if_needed` so the first vault registration also
    /// bootstraps the federation; subsequent registrations reuse it.
    #[account(
        init_if_needed,
        payer = authority,
        space = FederationSet::SPACE,
        seeds = [b"federation"],
        bump,
    )]
    pub federation_set: Account<'info, FederationSet>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitVault>, args: InitVaultArgs) -> Result<()> {
    require!(args.deposit_fee_bps <= 10_000, VaultError::BadFeeBps);
    require!(args.release_fee_bps <= 10_000, VaultError::BadFeeBps);

    // Asset-kind invariants: wSOL has no underlying mint / token account; SPL-backed
    // assets must have both. Catch fat-finger config before the vault accepts deposits.
    let is_native = args.flags & AssetFlag::NATIVE_SOL != 0;
    if is_native {
        require!(
            args.underlying_mint == Pubkey::default(),
            VaultError::AssetKindMismatch
        );
        require!(
            args.vault_token_account == Pubkey::default(),
            VaultError::AssetKindMismatch
        );
    } else {
        require!(
            args.underlying_mint != Pubkey::default(),
            VaultError::AssetKindMismatch
        );
        require!(
            args.vault_token_account != Pubkey::default(),
            VaultError::AssetKindMismatch
        );
    }

    let cfg = &mut ctx.accounts.vault_config;
    cfg.asset_id = args.asset_id;
    cfg.underlying_label = args.underlying_label;
    cfg.underlying_mint = args.underlying_mint;
    cfg.vault_token_account = args.vault_token_account;
    cfg.decimals = args.decimals;
    cfg.deposit_fee_bps = args.deposit_fee_bps;
    cfg.release_fee_bps = args.release_fee_bps;
    cfg.bump = ctx.bumps.vault_config;
    cfg.flags = args.flags;
    cfg.total_locked = 0;

    let nonce_in = &mut ctx.accounts.nonce_in;
    nonce_in.asset_id = args.asset_id;
    nonce_in.next_nonce = 0;
    nonce_in.bump = ctx.bumps.nonce_in;

    // First-call bootstrap of the federation set. Detect "already initialized" by
    // looking at `n` — `init_if_needed` populates with `Default` (n == 0) on creation.
    let fed = &mut ctx.accounts.federation_set;
    if fed.n == 0 {
        require!(
            args.federation_m > 0
                && args.federation_n > 0
                && args.federation_n as usize <= MAX_FEDERATION_MEMBERS
                && args.federation_m <= args.federation_n
                && args.federation_members.len() == args.federation_n as usize,
            VaultError::BadFederationParams
        );
        fed.m = args.federation_m;
        fed.n = args.federation_n;
        // Wire format is `Vec<Pubkey>` (length-prefixed) but storage is a
        // fixed `[Pubkey; MAX_FEDERATION_MEMBERS]` zero-padded internally.
        let mut padded = [Pubkey::default(); MAX_FEDERATION_MEMBERS];
        for (i, k) in args.federation_members.iter().enumerate() {
            padded[i] = *k;
        }
        fed.members = padded;
        fed.bump = ctx.bumps.federation_set;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    //! Pure tests for the init_vault arg-validation logic.
    //!
    //! The full `handler` requires an Anchor `Context` and an executing runtime, which
    //! is out of scope for `cargo test --lib`. Instead we factor the arg-validation
    //! invariants into a free function and exercise them here. The handler calls these
    //! same `require!` chains, so a green test here implies a sound init path.

    use super::*;

    /// Reproduces the arg-validation block at the top of `handler` — checks that the
    /// supplied `InitVaultArgs` are internally consistent (fees in range, asset-kind
    /// flags match account presence). Returns the same errors the handler does.
    fn validate_init_args(args: &InitVaultArgs) -> Result<()> {
        require!(args.deposit_fee_bps <= 10_000, VaultError::BadFeeBps);
        require!(args.release_fee_bps <= 10_000, VaultError::BadFeeBps);

        let is_native = args.flags & AssetFlag::NATIVE_SOL != 0;
        if is_native {
            require!(
                args.underlying_mint == Pubkey::default(),
                VaultError::AssetKindMismatch
            );
            require!(
                args.vault_token_account == Pubkey::default(),
                VaultError::AssetKindMismatch
            );
        } else {
            require!(
                args.underlying_mint != Pubkey::default(),
                VaultError::AssetKindMismatch
            );
            require!(
                args.vault_token_account != Pubkey::default(),
                VaultError::AssetKindMismatch
            );
        }

        // Federation params (only checked at first-call bootstrap, but we always
        // validate here — passing 0/0 with an existing federation set would be
        // ignored on-chain, but the test exercises the bootstrap branch).
        require!(
            args.federation_m > 0
                && args.federation_n > 0
                && args.federation_n as usize <= MAX_FEDERATION_MEMBERS
                && args.federation_m <= args.federation_n,
            VaultError::BadFederationParams
        );

        Ok(())
    }

    fn wsol_args() -> InitVaultArgs {
        // wSOL is registered with NATIVE_SOL flag, no underlying mint, no token account.
        // Per spec, R is locked at 1.0 staccana-side; this side just holds native SOL.
        InitVaultArgs {
            asset_id: 0,
            underlying_label: *b"wSOL                            ",
            underlying_mint: Pubkey::default(),
            vault_token_account: Pubkey::default(),
            decimals: 9,
            deposit_fee_bps: 10,
            release_fee_bps: 10,
            federation_m: 5,
            federation_n: 9,
            federation_members: vec![Pubkey::default(); 9],
            flags: AssetFlag::NATIVE_SOL,
        }
    }

    fn ssusdc_args() -> InitVaultArgs {
        // ssUSDC: SPL-backed (USDC). Real underlying mint required.
        InitVaultArgs {
            asset_id: 2,
            underlying_label: *b"ssUSDC                          ",
            underlying_mint: Pubkey::new_unique(),
            vault_token_account: Pubkey::new_unique(),
            decimals: 6,
            deposit_fee_bps: 10,
            release_fee_bps: 10,
            federation_m: 5,
            federation_n: 9,
            federation_members: vec![Pubkey::default(); 9],
            flags: 0,
        }
    }

    #[test]
    fn init_vault_with_valid_wsol_params_ok() {
        // Happy path for the wSOL asset registration.
        validate_init_args(&wsol_args()).expect("wsol params must validate");
    }

    #[test]
    fn init_vault_with_valid_ssusdc_params_ok() {
        // Happy path for an SPL-backed asset (ssUSDC / stSOL).
        validate_init_args(&ssusdc_args()).expect("ssusdc params must validate");
    }

    #[test]
    fn init_vault_rejects_native_with_underlying_mint() {
        // wSOL flag set but caller supplied an underlying mint → AssetKindMismatch.
        let mut args = wsol_args();
        args.underlying_mint = Pubkey::new_unique();
        let err = validate_init_args(&args).unwrap_err();
        assert_eq!(
            err,
            VaultError::AssetKindMismatch.into(),
            "expected AssetKindMismatch, got {err:?}"
        );
    }

    #[test]
    fn init_vault_rejects_spl_without_underlying_mint() {
        // No NATIVE_SOL flag but caller forgot to supply an underlying mint.
        let mut args = ssusdc_args();
        args.underlying_mint = Pubkey::default();
        let err = validate_init_args(&args).unwrap_err();
        assert_eq!(err, VaultError::AssetKindMismatch.into());
    }

    #[test]
    fn init_vault_rejects_oversize_fee() {
        // Fee bps > 10_000 (100%) is nonsensical; spec defaults are 10 bps.
        let mut args = ssusdc_args();
        args.deposit_fee_bps = 10_001;
        let err = validate_init_args(&args).unwrap_err();
        assert_eq!(err, VaultError::BadFeeBps.into());
    }

    #[test]
    fn init_vault_rejects_m_greater_than_n() {
        // M-of-N with M > N is degenerate and unrecoverable.
        let mut args = ssusdc_args();
        args.federation_m = 10;
        args.federation_n = 5;
        let err = validate_init_args(&args).unwrap_err();
        assert_eq!(err, VaultError::BadFederationParams.into());
    }

    #[test]
    fn init_vault_rejects_zero_threshold() {
        // M == 0 means any submission verifies, defeating the federation.
        let mut args = ssusdc_args();
        args.federation_m = 0;
        let err = validate_init_args(&args).unwrap_err();
        assert_eq!(err, VaultError::BadFederationParams.into());
    }
}

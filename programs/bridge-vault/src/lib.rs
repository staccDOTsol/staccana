//! Mainnet-side bridge vault program.
//!
//! Counterpart to `programs/bridge` (the staccana-side wrapper-mint program). This
//! program runs on **Solana mainnet** (or devnet for testing). It is the custody +
//! settlement layer for the bridge:
//!
//! - **Mainnet → staccana**: user calls [`instructions::deposit`] to lock underlying
//!   (or wrap native SOL into the vault for wSOL). The vault emits a `Deposit` event;
//!   the federation observes it, signs M-of-N, publishes a mint attestation that the
//!   staccana-side bridge consumes.
//! - **Staccana → mainnet**: user burns wrapper tokens on staccana, the federation
//!   observes the staccana-side `BurnEvent`, signs M-of-N, and the user (or a relayer)
//!   submits the resulting release attestation here via
//!   [`instructions::release_with_attestation`]. This program verifies the signatures
//!   over the canonical release-message bytes, transfers the locked underlying to the
//!   attested mainnet recipient, and marks the staccana-side outbound nonce as
//!   consumed (replay protection).
//!
//! Per-asset support mirrors the staccana side:
//!
//! - **wSOL** — vault holds native mainnet SOL; R fixed at 1.0 (`AssetFlag::R_LOCKED`);
//!   no `underlying_mint`.
//! - **stSOL** — vault holds an LST mint (e.g. pSYRUP); R accrues via federation
//!   `update_ratio` (not implemented in v1 of this crate — R lives staccana-side and
//!   the release-attestation already encodes the post-R underlying amount).
//! - **ssUSDC** — vault holds USDC; same shape as stSOL.
//!
//! ## Wire format compatibility
//!
//! The on-chain effects must be byte-compatible with what the staccana-side bridge
//! emits on `burn`. The release-attestation payload mirrors the structure of the
//! mint-attestation on the staccana side ([`crate::attestation::build_release_message`]
//! mirrors `build_mint_message`), with a distinct domain prefix
//! (`"MAINNET_RELEASE_V1"`) so a mint attestation can never replay as a release and
//! vice-versa. See SPEC §"Replay protection" — every attestation commits to
//! `(chain_id, asset_id, nonce)` and a domain prefix.

use anchor_lang::prelude::*;

pub mod attestation;
pub mod ed25519;
pub mod error;
pub mod instructions;
pub mod state;

pub use error::VaultError;
pub use instructions::*;

// Live on Solana mainnet-beta as of 2026-05-03. Deployed via upgrade
// authority HSwe2Y…5f4y. Anchor's runtime check requires this constant
// to match the runtime program-id, otherwise every ix rejects with
// `DeclaredProgramIdMismatch`. The previous `VauLt11…111` placeholder
// was never keypair-backed so we couldn't deploy under that address.
declare_id!("BwimCCoPP5of41ukG1wA1gLz5wXQ4mmbcmjdFT9M1mBL");

/// Hardcoded admin pubkey gating the one-shot `init_vault` ix.
///
/// `init_vault` initializes per-asset `VaultConfig` + `NonceInCounter`
/// PDAs and bootstraps the global `FederationSet` on first call. Bare
/// `Signer` with no constraint was the same front-run hole the auditor
/// flagged on the staccana-side `register_asset` / `update_megadrop` /
/// `init_subsidy` — on a fresh deploy anyone could call `init_vault`
/// first, bind their own federation set, and then sign their own
/// release-attestations to drain every wSOL/stSOL/ssUSDC deposit.
///
/// Same key as `staccana_bridge::ADMIN_AUTHORITY` and friends —
/// staccana's BPF upgrade-authority on val-1.
// Anchor 1.x doesn't re-export `pubkey!` — use the const-fn path directly.
pub const ADMIN_AUTHORITY: Pubkey =
    Pubkey::from_str_const("HSwe2Y7i6CPuJGb27rBwUumt8HZ8sCpQvG4PBBiC5f4y");

#[program]
pub mod staccana_bridge_vault {
    use super::*;

    /// Governance one-shot: register a new asset's vault account. Initializes the
    /// per-asset [`state::VaultConfig`] PDA, the inbound deposit nonce counter, and
    /// (on first call) the global federation set. See [`instructions::init_vault`].
    pub fn init_vault(ctx: Context<InitVault>, args: InitVaultArgs) -> Result<()> {
        instructions::init_vault::handler(ctx, args)
    }

    /// User locks `amount` of underlying (or native SOL for wSOL) into the vault and
    /// declares a destination on staccana. Increments the per-asset deposit nonce and
    /// emits a `DepositEvent` for the federation to observe. See
    /// [`instructions::deposit`].
    pub fn deposit(ctx: Context<Deposit>, args: DepositArgs) -> Result<()> {
        instructions::deposit::handler(ctx, args)
    }

    /// Verify M-of-N federation signatures over a release attestation and transfer
    /// `release_amount` of underlying to the attested mainnet recipient. The staccana
    /// outbound nonce is recorded in a marker PDA so the same attestation cannot be
    /// replayed. See [`instructions::release_with_attestation`].
    pub fn release_with_attestation(
        ctx: Context<ReleaseWithAttestation>,
        args: ReleaseArgs,
    ) -> Result<()> {
        instructions::release_with_attestation::handler(ctx, args)
    }
}

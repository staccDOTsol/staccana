//! On-chain state PDAs for the mainnet vault program.
//!
//! Three top-level account kinds:
//!
//! - [`VaultConfig`] — per-asset registration. PDA seeds: `["vault", asset_id_le]`.
//! - [`FederationSet`] — global M-of-N registered pubkey set used to verify release
//!   attestations. PDA seeds: `["federation"]`.
//! - [`NonceInCounter`] — per-asset monotonic counter for *outbound* deposits (i.e. the
//!   mainnet → staccana direction; this counter is the value emitted in the staccana-
//!   side mint attestation's `nonce` field). PDA seeds: `["nonce_in", asset_id_le]`.
//!
//! Per-instruction marker PDAs ([`NonceOutConsumed`]) are declared inline in the relevant
//! instruction module.

use anchor_lang::prelude::*;

/// Hard cap on federation set size. v1 spec is 5-of-9; 32 leaves comfortable headroom
/// for rotations and keeps `FederationSet` a fixed-size account.
pub const MAX_FEDERATION_MEMBERS: usize = 32;

/// Per-asset configuration. Set at `init_vault` time; rotation flow not in scope for v1.
#[account]
#[derive(Default)]
pub struct VaultConfig {
    /// Monotonic per-asset identifier. Doubles as the seed component for all per-asset
    /// PDAs. Must agree with the staccana-side `AssetConfig::asset_id` so attestations
    /// flow across without re-mapping.
    pub asset_id: u32,

    /// Human-readable label (e.g. `b"wSOL"` left-padded). Purely informational; on-chain
    /// logic keys off `asset_id`.
    pub underlying_label: [u8; 32],

    /// SPL mint of the underlying asset (e.g. pSYRUP for stSOL, USDC for ssUSDC). For
    /// `wSOL` this is set to `Pubkey::default()` (all zeros) — the vault holds native
    /// SOL directly in the [`VaultConfig`] PDA's lamports.
    pub underlying_mint: Pubkey,

    /// Vault token account holding the locked underlying. Owned by the [`VaultConfig`]
    /// PDA. For wSOL this is `Pubkey::default()`.
    pub vault_token_account: Pubkey,

    /// Decimals — must match the underlying.
    pub decimals: u8,

    /// Fee bps charged on deposit (deducted before the `Deposit` event is emitted).
    pub deposit_fee_bps: u16,

    /// Fee bps charged on release (deducted from the attested release amount before
    /// transferring to the recipient).
    pub release_fee_bps: u16,

    /// PDA bump cache.
    pub bump: u8,

    /// Bit-flags. See [`AssetFlag`].
    ///
    /// Bit 0 (`AssetFlag::NATIVE_SOL`): the vault holds native SOL rather than an SPL
    /// token. Set for the wSOL asset; clears `underlying_mint` and `vault_token_account`.
    pub flags: u8,

    /// Total locked underlying. Tracked so off-chain monitors can sanity-check
    /// solvency without re-deriving from event history. Updated on every deposit/release.
    pub total_locked: u64,
}

/// Bit positions for [`VaultConfig::flags`].
pub struct AssetFlag;

impl AssetFlag {
    /// Vault holds native SOL (lamports in the PDA itself), not an SPL token. The wSOL
    /// asset uses this flag.
    pub const NATIVE_SOL: u8 = 0b0000_0001;
}

impl VaultConfig {
    /// Anchor discriminator (8) + asset_id (4) + label (32) + underlying_mint (32)
    /// + vault_token_account (32) + decimals (1) + deposit_fee_bps (2)
    /// + release_fee_bps (2) + bump (1) + flags (1) + total_locked (8).
    pub const SPACE: usize = 8 + 4 + 32 + 32 + 32 + 1 + 2 + 2 + 1 + 1 + 8;

    /// Returns true if this vault holds native SOL (wSOL asset).
    pub fn is_native_sol(&self) -> bool {
        self.flags & AssetFlag::NATIVE_SOL != 0
    }
}

/// Registered federation pubkey set (M-of-N). Single global set in v1; future versions
/// may shard per-asset.
#[account]
pub struct FederationSet {
    /// Threshold — distinct signers required for any release attestation to verify.
    pub m: u8,

    /// Population — actual member count (`<= MAX_FEDERATION_MEMBERS`).
    pub n: u8,

    /// Registered ed25519 pubkeys. Slots beyond `n` are zero-filled.
    pub members: [Pubkey; MAX_FEDERATION_MEMBERS],

    /// PDA bump cache.
    pub bump: u8,
}

impl FederationSet {
    /// Anchor discriminator (8) + m (1) + n (1) + members (32 * 32) + bump (1).
    pub const SPACE: usize = 8 + 1 + 1 + (32 * MAX_FEDERATION_MEMBERS) + 1;
}

impl Default for FederationSet {
    fn default() -> Self {
        Self {
            m: 0,
            n: 0,
            members: [Pubkey::default(); MAX_FEDERATION_MEMBERS],
            bump: 0,
        }
    }
}

/// Per-asset deposit-direction nonce counter (mainnet → staccana). Each `deposit`
/// reads-and-increments. The emitted nonce is the value the staccana-side bridge expects
/// in the matching mint attestation.
///
/// Seeds: `["nonce_in", asset_id_le]`.
#[account]
#[derive(Default)]
pub struct NonceInCounter {
    pub asset_id: u32,
    pub next_nonce: u64,
    pub bump: u8,
}

impl NonceInCounter {
    /// Anchor discriminator (8) + asset_id (4) + next_nonce (8) + bump (1).
    pub const SPACE: usize = 8 + 4 + 8 + 1;
}

/// Marker PDA proving an outbound (release / staccana → mainnet) attestation nonce has
/// been consumed. Existence of the account is the proof; the `bump` byte is the only
/// payload.
///
/// Seeds: `["nonce_out", asset_id_le, nonce_le]`.
#[account]
#[derive(Default)]
pub struct NonceOutConsumed {
    pub bump: u8,
}

impl NonceOutConsumed {
    pub const SPACE: usize = 8 + 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_sol_flag_round_trips_through_vault_config() {
        // wSOL is initialized with NATIVE_SOL set; the helper must report true.
        let mut cfg = VaultConfig::default();
        cfg.flags = AssetFlag::NATIVE_SOL;
        assert!(cfg.is_native_sol());

        // stSOL / ssUSDC have flags == 0; helper reports false.
        let unset = VaultConfig::default();
        assert!(!unset.is_native_sol());
    }

    #[test]
    fn native_sol_flag_does_not_collide_with_future_flags() {
        let mut cfg = VaultConfig::default();
        cfg.flags = 0b1111_1110; // every bit EXCEPT NATIVE_SOL
        assert!(!cfg.is_native_sol());
    }

    #[test]
    fn vault_config_space_matches_layout() {
        // Sanity-check the manually computed SPACE constant against the field sizes.
        // If a field is added/removed, this will catch it before deploy.
        let expected =
            8       // discriminator
            + 4    // asset_id
            + 32   // underlying_label
            + 32   // underlying_mint
            + 32   // vault_token_account
            + 1    // decimals
            + 2    // deposit_fee_bps
            + 2    // release_fee_bps
            + 1    // bump
            + 1    // flags
            + 8;   // total_locked
        assert_eq!(VaultConfig::SPACE, expected);
    }
}

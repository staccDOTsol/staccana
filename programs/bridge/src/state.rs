//! On-chain state PDAs for the bridge program.
//!
//! Three top-level account kinds:
//!
//! - [`AssetConfig`] — per-asset registration (mainnet vault, staccana mint, fees).
//!   PDA seeds: `["asset", asset_id_le]`.
//! - [`RatioState`] — per-asset accruing ratio R (Q64.64) plus the slot at which it was
//!   last published. PDA seeds: `["ratio", asset_id_le]`.
//! - [`FederationSet`] — single global registered M-of-N pubkey set used to verify
//!   ratio attestations and inbound mint attestations. PDA seeds: `["federation"]`.
//!
//! Per-instruction "marker" PDAs ([`NonceConsumed`], [`NonceOutCounter`]) are declared
//! inline in the relevant `instructions/*.rs` modules.

use anchor_lang::prelude::*;

/// Hard cap on federation set size. v1 spec is 5-of-9 (§2.3); 32 leaves comfortable
/// headroom for future rotations and keeps `FederationSet` a fixed-size account.
pub const MAX_FEDERATION_MEMBERS: usize = 9;

/// Static per-asset configuration. Set at `register_asset` time and only governance can
/// rotate fields (rotate flow not in scope for v1).
#[account]
#[derive(Default)]
pub struct AssetConfig {
    /// Monotonic per-asset identifier. Doubles as the seed component for all per-asset
    /// PDAs (`["asset", id]`, `["ratio", id]`, `["nonce_in", id, n]`, `["nonce_out", id]`).
    pub asset_id: u32,

    /// Human-readable label (e.g. `b"stSOL"` left-padded). Purely informational; on-chain
    /// logic keys off `asset_id`.
    pub underlying_label: [u8; 32],

    /// Mainnet-side vault program. Used by relayers + UI to know where to deposit; the
    /// staccana program itself doesn't CPI into it.
    pub mainnet_vault_program: Pubkey,

    /// Token-22 mint on staccana with the Confidential Transfer extension active. Mint
    /// authority MUST be the bridge program's PDA so `mint`/`burn` can sign for it.
    pub staccana_mint: Pubkey,

    /// Decimals — must match the underlying for legibility. Stored here to avoid an extra
    /// account read on the hot path (mints don't need to load the mint account).
    pub decimals: u8,

    /// Fee bps charged on mint (paid implicitly by minting fewer tokens than `value/R`).
    /// Default: 10 bps (0.1%) per spec §2.3.
    pub mint_fee_bps: u16,

    /// Fee bps charged on burn (paid implicitly by releasing less underlying than `Z*R`).
    /// Default: 10 bps (0.1%) per spec §2.3.
    pub burn_fee_bps: u16,

    /// PDA bump cached so we don't re-derive on every instruction.
    pub bump: u8,

    /// Bit-flags controlling per-asset behaviour. See [`AssetFlag`].
    ///
    /// Bit 0 (`AssetFlag::R_LOCKED`): R is hard-pinned at 1.0 and cannot be moved by
    /// `update_ratio`. Used by the wSOL asset (1:1 mainnet SOL backing, no yield).
    /// See `docs/BRIDGE.md` §"Native SOL ↔ mainnet SOL via the bridge".
    pub flags: u8,
}

/// Bit positions for [`AssetConfig::flags`].
pub struct AssetFlag;

impl AssetFlag {
    /// R is fixed forever at 1.0 (Q64.64 == `1u128 << 64`). Any `update_ratio`
    /// attestation against this asset is rejected with [`crate::error::BridgeError::RatioLocked`].
    pub const R_LOCKED: u8 = 0b0000_0001;
}

impl AssetConfig {
    /// Anchor discriminator (8) + asset_id (4) + label (32) + mainnet_vault (32)
    /// + staccana_mint (32) + decimals (1) + mint_fee_bps (2) + burn_fee_bps (2)
    /// + bump (1) + flags (1).
    pub const SPACE: usize = 8 + 4 + 32 + 32 + 32 + 1 + 2 + 2 + 1 + 1;

    /// Returns true if R for this asset is hard-pinned at 1.0 and cannot be updated.
    /// Used by the wSOL asset.
    pub fn is_r_locked(&self) -> bool {
        self.flags & AssetFlag::R_LOCKED != 0
    }
}

/// Accruing ratio R published per-asset by the federation. Mints divide by `r_q64`,
/// burns multiply by it. See SPEC.md §5.2.
#[account]
#[derive(Default)]
pub struct RatioState {
    /// Asset this ratio belongs to. Sanity field; PDA seeds already enforce binding.
    pub asset_id: u32,

    /// R as a Q64.64 fixed-point number. `1.0 == 1u128 << 64`. The federation publishes
    /// `(vault_value, mint_supply)`; the program recomputes `R = (value << 64) / supply`
    /// to avoid trusting the federation's own division.
    pub r_q64: u128,

    /// Slot at which this R was published. Future updates must wait
    /// `R_PUBLISH_INTERVAL_SLOTS` slots before being accepted (SPEC §5.3 step 1).
    pub last_published_slot: u64,

    /// Most recent attestation nonce. Strictly increasing; replays of the same or
    /// earlier nonce are rejected even before the slot check.
    pub last_nonce: u64,

    /// PDA bump cache.
    pub bump: u8,
}

impl RatioState {
    /// Anchor discriminator (8) + asset_id (4) + r_q64 (16) + last_published_slot (8)
    /// + last_nonce (8) + bump (1).
    pub const SPACE: usize = 8 + 4 + 16 + 8 + 8 + 1;
}

/// Registered federation pubkey set (M-of-N). v1 is a single global set; future versions
/// may shard per-asset.
#[account]
pub struct FederationSet {
    /// Threshold — number of distinct signers required for any attestation to verify.
    pub m: u8,

    /// Population — actual member count (`<= MAX_FEDERATION_MEMBERS`).
    pub n: u8,

    /// Registered ed25519 pubkeys. Slots beyond `n` are zero-filled and meaningless.
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

/// Marker PDA proving an inbound (mint) attestation nonce has been consumed. Existence
/// of the account is the proof; the `bump` field is the only payload.
///
/// Seeds: `["nonce_in", asset_id_le, nonce_le]`.
#[account]
#[derive(Default)]
pub struct NonceConsumed {
    pub bump: u8,
}

impl NonceConsumed {
    /// Anchor discriminator (8) + bump (1).
    pub const SPACE: usize = 8 + 1;
}

/// Outbound (burn) nonce counter — one per asset. Incremented on each burn so that the
/// emitted event carries a strictly increasing per-asset sequence number for the mainnet
/// vault to consume in order.
///
/// Seeds: `["nonce_out", asset_id_le]`.
#[account]
#[derive(Default)]
pub struct NonceOutCounter {
    pub asset_id: u32,
    pub next_nonce: u64,
    pub bump: u8,
}

impl NonceOutCounter {
    /// Anchor discriminator (8) + asset_id (4) + next_nonce (8) + bump (1).
    pub const SPACE: usize = 8 + 4 + 8 + 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_locked_flag_round_trips_through_asset_config() {
        // wSOL is registered with R_LOCKED set; the helper must report true.
        let mut cfg = AssetConfig::default();
        cfg.flags = AssetFlag::R_LOCKED;
        assert!(cfg.is_r_locked());

        // stSOL / ssUSDC have flags == 0; helper reports false.
        let unlocked = AssetConfig::default();
        assert!(!unlocked.is_r_locked());
    }

    #[test]
    fn r_locked_flag_does_not_collide_with_future_flags() {
        // A future flag at bit 1 should not accidentally match `is_r_locked`.
        let mut cfg = AssetConfig::default();
        cfg.flags = 0b1111_1110; // every bit EXCEPT R_LOCKED
        assert!(!cfg.is_r_locked());
    }
}

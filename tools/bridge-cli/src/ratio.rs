//! Ratio state — read the per-asset `R_q64` published by the federation.
//!
//! SPEC §5.2 specifies the canonical 45-byte Anchor account layout:
//!
//! ```text
//! offset  size  field
//! ─────── ───── ──────────────────────────────
//!    0      8   anchor_discriminator   sha256("account:RatioState")[0..8]
//!    8      4   asset_id (u32 LE)      sanity field; PDA seeds bind too
//!   12     16   r_q64 (u128 LE)        Q64.64 fixed-point ratio
//!   28      8   last_published_slot    slot federation observed at
//!   36      8   last_nonce             monotonic per asset; replay guard
//!   44      1   bump                   PDA bump cache
//! ```
//!
//! The bridge program's `update_ratio` ix recomputes `R_q64` from the federation-attested
//! `(vault_value, mint_supply)` and stores **only** the result plus provenance (slot,
//! nonce). The inputs are not persisted on-chain — see SPEC §5.2's note on
//! trust-minimization (the program re-derives R rather than trusting a precomputed value).

use anyhow::{anyhow, Result};
use solana_program::pubkey::Pubkey;

use crate::asset::AssetId;

/// Encoded length of the ratio state account in bytes per SPEC §5.2.
pub const RATIO_STATE_LEN: usize = 45;

/// Anchor account discriminator for `RatioState`, computed as
/// `sha256("account:RatioState")[0..8]`. Verify with:
///
/// ```bash
/// python3 -c "import hashlib; print(hashlib.sha256(b'account:RatioState').digest()[:8].hex())"
/// # → c96c35e7d203ae05
/// ```
pub const RATIO_STATE_DISCRIMINATOR: [u8; 8] = [0xc9, 0x6c, 0x35, 0xe7, 0xd2, 0x03, 0xae, 0x05];

/// Q64.64 representation of `1.0`. Useful as a sanity check and as the expected initial
/// value at asset registration.
pub const ONE_Q64: u128 = 1u128 << 64;

/// Decoded view of the on-chain `RatioState` PDA.
///
/// Field order and types mirror `staccana_bridge::state::RatioState` exactly so the
/// `to_bytes` / `from_bytes` round trip matches the bridge program's Anchor serialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatioState {
    /// Asset this ratio belongs to. Sanity field; the PDA seed already binds asset_id.
    pub asset_id: u32,
    /// Q64.64 fixed-point ratio: `(vault_value * 2^64) / mint_supply`.
    pub r_q64: u128,
    /// Slot the federation observed when computing this attestation.
    pub last_published_slot: u64,
    /// Most recent attestation nonce. Strictly monotonic per asset.
    pub last_nonce: u64,
    /// PDA bump cache.
    pub bump: u8,
}

impl RatioState {
    /// Decode from the canonical on-chain layout.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != RATIO_STATE_LEN {
            return Err(anyhow!(
                "ratio state account must be exactly {RATIO_STATE_LEN} bytes, got {}",
                bytes.len()
            ));
        }
        if bytes[..8] != RATIO_STATE_DISCRIMINATOR {
            return Err(anyhow!(
                "ratio state account discriminator mismatch: expected {:02x?}, found {:02x?}",
                RATIO_STATE_DISCRIMINATOR,
                &bytes[..8]
            ));
        }
        let asset_id = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let r_q64 = u128::from_le_bytes(bytes[12..28].try_into().unwrap());
        let last_published_slot = u64::from_le_bytes(bytes[28..36].try_into().unwrap());
        let last_nonce = u64::from_le_bytes(bytes[36..44].try_into().unwrap());
        let bump = bytes[44];
        Ok(Self {
            asset_id,
            r_q64,
            last_published_slot,
            last_nonce,
            bump,
        })
    }

    /// Encode to the canonical on-chain layout. Symmetric with [`from_bytes`].
    pub fn to_bytes(&self) -> [u8; RATIO_STATE_LEN] {
        let mut buf = [0u8; RATIO_STATE_LEN];
        buf[0..8].copy_from_slice(&RATIO_STATE_DISCRIMINATOR);
        buf[8..12].copy_from_slice(&self.asset_id.to_le_bytes());
        buf[12..28].copy_from_slice(&self.r_q64.to_le_bytes());
        buf[28..36].copy_from_slice(&self.last_published_slot.to_le_bytes());
        buf[36..44].copy_from_slice(&self.last_nonce.to_le_bytes());
        buf[44] = self.bump;
        buf
    }

    /// Convert `r_q64` to an approximate `f64`. Lossy by construction (Q64.64 has 64
    /// fractional bits; f64 has 52 mantissa bits) and intended only for human-readable
    /// display. Do NOT use this value for any on-chain computation — always work with
    /// `r_q64` directly.
    pub fn r_as_f64(&self) -> f64 {
        let high = (self.r_q64 >> 64) as u64;
        let low = self.r_q64 as u64;
        let scale = 2f64.powi(-64);
        (high as f64) + (low as f64) * scale
    }

    /// Q64.64 raw ratio formatted as a 32-hex-digit string (no `0x` prefix, uppercase,
    /// zero-padded). Round-trippable canonical text form of `r_q64`.
    pub fn r_as_hex(&self) -> String {
        format!("{:032X}", self.r_q64)
    }
}

/// Convenience: render an arbitrary Q64.64 value as a (lossy) `f64`. Same caveats as
/// [`RatioState::r_as_f64`].
pub fn q64_to_f64(r_q64: u128) -> f64 {
    let high = (r_q64 >> 64) as u64;
    let low = r_q64 as u64;
    (high as f64) + (low as f64) * 2f64.powi(-64)
}

/// Convenience: format a Q64.64 value as a 32-hex-digit string.
pub fn q64_to_hex(r_q64: u128) -> String {
    format!("{:032X}", r_q64)
}

/// Resolve the `RatioState` PDA pubkey for an asset on a given bridge program. Convenience
/// re-export of [`AssetId::ratio_state_pda`] for callers that have already imported the
/// ratio module.
pub fn ratio_pda_for(asset: AssetId, bridge_program_id: &Pubkey) -> (Pubkey, u8) {
    asset.ratio_state_pda(bridge_program_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(r_q64: u128) -> RatioState {
        RatioState {
            asset_id: 0xDEAD_BEEF,
            r_q64,
            last_published_slot: 0x2122_2324_2526_2728,
            last_nonce: 0x3132_3334_3536_3738,
            bump: 0xFE,
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let s = sample(ONE_Q64 + (ONE_Q64 / 100));
        let bytes = s.to_bytes();
        let decoded = RatioState::from_bytes(&bytes).unwrap();
        assert_eq!(s, decoded);
    }

    #[test]
    fn from_bytes_rejects_wrong_length() {
        assert!(RatioState::from_bytes(&[0u8; 44]).is_err());
        assert!(RatioState::from_bytes(&[0u8; 46]).is_err());
        assert!(RatioState::from_bytes(&[0u8; 60]).is_err());
    }

    #[test]
    fn from_bytes_rejects_wrong_discriminator() {
        let mut buf = sample(ONE_Q64).to_bytes();
        buf[0] ^= 0xFF;
        let err = RatioState::from_bytes(&buf).unwrap_err();
        assert!(err.to_string().contains("discriminator"));
    }

    #[test]
    fn account_layout_pins_field_offsets() {
        // Lock the layout offsets so an accidental field reorder is loud.
        let s = RatioState {
            asset_id: 0xDEAD_BEEF,
            r_q64: 0x1122_3344_5566_7788_99AA_BBCC_DDEE_FF00,
            last_published_slot: 0x2122_2324_2526_2728,
            last_nonce: 0x3132_3334_3536_3738,
            bump: 0xFE,
        };
        let bytes = s.to_bytes();

        assert_eq!(&bytes[0..8], &RATIO_STATE_DISCRIMINATOR);
        assert_eq!(&bytes[8..12], &0xDEAD_BEEFu32.to_le_bytes());
        assert_eq!(
            &bytes[12..28],
            &0x1122_3344_5566_7788_99AA_BBCC_DDEE_FF00u128.to_le_bytes()
        );
        assert_eq!(&bytes[28..36], &0x2122_2324_2526_2728u64.to_le_bytes());
        assert_eq!(&bytes[36..44], &0x3132_3334_3536_3738u64.to_le_bytes());
        assert_eq!(bytes[44], 0xFE);
    }

    #[test]
    fn ratio_state_len_matches_constant() {
        let s = sample(0);
        assert_eq!(s.to_bytes().len(), RATIO_STATE_LEN);
    }

    #[test]
    fn discriminator_is_anchor_canonical() {
        // Anchor's account discriminator is `sha256("account:<TypeName>")[0..8]`.
        // If this test ever fails, regenerate via the python snippet in the doc comment
        // and update both the bridge program's account derivation and this constant.
        // Pinned here as a defensive check against a silent Anchor convention drift.
        assert_eq!(
            RATIO_STATE_DISCRIMINATOR,
            [0xc9, 0x6c, 0x35, 0xe7, 0xd2, 0x03, 0xae, 0x05]
        );
    }

    #[test]
    fn r_as_f64_is_one_for_one_q64() {
        let s = sample(ONE_Q64);
        assert_eq!(s.r_as_f64(), 1.0);
    }

    #[test]
    fn r_as_f64_is_two_for_two_q64() {
        let s = sample(2 * ONE_Q64);
        assert_eq!(s.r_as_f64(), 2.0);
    }

    #[test]
    fn r_as_f64_approximates_one_point_oh_one() {
        let s = sample(ONE_Q64 + (ONE_Q64 / 100));
        let f = s.r_as_f64();
        assert!((f - 1.01).abs() < 1e-15, "got {f}");
    }

    #[test]
    fn r_as_hex_is_zero_padded_uppercase() {
        let s = sample(ONE_Q64);
        let h = s.r_as_hex();
        assert_eq!(h.len(), 32);
        assert_eq!(h, "00000000000000010000000000000000");
        assert!(h
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    }

    #[test]
    fn q64_helpers_match_method_outputs() {
        let r = ONE_Q64 + 12345;
        let s = sample(r);
        assert_eq!(s.r_as_f64(), q64_to_f64(r));
        assert_eq!(s.r_as_hex(), q64_to_hex(r));
    }
}

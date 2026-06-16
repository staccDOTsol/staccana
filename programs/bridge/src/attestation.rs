//! Pure helpers for federation attestations and R math.
//!
//! These functions are factored out so they can be unit-tested without spinning up a
//! local validator. Anchor handler code (in `instructions/*.rs`) calls into here for
//! anything stateless — message construction, ratio arithmetic, signer-set dedup checks.
//!
//! Wire format reference: SPEC.md §5.3 (`STACCANA_RATIO_V1`) and §5.4 (mint flow).

use crate::error::BridgeError;

/// Domain-separation prefix for ratio attestations. Must match SPEC.md §5.3 byte-for-byte.
pub const RATIO_DOMAIN: &[u8] = b"STACCANA_RATIO_V1";

/// Domain-separation prefix for inbound (mainnet → staccana) mint attestations.
pub const MINT_DOMAIN: &[u8] = b"STACCANA_MINT_V1";

/// Build the ratio-attestation message that the federation signs.
///
/// Layout: `b"STACCANA_RATIO_V1" || asset_id_le || vault_value_le || mint_supply_le ||
/// slot_le || nonce_le`. Total length is 17 + 4 + 8 + 8 + 8 + 8 = 53 bytes.
pub fn build_ratio_message(
    asset_id: u32,
    vault_value: u64,
    mint_supply: u64,
    slot: u64,
    nonce: u64,
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(RATIO_DOMAIN.len() + 4 + 8 + 8 + 8 + 8);
    msg.extend_from_slice(RATIO_DOMAIN);
    msg.extend_from_slice(&asset_id.to_le_bytes());
    msg.extend_from_slice(&vault_value.to_le_bytes());
    msg.extend_from_slice(&mint_supply.to_le_bytes());
    msg.extend_from_slice(&slot.to_le_bytes());
    msg.extend_from_slice(&nonce.to_le_bytes());
    msg
}

/// Build the inbound-mint attestation message that the federation signs.
///
/// Layout: `b"STACCANA_MINT_V1" || asset_id_le || value_after_fee_le || recipient ||
/// nonce_le`. Total length is 16 + 4 + 8 + 32 + 8 = 68 bytes.
///
/// Note: SPEC §5.4 specifies the on-chain effects of `mint` but doesn't give an exact
/// byte-level attestation format. This is the canonical format the staccana-side bridge
/// expects; the mainnet vault and federation must produce the same bytes.
pub fn build_mint_message(
    asset_id: u32,
    value_after_fee: u64,
    recipient: &[u8; 32],
    nonce: u64,
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(MINT_DOMAIN.len() + 4 + 8 + 32 + 8);
    msg.extend_from_slice(MINT_DOMAIN);
    msg.extend_from_slice(&asset_id.to_le_bytes());
    msg.extend_from_slice(&value_after_fee.to_le_bytes());
    msg.extend_from_slice(recipient);
    msg.extend_from_slice(&nonce.to_le_bytes());
    msg
}

/// Compute `R_q64 = (vault_value << 64) / mint_supply`.
///
/// Returns `Err(ZeroMintSupply)` if `mint_supply == 0` — would otherwise divide by zero.
/// Result is u128 (Q64.64); `vault_value <= u64::MAX` so `(value << 64) <= u128::MAX`.
pub fn compute_r_q64(vault_value: u64, mint_supply: u64) -> Result<u128, BridgeError> {
    if mint_supply == 0 {
        return Err(BridgeError::ZeroMintSupply);
    }
    Ok(((vault_value as u128) << 64) / (mint_supply as u128))
}

/// Compute the number of staccana mint tokens to issue for `value` underlying at the
/// current ratio: `mint_amount = (value << 64) / R_q64`.
///
/// Returns `Err(ZeroRatio)` if `r_q64 == 0`, `Err(MintAmountOverflow)` if the result
/// doesn't fit in u64.
pub fn mint_amount_for_value(value: u64, r_q64: u128) -> Result<u64, BridgeError> {
    if r_q64 == 0 {
        return Err(BridgeError::ZeroRatio);
    }
    let numer = (value as u128) << 64;
    let amount = numer / r_q64;
    u64::try_from(amount).map_err(|_| BridgeError::MintAmountOverflow)
}

/// Compute the underlying-asset release amount for burning `amount` mint tokens at the
/// current ratio: `release = (amount * R_q64) >> 64`.
///
/// Uses u256 emulation via two u128 halves so that `amount * r_q64` doesn't overflow
/// for adversarial inputs (`amount = u64::MAX`, `r_q64 = u128::MAX`).
///
/// Returns `Err(ReleaseAmountOverflow)` if the high bits of the product (after `>> 64`)
/// don't fit in u64.
pub fn release_amount_for_burn(amount: u64, r_q64: u128) -> Result<u64, BridgeError> {
    // amount fits in u64 → cast to u128 is lossless. r_q64 is u128. Product is up to u192.
    // Split r_q64 into hi:64 / lo:64 and compute piecewise:
    //   product = amount * (r_hi << 64 + r_lo)
    //           = (amount * r_hi) << 64 + (amount * r_lo)
    // Then `>> 64` discards the low 64 of the second term, yielding:
    //   release = (amount * r_hi) + (amount * r_lo) >> 64
    let amt = amount as u128;
    let r_hi = r_q64 >> 64;
    let r_lo = r_q64 & ((1u128 << 64) - 1);

    // amount <= u64::MAX, r_hi <= u64::MAX → amount * r_hi fits in u128 exactly.
    let hi_term = amt
        .checked_mul(r_hi)
        .ok_or(BridgeError::ReleaseAmountOverflow)?;
    // amount <= u64::MAX, r_lo <= u64::MAX → amount * r_lo fits in u128 exactly.
    let lo_term = amt.checked_mul(r_lo).expect("u64 * u64 fits in u128");
    let lo_shifted = lo_term >> 64;

    let release = hi_term
        .checked_add(lo_shifted)
        .ok_or(BridgeError::ReleaseAmountOverflow)?;
    u64::try_from(release).map_err(|_| BridgeError::ReleaseAmountOverflow)
}

/// Apply a bps fee (deducted): `gross * (10_000 - fee_bps) / 10_000`.
///
/// Returns the post-fee amount. Fee bps must be `<= 10_000`; the spec defaults are 10
/// (0.1%). Uses u128 intermediate to avoid overflow for `amount = u64::MAX`.
pub fn apply_bps_fee(gross: u64, fee_bps: u16) -> u64 {
    debug_assert!(fee_bps <= 10_000, "fee_bps capped at 10000 = 100%");
    let bps = fee_bps.min(10_000) as u128;
    let net = ((gross as u128) * (10_000 - bps)) / 10_000;
    // gross fits in u64 → net <= gross also fits.
    net as u64
}

/// Reject duplicate signer indices within one attestation. `indices` is the M-element
/// slice the user passes in; valid attestations must have M distinct values, all in
/// `[0, n)`.
pub fn check_unique_indices(indices: &[u8], n: u8) -> Result<(), BridgeError> {
    // n caps at 32 per `MAX_FEDERATION_MEMBERS`, so a 32-bit bitset is plenty.
    let mut seen: u64 = 0;
    for &idx in indices {
        if idx >= n {
            return Err(BridgeError::FederationIndexOutOfRange);
        }
        let bit = 1u64 << idx;
        if seen & bit != 0 {
            return Err(BridgeError::DuplicateFederationSigner);
        }
        seen |= bit;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- message construction -------------------------------------------------

    #[test]
    fn ratio_message_layout_matches_spec() {
        // SPEC §5.3 specifies: prefix || asset_id_le || vault_value_le || mint_supply_le
        // || slot_le || nonce_le. Verify total length and each field at its declared
        // offset.
        let asset_id: u32 = 0xDEAD_BEEF;
        let vault_value: u64 = 0x0102_0304_0506_0708;
        let mint_supply: u64 = 0x1112_1314_1516_1718;
        let slot: u64 = 0x2122_2324_2526_2728;
        let nonce: u64 = 0x3132_3334_3536_3738;

        let msg = build_ratio_message(asset_id, vault_value, mint_supply, slot, nonce);

        assert_eq!(msg.len(), 17 + 4 + 8 + 8 + 8 + 8);
        assert_eq!(&msg[0..17], RATIO_DOMAIN);
        assert_eq!(&msg[17..21], &asset_id.to_le_bytes());
        assert_eq!(&msg[21..29], &vault_value.to_le_bytes());
        assert_eq!(&msg[29..37], &mint_supply.to_le_bytes());
        assert_eq!(&msg[37..45], &slot.to_le_bytes());
        assert_eq!(&msg[45..53], &nonce.to_le_bytes());
    }

    #[test]
    fn ratio_message_domain_is_exact_ascii() {
        // Domain prefix MUST match the spec byte-for-byte. Catch any "STACCANA_RATIO" /
        // "_V2" / casing slip in code review with a hard-coded byte check.
        assert_eq!(RATIO_DOMAIN, b"STACCANA_RATIO_V1");
        assert_eq!(RATIO_DOMAIN.len(), 17);
    }

    #[test]
    fn ratio_message_changes_for_each_field() {
        // Flip each field one at a time, verify the output bytes differ. Smoke-tests
        // that no field is silently dropped from the message.
        let base = build_ratio_message(1, 2, 3, 4, 5);
        assert_ne!(base, build_ratio_message(99, 2, 3, 4, 5));
        assert_ne!(base, build_ratio_message(1, 99, 3, 4, 5));
        assert_ne!(base, build_ratio_message(1, 2, 99, 4, 5));
        assert_ne!(base, build_ratio_message(1, 2, 3, 99, 5));
        assert_ne!(base, build_ratio_message(1, 2, 3, 4, 99));
    }

    #[test]
    fn mint_message_layout() {
        let asset_id: u32 = 7;
        let value: u64 = 1_000_000;
        let recipient = [42u8; 32];
        let nonce: u64 = 99;

        let msg = build_mint_message(asset_id, value, &recipient, nonce);
        assert_eq!(msg.len(), 16 + 4 + 8 + 32 + 8);
        assert_eq!(&msg[0..16], MINT_DOMAIN);
        assert_eq!(&msg[16..20], &asset_id.to_le_bytes());
        assert_eq!(&msg[20..28], &value.to_le_bytes());
        assert_eq!(&msg[28..60], &recipient);
        assert_eq!(&msg[60..68], &nonce.to_le_bytes());
    }

    // -- R math ---------------------------------------------------------------

    #[test]
    fn r_q64_at_one_when_supply_equals_value() {
        // 1.0 in Q64.64 is `1u128 << 64`. Initial state: no fees accrued, supply == value.
        let r = compute_r_q64(1_000_000, 1_000_000).unwrap();
        assert_eq!(r, 1u128 << 64);
    }

    #[test]
    fn r_q64_doubles_when_value_doubles() {
        // Supply unchanged, vault value doubles → R doubles. This is the model for
        // accrued yield without intervening mints.
        let r = compute_r_q64(2_000_000, 1_000_000).unwrap();
        assert_eq!(r, 2u128 << 64);
    }

    #[test]
    fn r_q64_zero_supply_rejects() {
        let err = compute_r_q64(100, 0).unwrap_err();
        assert!(matches!(err, BridgeError::ZeroMintSupply));
    }

    #[test]
    fn mint_amount_at_unit_ratio_is_identity() {
        // At R == 1.0, deposit X underlying → mint X tokens.
        let r = 1u128 << 64;
        assert_eq!(mint_amount_for_value(12_345, r).unwrap(), 12_345);
    }

    #[test]
    fn mint_amount_at_2x_ratio_halves() {
        // At R == 2.0, each token is worth 2 underlying → minting against value X
        // produces X/2 tokens.
        let r = 2u128 << 64;
        assert_eq!(mint_amount_for_value(1_000, r).unwrap(), 500);
    }

    #[test]
    fn mint_amount_zero_ratio_rejects() {
        let err = mint_amount_for_value(100, 0).unwrap_err();
        assert!(matches!(err, BridgeError::ZeroRatio));
    }

    #[test]
    fn mint_amount_overflow_caught() {
        // R extremely small → mint amount blows past u64. Pick R = 1 (Q64.64 ≈ 5e-20)
        // and a non-trivial value. Value=2, R=1 → amount = (2 << 64) / 1 = 2 << 64 ≫ u64.
        let err = mint_amount_for_value(2, 1).unwrap_err();
        assert!(matches!(err, BridgeError::MintAmountOverflow));
    }

    #[test]
    fn release_amount_at_unit_ratio_is_identity() {
        let r = 1u128 << 64;
        assert_eq!(release_amount_for_burn(99_999, r).unwrap(), 99_999);
    }

    #[test]
    fn release_amount_at_2x_ratio_doubles() {
        // At R == 2.0, each mint token redeems 2 underlying.
        let r = 2u128 << 64;
        assert_eq!(release_amount_for_burn(500, r).unwrap(), 1_000);
    }

    #[test]
    fn release_amount_at_half_ratio_halves() {
        // R == 0.5 (post-slashing scenario). 1000 mint tokens → 500 underlying.
        let r = 1u128 << 63; // 0.5 in Q64.64
        assert_eq!(release_amount_for_burn(1_000, r).unwrap(), 500);
    }

    #[test]
    fn release_amount_overflow_caught() {
        // amount = u64::MAX, R == 2.0 → release > u64::MAX.
        let r = 2u128 << 64;
        let err = release_amount_for_burn(u64::MAX, r).unwrap_err();
        assert!(matches!(err, BridgeError::ReleaseAmountOverflow));
    }

    #[test]
    fn release_amount_handles_max_r_q64_without_panic() {
        // Adversarial: R = u128::MAX. The naive `(amount as u128) * r_q64` would
        // overflow; the hi/lo split must handle this without panicking. amount = 1,
        // R = u128::MAX → release = (1 * u128::MAX) >> 64, which is roughly u128::MAX / 2^64
        // = 2^64 - 1 = u64::MAX.
        let r = u128::MAX;
        let result = release_amount_for_burn(1, r);
        assert_eq!(result.unwrap(), u64::MAX);
    }

    #[test]
    fn release_amount_round_trip_with_mint() {
        // Mint then burn at the same R should round-trip exactly (no fees applied).
        // Use a non-power-of-2 R to exercise truncation: R = 1.5.
        let r = (1u128 << 64) + (1u128 << 63); // 1.5 in Q64.64
        let value = 1_000_000;
        let minted = mint_amount_for_value(value, r).unwrap();
        let released = release_amount_for_burn(minted, r).unwrap();
        // Allow 1 lamport rounding drift (truncation in both directions).
        assert!(value.abs_diff(released) <= 1, "drift: {} vs {}", value, released);
    }

    // -- bps fee --------------------------------------------------------------

    #[test]
    fn bps_fee_zero_is_passthrough() {
        assert_eq!(apply_bps_fee(1_000_000, 0), 1_000_000);
    }

    #[test]
    fn bps_fee_default_10_bps() {
        // 10 bps = 0.1% → 1_000_000 * 0.999 = 999_000.
        assert_eq!(apply_bps_fee(1_000_000, 10), 999_000);
    }

    #[test]
    fn bps_fee_handles_max_u64_no_overflow() {
        // u64::MAX gross with 10 bps fee should not panic. Expected: u64::MAX * 9990 / 10000.
        let result = apply_bps_fee(u64::MAX, 10);
        let expected = ((u64::MAX as u128) * 9_990 / 10_000) as u64;
        assert_eq!(result, expected);
    }

    #[test]
    fn bps_fee_full_take() {
        // 10_000 bps = 100% → result is 0.
        assert_eq!(apply_bps_fee(1_000_000, 10_000), 0);
    }

    #[test]
    fn bps_fee_at_100_percent_returns_zero() {
        // 10_000 bps == 100% — the boundary. Returns zero (debug_assert allows it).
        assert_eq!(apply_bps_fee(500, 10_000), 0);
    }

    // -- signer dedup ---------------------------------------------------------

    #[test]
    fn unique_indices_accepts_distinct() {
        check_unique_indices(&[0, 1, 2, 3, 4], 9).unwrap();
    }

    #[test]
    fn unique_indices_rejects_duplicate() {
        let err = check_unique_indices(&[0, 1, 2, 1, 4], 9).unwrap_err();
        assert!(matches!(err, BridgeError::DuplicateFederationSigner));
    }

    #[test]
    fn unique_indices_rejects_out_of_range() {
        let err = check_unique_indices(&[0, 1, 9, 3, 4], 9).unwrap_err();
        assert!(matches!(err, BridgeError::FederationIndexOutOfRange));
    }

    #[test]
    fn unique_indices_empty_is_ok() {
        check_unique_indices(&[], 9).unwrap();
    }

    #[test]
    fn unique_indices_handles_full_population() {
        // All 32 indices distinct — exercises the full bitset width.
        let all: Vec<u8> = (0u8..32).collect();
        check_unique_indices(&all, 32).unwrap();
    }

    // -- wSOL (R-locked at 1.0) sanity ----------------------------------------

    /// wSOL is the 1:1 mainnet-SOL-backed asset (`docs/BRIDGE.md` §"Native SOL ↔
    /// mainnet SOL"). R is hard-pinned at 1.0 forever via [`crate::state::AssetFlag::R_LOCKED`].
    /// These tests document the math at that fixed R.
    const WSOL_R_Q64: u128 = 1u128 << 64;

    #[test]
    fn wsol_mint_is_lamport_for_lamport() {
        // 1 lamport mainnet-SOL → 1 lamport wSOL, every time, for any input that fits
        // in u64. R==1.0 means `mint_amount = value`.
        for &v in &[1u64, 1_000, 1_000_000_000, u64::MAX / 2] {
            assert_eq!(mint_amount_for_value(v, WSOL_R_Q64).unwrap(), v);
        }
    }

    #[test]
    fn wsol_burn_is_lamport_for_lamport() {
        // wSOL burn at R=1.0: release == amount, no truncation possible.
        for &a in &[1u64, 1_000, 1_000_000_000, u64::MAX / 2] {
            assert_eq!(release_amount_for_burn(a, WSOL_R_Q64).unwrap(), a);
        }
    }

    #[test]
    fn wsol_round_trip_is_exact_at_zero_fee() {
        // No drift on mint→burn at R=1.0 with no fees applied. Distinguishes the wSOL
        // path from the stSOL path which can drift 1 lamport at non-power-of-2 R.
        let value = 999_999_999;
        let minted = mint_amount_for_value(value, WSOL_R_Q64).unwrap();
        let released = release_amount_for_burn(minted, WSOL_R_Q64).unwrap();
        assert_eq!(value, released, "wSOL must round-trip exactly at R=1.0");
    }
}

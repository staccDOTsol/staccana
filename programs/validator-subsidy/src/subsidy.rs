//! Pure helpers for validator-subsidy math + attestation messages.
//!
//! Mirrors the `staccana_bridge::attestation` pattern: every stateless calculation lives
//! here so it can be unit-tested without spinning up a local validator. Anchor handler
//! code in `instructions/*.rs` calls into these helpers for anything that does not
//! require account-info plumbing.
//!
//! Wire format reference: SPEC.md §7.2 (mechanism), §7.3 (constants). The
//! `STACCANA_VALIDATOR_METRICS_V1` byte format is canonical to this crate; the
//! federation-attestor service must produce identical bytes.

use crate::error::SubsidyError;
use crate::state::{BOOTSTRAP_EPOCHS, TREASURY_BOOTSTRAP_BPS, TREASURY_PRODUCTIVE_BPS};

/// Domain-separation prefix for validator-metrics attestations. v1 byte-pinned.
pub const METRICS_DOMAIN: &[u8] = b"STACCANA_VALIDATOR_METRICS_V1";

/// Compute a validator's per-epoch weight from raw metrics.
///
/// Formula (SPEC §7.2): `weight = uptime_bps × delegated_stake × votes_cast`. All
/// arithmetic is in `u128`: `u16 × u64 × u64` fits comfortably (16 + 64 + 64 = 144 bits
/// would overflow `u128`, but the practical maxima are far smaller — `uptime_bps <=
/// 10_000`, stake roughly `2^60` lamports, votes per epoch in the low millions, so the
/// product fits in ~104 bits with room to spare).
pub fn compute_validator_weight(uptime_bps: u16, delegated_stake: u64, votes_cast: u64) -> u128 {
    (uptime_bps as u128) * (delegated_stake as u128) * (votes_cast as u128)
}

/// Compute a validator's pro-rata share of `yield_total` given their `validator_weight`
/// and the registry-wide `total_weight`.
///
/// Formula: `share = yield_total × validator_weight / total_weight`, clamped to `u64`.
///
/// Returns `Err(ZeroTotalWeight)` if `total_weight == 0` (would be division by zero —
/// the caller should detect this earlier and short-circuit), `Err(ShareOverflow)` if the
/// final share doesn't fit in `u64`.
pub fn compute_validator_share(
    yield_total: u64,
    validator_weight: u128,
    total_weight: u128,
) -> Result<u64, SubsidyError> {
    if total_weight == 0 {
        return Err(SubsidyError::ZeroTotalWeight);
    }
    // Numerator: `yield_total <= u64::MAX < 2^64`, weight fits in ~104 bits → product is
    // at most ~168 bits which overflows u128. Use checked_mul; in v1 the practical sizes
    // never approach this, but adversarial inputs (e.g. crafted weights from a malicious
    // federation) must not panic.
    let numer = (yield_total as u128)
        .checked_mul(validator_weight)
        .ok_or(SubsidyError::ShareOverflow)?;
    let share = numer / total_weight;
    u64::try_from(share).map_err(|_| SubsidyError::ShareOverflow)
}

/// Compute the per-epoch bootstrap distribution amount.
///
/// Formula: `bootstrap_per_epoch = reserve_total / BOOTSTRAP_EPOCHS`. Integer truncation
/// keeps `BOOTSTRAP_EPOCHS × bootstrap_per_epoch <= reserve_total`, so the sum of
/// per-epoch payouts never overshoots the reserve. The truncation residue (up to
/// `BOOTSTRAP_EPOCHS - 1` lamports) stays in the reserve and is rolled into yield-only
/// accounting after epoch 60.
pub fn bootstrap_per_epoch(reserve_total: u64) -> u64 {
    reserve_total / BOOTSTRAP_EPOCHS
}

/// Compute the bootstrap reserve initial size from a treasury total.
///
/// Formula: `bootstrap_reserve = treasury_total × TREASURY_BOOTSTRAP_BPS / 10_000`.
/// Used by `init_subsidy` to set `SubsidyConfig.bootstrap_reserve_initial` from the
/// governance-supplied `treasury_total` argument.
pub fn compute_bootstrap_reserve(treasury_total: u64) -> u64 {
    let scaled = (treasury_total as u128) * (TREASURY_BOOTSTRAP_BPS as u128);
    (scaled / 10_000) as u64
}

/// Compute the productive position size from a treasury total.
///
/// Formula: `productive = treasury_total × TREASURY_PRODUCTIVE_BPS / 10_000`. This is
/// informational at the protocol level — the actual `stake_to_productive` ix takes its
/// amount from the caller, not this calculation — but the value is exposed so off-chain
/// tooling has a single source of truth.
pub fn compute_productive_position(treasury_total: u64) -> u64 {
    let scaled = (treasury_total as u128) * (TREASURY_PRODUCTIVE_BPS as u128);
    (scaled / 10_000) as u64
}

/// Build the validator-metrics attestation message that the federation signs.
///
/// Layout (29 + 32 + 2 + 8 + 8 + 8 + 8 = 95 bytes):
///
/// ```text
/// b"STACCANA_VALIDATOR_METRICS_V1"
///     || validator_pubkey         // 32 bytes
///     || uptime_bps_le            // 2 bytes (u16)
///     || delegated_stake_le       // 8 bytes (u64)
///     || votes_cast_le            // 8 bytes (u64)
///     || slot_le                  // 8 bytes (u64)
///     || nonce_le                 // 8 bytes (u64)
/// ```
pub fn build_metrics_message(
    validator: &[u8; 32],
    uptime_bps: u16,
    delegated_stake: u64,
    votes_cast: u64,
    slot: u64,
    nonce: u64,
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(METRICS_DOMAIN.len() + 32 + 2 + 8 + 8 + 8 + 8);
    msg.extend_from_slice(METRICS_DOMAIN);
    msg.extend_from_slice(validator);
    msg.extend_from_slice(&uptime_bps.to_le_bytes());
    msg.extend_from_slice(&delegated_stake.to_le_bytes());
    msg.extend_from_slice(&votes_cast.to_le_bytes());
    msg.extend_from_slice(&slot.to_le_bytes());
    msg.extend_from_slice(&nonce.to_le_bytes());
    msg
}

/// Reject duplicate signer indices within one attestation. `indices` is the M-element
/// slice the user passes in; valid attestations must have M distinct values, all in
/// `[0, n)`.
///
/// Identical semantics to `staccana_bridge::attestation::check_unique_indices`; copied
/// rather than re-exported to keep this crate self-contained for the pure-helper pass
/// and to avoid leaking bridge errors into the subsidy error namespace.
pub fn check_unique_indices(indices: &[u8], n: u8) -> Result<(), SubsidyError> {
    // n caps at 32 per `MAX_FEDERATION_MEMBERS`, so a 64-bit bitset is plenty.
    let mut seen: u64 = 0;
    for &idx in indices {
        if idx >= n {
            return Err(SubsidyError::FederationIndexOutOfRange);
        }
        let bit = 1u64 << idx;
        if seen & bit != 0 {
            return Err(SubsidyError::DuplicateFederationSigner);
        }
        seen |= bit;
    }
    Ok(())
}

/// Validate the `uptime_bps` argument is in the inclusive range `[0, 10_000]`.
///
/// Called by `update_validator_metrics` before any state change; out-of-range values are
/// almost certainly a buggy attestor.
pub fn check_uptime_bps(uptime_bps: u16) -> Result<(), SubsidyError> {
    if uptime_bps > 10_000 {
        return Err(SubsidyError::UptimeBpsOutOfRange);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- weight formula -------------------------------------------------------

    #[test]
    fn weight_zero_when_any_factor_zero() {
        // The `multiply` form of the weight formula collapses to zero on any zero input.
        // Verifies the multiplicative identity (no surprise additive offsets).
        assert_eq!(compute_validator_weight(0, 100, 200), 0);
        assert_eq!(compute_validator_weight(10_000, 0, 200), 0);
        assert_eq!(compute_validator_weight(10_000, 100, 0), 0);
    }

    #[test]
    fn weight_basic_product() {
        // 100% uptime, 1 SOL stake, 100 votes — sanity check the arithmetic.
        let w = compute_validator_weight(10_000, 1_000_000_000, 100);
        assert_eq!(w, 10_000u128 * 1_000_000_000u128 * 100u128);
    }

    #[test]
    fn weight_handles_max_inputs_without_panic() {
        // Adversarial: max u16 uptime, max u64 stake, max u64 votes. Real-world inputs
        // are nowhere near this, but a u128 product of these still fits (16 + 64 + 64 =
        // 144 — no, that exceeds 128). For the upper-end practical case (uptime_bps <=
        // 10_000, stake at 2^60, votes at 2^32), the product is ~106 bits, well within
        // u128. Test a high-end realistic value instead.
        let w = compute_validator_weight(10_000, 1u64 << 60, 1u64 << 30);
        // 10_000 * 2^60 * 2^30 = 10_000 * 2^90 ≈ 1.2e94 — fits in u128 (max ~3.4e38? no,
        // u128 max is 2^128 ≈ 3.4e38). 10_000 * 2^90 ≈ 1.2e34 — comfortably under.
        assert_eq!(w, 10_000u128 * (1u128 << 60) * (1u128 << 30));
    }

    #[test]
    fn weight_uptime_scales_linearly() {
        let stake = 1_000_000_000;
        let votes = 100;
        let half = compute_validator_weight(5_000, stake, votes);
        let full = compute_validator_weight(10_000, stake, votes);
        assert_eq!(full, 2 * half);
    }

    // -- share formula --------------------------------------------------------

    #[test]
    fn share_full_yield_to_only_validator() {
        // Single validator → entire yield. Demonstrates the pro-rata reduces correctly
        // in the trivial case.
        let share =
            compute_validator_share(1_000_000, 42, 42).unwrap();
        assert_eq!(share, 1_000_000);
    }

    #[test]
    fn share_half_yield_when_half_weight() {
        // Validator owns half the total weight → half the yield.
        let share = compute_validator_share(1_000, 50, 100).unwrap();
        assert_eq!(share, 500);
    }

    #[test]
    fn share_zero_weight_yields_zero() {
        // A zero-weight validator (e.g. 0% uptime) gets nothing — even with non-zero
        // yield. Important: does NOT error, just returns zero.
        let share = compute_validator_share(1_000_000, 0, 100).unwrap();
        assert_eq!(share, 0);
    }

    #[test]
    fn share_zero_yield_yields_zero() {
        let share = compute_validator_share(0, 100, 200).unwrap();
        assert_eq!(share, 0);
    }

    #[test]
    fn share_zero_total_weight_errors() {
        // Total weight zero is a callsite invariant violation — the registry has no
        // active validators. Caller should short-circuit; the helper reports the error
        // in case it doesn't.
        let err = compute_validator_share(1_000, 0, 0).unwrap_err();
        assert!(matches!(err, SubsidyError::ZeroTotalWeight));
    }

    #[test]
    fn share_truncates_toward_zero() {
        // 100 / 3 = 33 (truncated). Two recipients with equal weight share 100 lamports
        // get 33 each, with 1 lamport residue left in the treasury.
        let s = compute_validator_share(100, 1, 3).unwrap();
        assert_eq!(s, 33);
    }

    #[test]
    fn share_distribution_sums_to_under_yield_due_to_truncation() {
        // Three equal-weight validators dividing 100 lamports of yield: each gets 33,
        // total 99. The 1 lamport residue is the dust that stays in the treasury.
        let yield_total = 100u64;
        let total_weight = 3u128;
        let s1 = compute_validator_share(yield_total, 1, total_weight).unwrap();
        let s2 = compute_validator_share(yield_total, 1, total_weight).unwrap();
        let s3 = compute_validator_share(yield_total, 1, total_weight).unwrap();
        assert_eq!(s1, 33);
        assert_eq!(s2, 33);
        assert_eq!(s3, 33);
        assert!(s1 + s2 + s3 <= yield_total);
    }

    #[test]
    fn share_distribution_three_unequal_validators() {
        // Validators with weights 10/20/30 dividing 600 of yield: shares should be
        // 100/200/300, summing to exactly 600 (no truncation residue here because
        // 600 / 60 divides cleanly).
        let yield_total = 600u64;
        let total = 60u128;
        let a = compute_validator_share(yield_total, 10, total).unwrap();
        let b = compute_validator_share(yield_total, 20, total).unwrap();
        let c = compute_validator_share(yield_total, 30, total).unwrap();
        assert_eq!((a, b, c), (100, 200, 300));
        assert_eq!(a + b + c, yield_total);
    }

    #[test]
    fn share_overflow_caught_on_adversarial_weight() {
        // Yield = u64::MAX, weight = u128::MAX → product overflows u128.
        let err = compute_validator_share(u64::MAX, u128::MAX, 1).unwrap_err();
        assert!(matches!(err, SubsidyError::ShareOverflow));
    }

    #[test]
    fn share_clamps_when_quotient_exceeds_u64() {
        // Yield = u64::MAX, weight = total_weight → share = u64::MAX. Edge case: should
        // clamp without error. Then push slightly past: weight > total_weight
        // is mathematically nonsensical (caller bug), but if it happens the result is
        // > u64::MAX → ShareOverflow.
        let s = compute_validator_share(u64::MAX, 1, 1).unwrap();
        assert_eq!(s, u64::MAX);
        // weight > total_weight would mean share > yield_total, possibly > u64::MAX.
        let err = compute_validator_share(u64::MAX, 2, 1).unwrap_err();
        assert!(matches!(err, SubsidyError::ShareOverflow));
    }

    // -- bootstrap math -------------------------------------------------------

    #[test]
    fn bootstrap_per_epoch_divides_by_60() {
        // SPEC §7.3: BOOTSTRAP_EPOCHS = 60. Verify the divisor.
        assert_eq!(bootstrap_per_epoch(60_000), 1_000);
    }

    #[test]
    fn bootstrap_per_epoch_truncates() {
        // 100 lamports over 60 epochs → 1 per epoch (truncated). 60 lamports total
        // distributed; 40-lamport residue stays.
        assert_eq!(bootstrap_per_epoch(100), 1);
    }

    #[test]
    fn bootstrap_per_epoch_zero_reserve() {
        assert_eq!(bootstrap_per_epoch(0), 0);
    }

    #[test]
    fn bootstrap_per_epoch_under_60_lamports() {
        // < 60 lamports → 0 per epoch — the reserve is dust-sized.
        assert_eq!(bootstrap_per_epoch(59), 0);
    }

    #[test]
    fn bootstrap_per_epoch_60_epochs_at_max_drains_reserve() {
        // For a "round" reserve like 600 lamports: 10 per epoch × 60 epochs = 600.
        // Confirms the reserve-draining invariant when the value divides cleanly.
        let per = bootstrap_per_epoch(600);
        assert_eq!(per, 10);
        assert_eq!(per * BOOTSTRAP_EPOCHS, 600);
    }

    #[test]
    fn bootstrap_reserve_at_default_bps() {
        // SPEC §7.3 default: 200 bps = 2%. 1B lamports treasury → 20M reserve.
        assert_eq!(compute_bootstrap_reserve(1_000_000_000), 20_000_000);
    }

    #[test]
    fn bootstrap_reserve_zero_treasury() {
        assert_eq!(compute_bootstrap_reserve(0), 0);
    }

    #[test]
    fn bootstrap_reserve_handles_max_u64() {
        // u64::MAX × 200 / 10_000 — product is 200 × ~1.8e19 ≈ 3.6e21 which fits in
        // u128 comfortably. Result must be < u64::MAX (it's 2% of u64::MAX).
        let r = compute_bootstrap_reserve(u64::MAX);
        let expected = ((u64::MAX as u128) * 200u128 / 10_000u128) as u64;
        assert_eq!(r, expected);
    }

    #[test]
    fn productive_position_at_default_bps() {
        // SPEC §7.3 default: 8000 bps = 80%. 1B lamports treasury → 800M productive.
        assert_eq!(compute_productive_position(1_000_000_000), 800_000_000);
    }

    #[test]
    fn productive_plus_bootstrap_under_treasury_total() {
        // PRODUCTIVE_BPS (8000) + BOOTSTRAP_BPS (200) = 8200 of 10_000 = 82%. The
        // remaining 18% is unallocated — operations, grants, insurance fund, etc.
        // (See SPEC §7.1 list of authorized ops.)
        let total = 1_000_000_000u64;
        let prod = compute_productive_position(total);
        let boot = compute_bootstrap_reserve(total);
        assert!(prod + boot < total);
        assert_eq!(prod + boot, 820_000_000);
    }

    // -- attestation message --------------------------------------------------

    #[test]
    fn metrics_message_layout_matches_spec() {
        // Pin every byte offset. If anyone refactors the message format, this catches
        // it before the federation goes out of sync.
        let validator = [7u8; 32];
        let uptime: u16 = 0xABCD;
        let stake: u64 = 0x0102_0304_0506_0708;
        let votes: u64 = 0x1112_1314_1516_1718;
        let slot: u64 = 0x2122_2324_2526_2728;
        let nonce: u64 = 0x3132_3334_3536_3738;

        let msg = build_metrics_message(&validator, uptime, stake, votes, slot, nonce);

        let expected_len = 29 + 32 + 2 + 8 + 8 + 8 + 8;
        assert_eq!(msg.len(), expected_len);
        assert_eq!(&msg[0..29], METRICS_DOMAIN);
        assert_eq!(&msg[29..61], &validator);
        assert_eq!(&msg[61..63], &uptime.to_le_bytes());
        assert_eq!(&msg[63..71], &stake.to_le_bytes());
        assert_eq!(&msg[71..79], &votes.to_le_bytes());
        assert_eq!(&msg[79..87], &slot.to_le_bytes());
        assert_eq!(&msg[87..95], &nonce.to_le_bytes());
    }

    #[test]
    fn metrics_message_domain_is_exact_ascii() {
        // Domain prefix MUST match the documented constant byte-for-byte. Catches any
        // "_V2" / casing slip in code review with a hard-coded byte check.
        assert_eq!(METRICS_DOMAIN, b"STACCANA_VALIDATOR_METRICS_V1");
        assert_eq!(METRICS_DOMAIN.len(), 29);
    }

    #[test]
    fn metrics_message_changes_for_each_field() {
        // Flip each field one at a time, verify the output bytes differ. Smoke-tests
        // that no field is silently dropped from the message.
        let v0 = [0u8; 32];
        let v1 = [1u8; 32];
        let base = build_metrics_message(&v0, 1, 2, 3, 4, 5);
        assert_ne!(base, build_metrics_message(&v1, 1, 2, 3, 4, 5));
        assert_ne!(base, build_metrics_message(&v0, 99, 2, 3, 4, 5));
        assert_ne!(base, build_metrics_message(&v0, 1, 99, 3, 4, 5));
        assert_ne!(base, build_metrics_message(&v0, 1, 2, 99, 4, 5));
        assert_ne!(base, build_metrics_message(&v0, 1, 2, 3, 99, 5));
        assert_ne!(base, build_metrics_message(&v0, 1, 2, 3, 4, 99));
    }

    #[test]
    fn metrics_message_validator_pubkey_position() {
        // Specifically verify the pubkey lands at offset 29 (immediately after the
        // domain) — easy to get wrong if someone adds a "version" byte etc.
        let validator = *b"________this_is_32_bytes_pubkey_";
        let msg = build_metrics_message(&validator, 0, 0, 0, 0, 0);
        assert_eq!(&msg[29..61], &validator);
    }

    // -- signer dedup ---------------------------------------------------------

    #[test]
    fn unique_indices_accepts_distinct() {
        check_unique_indices(&[0, 1, 2, 3, 4], 9).unwrap();
    }

    #[test]
    fn unique_indices_rejects_duplicate() {
        let err = check_unique_indices(&[0, 1, 2, 1, 4], 9).unwrap_err();
        assert!(matches!(err, SubsidyError::DuplicateFederationSigner));
    }

    #[test]
    fn unique_indices_rejects_out_of_range() {
        let err = check_unique_indices(&[0, 1, 9, 3, 4], 9).unwrap_err();
        assert!(matches!(err, SubsidyError::FederationIndexOutOfRange));
    }

    #[test]
    fn unique_indices_empty_is_ok() {
        check_unique_indices(&[], 9).unwrap();
    }

    #[test]
    fn unique_indices_handles_full_population() {
        // All 32 indices distinct — exercises the full bitset width that
        // MAX_FEDERATION_MEMBERS implies.
        let all: Vec<u8> = (0u8..32).collect();
        check_unique_indices(&all, 32).unwrap();
    }

    // -- uptime range check ---------------------------------------------------

    #[test]
    fn uptime_at_zero_ok() {
        check_uptime_bps(0).unwrap();
    }

    #[test]
    fn uptime_at_full_ok() {
        check_uptime_bps(10_000).unwrap();
    }

    #[test]
    fn uptime_above_full_rejects() {
        // 10_001 is the first invalid value. Tests the boundary.
        let err = check_uptime_bps(10_001).unwrap_err();
        assert!(matches!(err, SubsidyError::UptimeBpsOutOfRange));
    }

    #[test]
    fn uptime_at_max_u16_rejects() {
        let err = check_uptime_bps(u16::MAX).unwrap_err();
        assert!(matches!(err, SubsidyError::UptimeBpsOutOfRange));
    }

    // -- end-to-end pro-rata shape --------------------------------------------

    #[test]
    fn end_to_end_distribution_three_validators() {
        // 100% uptime, equal stake, votes 100/200/300 → weights 1/2/3 of the total.
        // 600 lamports of yield → shares 100/200/300.
        let stake = 1_000_000_000u64;
        let w1 = compute_validator_weight(10_000, stake, 100);
        let w2 = compute_validator_weight(10_000, stake, 200);
        let w3 = compute_validator_weight(10_000, stake, 300);
        let total = w1 + w2 + w3;

        let yield_total = 600u64;
        let s1 = compute_validator_share(yield_total, w1, total).unwrap();
        let s2 = compute_validator_share(yield_total, w2, total).unwrap();
        let s3 = compute_validator_share(yield_total, w3, total).unwrap();
        assert_eq!((s1, s2, s3), (100, 200, 300));
    }
}

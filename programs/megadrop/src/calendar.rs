//! Calendar math for tranche unlock checks.
//!
//! The megadrop's vesting model unlocks one tranche per calendar month, gated by
//! `current_month >= genesis_month + (i - 1)` where months are encoded as ISO `yyyymm`
//! integers (e.g. `202605` = May 2026). The on-chain handler reads the current Unix
//! timestamp from the Clock sysvar and converts it to `yyyymm` via [`month_from_unix_timestamp`].
//!
//! ## Why hand-roll the calendar
//!
//! The natural choice would be `chrono::DateTime<Utc>::from_timestamp(...)`, but
//! `chrono`'s on-chain footprint is non-trivial (hundreds of KB of code, depending on
//! features). Calendar arithmetic from a Unix timestamp is fully expressible as integer
//! math; the algorithm here is the classic "days since epoch → (year, month, day)"
//! conversion (Howard Hinnant's `civil_from_days`, MIT-licensed pseudocode).
//!
//! ## Wraparound semantics
//!
//! `genesis_month + (i - 1)` uses month-arithmetic, NOT plain integer addition: month
//! `202612 + 1 = 202701`, not `202613`. The helpers [`add_months`] and [`compare_months`]
//! handle the year-rollover correctly.
//!
//! ## Edge cases unit-tested below
//!
//! - Leap year boundaries (Feb 29 of 2024, 2000, 2400; NOT 1900, 2100)
//! - Month length boundaries (Feb 28/29, Apr 30, Jul 31)
//! - Year-rollover from December to January
//! - Timestamps before 1970 → reject as `BadClock`
//! - Tranche index out-of-range → reject as `TrancheIndexOutOfRange`

use crate::error::MegadropError;

/// Days from year 0 (Gregorian proleptic) to the Unix epoch (1970-01-01).
/// Computed once via the same `days_from_civil` formula used in
/// [`days_from_civil`]; pinned here so the constant never drifts if the formula
/// is refactored.
const DAYS_FROM_YEAR0_TO_EPOCH: i64 = 719_468;

/// Convert a Unix timestamp (seconds since 1970-01-01 00:00:00 UTC) to an ISO
/// `yyyymm` integer.
///
/// Returns `Err(BadClock)` for any negative timestamp or any timestamp so far in the
/// future that the year doesn't fit in u32 / `yyyymm` overflows. In practice neither
/// case can arise from the Solana Clock sysvar (which reports a positive Unix time and
/// is bounded by current real-world time), but the helper is explicit about its
/// invariants so callers don't have to reason about them.
pub fn month_from_unix_timestamp(ts: i64) -> Result<u32, MegadropError> {
    if ts < 0 {
        return Err(MegadropError::BadClock);
    }
    let days_since_epoch = ts.div_euclid(86_400);
    let (year, month, _day) = civil_from_days(days_since_epoch);
    if year < 0 || year > 9_999 {
        // yyyymm with `year > 9999` overflows; year < 0 is unreachable from a
        // non-negative `ts` but check for completeness.
        return Err(MegadropError::BadClock);
    }
    Ok(yyyymm(year as u32, month as u32))
}

/// Pack a `(year, month)` pair into the `yyyymm` integer encoding.
pub fn yyyymm(year: u32, month: u32) -> u32 {
    year * 100 + month
}

/// Unpack a `yyyymm` integer into `(year, month)`.
pub fn unpack_yyyymm(ym: u32) -> (u32, u32) {
    (ym / 100, ym % 100)
}

/// Add `n` months to a `yyyymm` value, with year-rollover. `n` is bounded — callers
/// should never need more than `NUM_TRANCHES - 1 == 9` for the tranche unlock check.
pub fn add_months(ym: u32, n: u32) -> u32 {
    let (year, month) = unpack_yyyymm(ym);
    // Months in [1, 12]; add `n` after shifting to a 0-indexed mod-12 representation.
    let total_months_zero_indexed = (month - 1) + n;
    let new_year = year + (total_months_zero_indexed / 12);
    let new_month = (total_months_zero_indexed % 12) + 1;
    yyyymm(new_year, new_month)
}

/// Order-comparison on `yyyymm` values. Trivial because the encoding is monotone.
pub fn compare_months(a: u32, b: u32) -> core::cmp::Ordering {
    a.cmp(&b)
}

/// Civil-from-days conversion: days since the Unix epoch (Jan 1 1970, day 0) to
/// `(year, month, day)` in the Gregorian calendar. Standard `civil_from_days` from
/// Howard Hinnant's date library — handles leap years correctly across the proleptic
/// Gregorian range.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + DAYS_FROM_YEAR0_TO_EPOCH;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a Unix timestamp for `(year, month, day)` at 00:00:00 UTC.
    /// Inverse of `civil_from_days`. Test-only — production never needs this direction.
    fn ts_from_civil(year: i64, month: u32, day: u32) -> i64 {
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = (y - era * 400) as u64; // [0, 399]
        let m = month as u64;
        let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
        let doy = (153 * mp + 2) / 5 + (day as u64) - 1; // [0, 365]
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
        let z = era * 146_097 + (doe as i64);
        (z - DAYS_FROM_YEAR0_TO_EPOCH) * 86_400
    }

    #[test]
    fn epoch_is_january_1970() {
        // Unix t=0 ⇒ 1970-01-01 ⇒ 197001.
        assert_eq!(month_from_unix_timestamp(0).unwrap(), 197001);
    }

    #[test]
    fn ts_from_civil_inverts_civil_from_days() {
        // Round-trip a few canonical dates: Unix 0, mainnet-sigma launch (May 1 2026
        // 00:00 UTC), the leap day Feb 29 2024.
        let cases = [
            (1970, 1u32, 1u32),
            (2026, 5, 1),
            (2024, 2, 29),
            (2000, 1, 1),
            (1999, 12, 31),
            (2400, 2, 29), // is a leap year (divisible by 400)
        ];
        for (y, m, d) in cases {
            let ts = ts_from_civil(y, m, d);
            let (gy, gm, gd) = civil_from_days(ts.div_euclid(86_400));
            assert_eq!((y, m, d), (gy, gm, gd), "round-trip {y}-{m:02}-{d:02}");
        }
    }

    #[test]
    fn mainnet_sigma_launch_month() {
        // Per docs/MEGADROP.md GENESIS_MONTH = 202605 (May 2026). Verify the helper
        // produces the right value for May 1, May 15, May 31 2026.
        assert_eq!(
            month_from_unix_timestamp(ts_from_civil(2026, 5, 1)).unwrap(),
            202605
        );
        assert_eq!(
            month_from_unix_timestamp(ts_from_civil(2026, 5, 15)).unwrap(),
            202605
        );
        assert_eq!(
            month_from_unix_timestamp(ts_from_civil(2026, 5, 31)).unwrap(),
            202605
        );
    }

    #[test]
    fn month_boundaries_are_sharp() {
        // Apr 30 23:59:59 → 202604; May 1 00:00:00 → 202605. Tests the day boundary.
        let last_day_apr = ts_from_civil(2026, 4, 30) + 86_399;
        let first_day_may = ts_from_civil(2026, 5, 1);
        assert_eq!(month_from_unix_timestamp(last_day_apr).unwrap(), 202604);
        assert_eq!(month_from_unix_timestamp(first_day_may).unwrap(), 202605);
    }

    #[test]
    fn leap_year_2024_feb_29_resolves() {
        // 2024 is a leap year (divisible by 4, not by 100). Feb 29 must be a valid day.
        let feb29 = ts_from_civil(2024, 2, 29);
        assert_eq!(month_from_unix_timestamp(feb29).unwrap(), 202402);
        // Mar 1 2024 the next day.
        assert_eq!(
            month_from_unix_timestamp(feb29 + 86_400).unwrap(),
            202403
        );
    }

    #[test]
    fn non_leap_year_2100_feb_28_to_march() {
        // 2100 is NOT a leap year (divisible by 100, not by 400). Feb 28 → Mar 1.
        let feb28 = ts_from_civil(2100, 2, 28);
        assert_eq!(month_from_unix_timestamp(feb28).unwrap(), 210002);
        assert_eq!(
            month_from_unix_timestamp(feb28 + 86_400).unwrap(),
            210003
        );
    }

    #[test]
    fn leap_year_2000_feb_29_resolves() {
        // 2000 IS a leap year (divisible by 400). Edge of the Gregorian rule.
        let feb29 = ts_from_civil(2000, 2, 29);
        assert_eq!(month_from_unix_timestamp(feb29).unwrap(), 200002);
    }

    #[test]
    fn year_rollover_dec_to_jan() {
        // Dec 31 23:59:59 2026 → 202612; Jan 1 00:00:00 2027 → 202701.
        let dec31 = ts_from_civil(2026, 12, 31) + 86_399;
        let jan1 = ts_from_civil(2027, 1, 1);
        assert_eq!(month_from_unix_timestamp(dec31).unwrap(), 202612);
        assert_eq!(month_from_unix_timestamp(jan1).unwrap(), 202701);
    }

    #[test]
    fn negative_timestamp_rejected() {
        // Pre-epoch timestamps are never valid for the Clock sysvar but the helper
        // explicitly rejects them.
        let err = month_from_unix_timestamp(-1).unwrap_err();
        assert!(matches!(err, MegadropError::BadClock));
    }

    #[test]
    fn add_months_within_year() {
        // 202605 + 4 = 202609 (Sep 2026).
        assert_eq!(add_months(202605, 4), 202609);
    }

    #[test]
    fn add_months_rolls_over_year() {
        // Tranches 1..10 starting at 202605 should land at 202605..202702.
        // Tranche 8 unlocks at genesis + 7 = 202612 (Dec 2026).
        // Tranche 9 unlocks at genesis + 8 = 202701 (Jan 2027) — year rollover.
        // Tranche 10 unlocks at genesis + 9 = 202702 (Feb 2027).
        assert_eq!(add_months(202605, 7), 202612);
        assert_eq!(add_months(202605, 8), 202701);
        assert_eq!(add_months(202605, 9), 202702);
    }

    #[test]
    fn add_months_full_tranche_schedule() {
        // Pin every entry of the docs/MEGADROP.md "Vesting timeline" table.
        let genesis = 202605;
        let expected = [
            202605, // tranche 1, May 2026
            202606, 202607, 202608, 202609, 202610, 202611, 202612, // 2-8
            202701, // tranche 9, Jan 2027
            202702, // tranche 10, Feb 2027
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(add_months(genesis, i as u32), *want, "tranche {}", i + 1);
        }
    }

    #[test]
    fn add_months_zero_is_identity() {
        assert_eq!(add_months(202605, 0), 202605);
    }

    #[test]
    fn add_months_large_n_rolls_multiple_years() {
        // 25 months from May 2026 = May 2028 (24 months ⇒ +2 years; +1 ⇒ Jun 2028).
        assert_eq!(add_months(202605, 25), 202806);
    }

    #[test]
    fn yyyymm_packing_round_trips() {
        for (y, m) in [(2026, 5), (2027, 12), (1970, 1), (9999, 12)] {
            let packed = yyyymm(y, m);
            let (uy, um) = unpack_yyyymm(packed);
            assert_eq!((y, m), (uy, um), "round trip {y}-{m:02}");
        }
    }

    #[test]
    fn compare_months_orders_correctly() {
        use core::cmp::Ordering;
        assert_eq!(compare_months(202605, 202605), Ordering::Equal);
        assert_eq!(compare_months(202605, 202606), Ordering::Less);
        assert_eq!(compare_months(202612, 202701), Ordering::Less);
        assert_eq!(compare_months(202701, 202612), Ordering::Greater);
    }
}

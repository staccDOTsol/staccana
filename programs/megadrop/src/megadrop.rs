//! Pure helpers for megadrop math + claim message construction.
//!
//! Mirrors the `staccana_bridge::attestation` and `staccana_validator_subsidy::subsidy`
//! patterns: every stateless calculation lives here so it can be unit-tested without
//! spinning up a local validator. The Anchor handler in
//! [`crate::instructions::claim_megadrop`] calls into these helpers for anything that
//! does not require account-info plumbing.
//!
//! Wire format reference: `docs/MEGADROP.md` (mechanism, vesting, claim ix). The
//! `STACCANA_MEGADROP_V1` byte format is canonical to this crate; the off-chain claim
//! CLI / frontend MUST produce identical bytes.

use crate::calendar::{add_months, compare_months};
use crate::error::MegadropError;
use crate::state::NUM_TRANCHES;

/// Domain-separation prefix for megadrop claim messages. v1 byte-pinned.
pub const CLAIM_DOMAIN: &[u8] = b"STACCANA_MEGADROP_V1";

/// Compute the per-tranche payout: `total / 10`.
///
/// Truncation residue (up to 9 lamports per holder) stays in the treasury — the same
/// pattern the validator-subsidy `bootstrap_per_epoch` uses. Sub-ten-lamport allocation
/// totals are dust and would round to zero per tranche; the snapshot tool should
/// arrange totals well above that range.
pub fn tranche_amount(total: u64) -> u64 {
    total / NUM_TRANCHES as u64
}

/// Compute the lamport amount payable for a set of tranches encoded as a 16-bit bitmap.
///
/// Bit `i` set ⇒ tranche `(i + 1)` is included. Returns `tranche_amount(total)`
/// multiplied by the popcount of the bitmap, fully checked for u64 overflow even
/// though the practical inputs are far below the danger zone.
pub fn compute_claim_amount(total: u64, tranche_bits: u16) -> Result<u64, MegadropError> {
    let per_tranche = tranche_amount(total);
    let count = tranche_bits.count_ones() as u64;
    per_tranche
        .checked_mul(count)
        .ok_or(MegadropError::ClaimAmountOverflow)
}

/// Test whether tranche `tranche_idx` (1..=10) is recorded as claimed in `bitmap`.
///
/// `tranche_idx` is 1-indexed to match the wire / spec naming
/// ("tranche 1, 2, ..., 10"); internally we use bit `tranche_idx - 1`. Returns
/// `Err(TrancheIndexOutOfRange)` if `tranche_idx == 0` or `> NUM_TRANCHES`.
pub fn is_tranche_claimed(bitmap: u16, tranche_idx: u8) -> Result<bool, MegadropError> {
    let bit = tranche_bit(tranche_idx)?;
    Ok(bitmap & (1u16 << bit) != 0)
}

/// Set the bit corresponding to `tranche_idx` (1..=10) in `bitmap` and return the
/// new value. Idempotent if already set.
pub fn set_tranche_claimed(bitmap: u16, tranche_idx: u8) -> Result<u16, MegadropError> {
    let bit = tranche_bit(tranche_idx)?;
    Ok(bitmap | (1u16 << bit))
}

/// Has tranche `tranche_idx` (1..=10) unlocked at `current_month`, given the chain's
/// `genesis_month`?
///
/// Tranche `i` unlocks when `current_month >= genesis_month + (i - 1)`. Tranche 1
/// therefore unlocks at `genesis_month`; tranche 10 at `genesis_month + 9`. Calendar
/// arithmetic is delegated to [`crate::calendar::add_months`] so year-rollover is
/// handled correctly.
pub fn is_tranche_unlocked(
    genesis_month: u32,
    current_month: u32,
    tranche_idx: u8,
) -> Result<bool, MegadropError> {
    if tranche_idx == 0 || tranche_idx > NUM_TRANCHES {
        return Err(MegadropError::TrancheIndexOutOfRange);
    }
    let unlock_month = add_months(genesis_month, (tranche_idx - 1) as u32);
    Ok(matches!(
        compare_months(current_month, unlock_month),
        core::cmp::Ordering::Equal | core::cmp::Ordering::Greater
    ))
}

/// Build the canonical claim message that the holder signs.
///
/// Layout: `b"STACCANA_MEGADROP_V1" || holder_pubkey || total_allocation_le ||
/// sorted_tranches_packed || program_id`. The tranche list is sorted ascending and
/// packed as a length-prefixed sequence of u8 — see SPEC §4.2 / `docs/MEGADROP.md`
/// "Claim instruction" for the rationale on having `program_id` in the signed
/// preimage (binds the signature to this program; replaying on a fork with a
/// different program id won't authorize a claim).
///
/// Total length: 20 (domain) + 32 (pubkey) + 8 (total) + 1 (n_tranches) + n_tranches
/// (one byte each) + 32 (program_id).
pub fn build_claim_message(
    holder: &[u8; 32],
    total_allocation: u64,
    sorted_tranches: &[u8],
    program_id: &[u8; 32],
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(
        CLAIM_DOMAIN.len() + 32 + 8 + 1 + sorted_tranches.len() + 32,
    );
    msg.extend_from_slice(CLAIM_DOMAIN);
    msg.extend_from_slice(holder);
    msg.extend_from_slice(&total_allocation.to_le_bytes());
    msg.push(sorted_tranches.len() as u8);
    msg.extend_from_slice(sorted_tranches);
    msg.extend_from_slice(program_id);
    msg
}

/// Validate a list of requested tranche indices and pack them into a 16-bit bitmap.
///
/// Rejects:
/// - empty list (would be a no-op claim)
/// - any index outside `[1, NUM_TRANCHES]`
/// - any duplicate index
///
/// Returns `(sorted_indices, bitmap)`. The returned `sorted_indices` is byte-equal to
/// what should appear in the signed message preimage, so the caller doesn't have to
/// re-sort separately.
pub fn validate_and_pack_tranches(
    requested: &[u8],
) -> Result<(Vec<u8>, u16), MegadropError> {
    if requested.is_empty() {
        return Err(MegadropError::EmptyTrancheList);
    }
    let mut bitmap: u16 = 0;
    for &idx in requested {
        let bit = tranche_bit(idx)?;
        let mask = 1u16 << bit;
        if bitmap & mask != 0 {
            return Err(MegadropError::DuplicateTrancheIndex);
        }
        bitmap |= mask;
    }
    let mut sorted = requested.to_vec();
    sorted.sort_unstable();
    Ok((sorted, bitmap))
}

/// Map a 1-indexed tranche to its bit position (0-indexed). Rejects out-of-range.
fn tranche_bit(tranche_idx: u8) -> Result<u8, MegadropError> {
    if tranche_idx == 0 || tranche_idx > NUM_TRANCHES {
        return Err(MegadropError::TrancheIndexOutOfRange);
    }
    Ok(tranche_idx - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- tranche_amount ------------------------------------------------------

    #[test]
    fn tranche_amount_divides_by_ten() {
        assert_eq!(tranche_amount(1_000), 100);
        assert_eq!(tranche_amount(10_000_000_000), 1_000_000_000);
    }

    #[test]
    fn tranche_amount_truncates_residue() {
        // 999 / 10 = 99 (residue 9); residue stays in treasury.
        assert_eq!(tranche_amount(999), 99);
    }

    #[test]
    fn tranche_amount_handles_zero() {
        assert_eq!(tranche_amount(0), 0);
    }

    #[test]
    fn tranche_amount_dust_input_rounds_to_zero() {
        // < 10 lamports per holder is below the per-tranche resolution. Document the
        // behavior so a bug in the snapshot tool that produces dust allocations
        // surfaces as "no payout" rather than a panic.
        assert_eq!(tranche_amount(9), 0);
    }

    #[test]
    fn tranche_amount_large_total() {
        // 300M SOL = 300M * 1e9 lamports = 3e17 ⇒ /10 = 3e16. Comfortably under u64::MAX.
        let total = 300_000_000_000_000_000u64;
        assert_eq!(tranche_amount(total), 30_000_000_000_000_000);
    }

    // -- compute_claim_amount ------------------------------------------------

    #[test]
    fn compute_claim_amount_single_tranche() {
        let total = 1_000u64;
        let bitmap: u16 = 0b0000_0000_0000_0001; // tranche 1
        assert_eq!(compute_claim_amount(total, bitmap).unwrap(), 100);
    }

    #[test]
    fn compute_claim_amount_three_tranches() {
        let total = 1_000u64;
        // Tranches 1, 3, 5 — popcount = 3.
        let bitmap: u16 = 0b0000_0000_0001_0101;
        assert_eq!(compute_claim_amount(total, bitmap).unwrap(), 300);
    }

    #[test]
    fn compute_claim_amount_full_vesting() {
        // All 10 tranches — popcount = 10 — should equal total (modulo truncation).
        let total = 1_000u64;
        let bitmap: u16 = 0b0000_0011_1111_1111;
        assert_eq!(compute_claim_amount(total, bitmap).unwrap(), 1_000);
    }

    #[test]
    fn compute_claim_amount_full_vesting_with_residue() {
        // Total has truncation residue; full vesting pays out total - residue.
        let total = 999u64; // per-tranche = 99; popcount = 10 ⇒ 990
        let bitmap: u16 = 0b0000_0011_1111_1111;
        assert_eq!(compute_claim_amount(total, bitmap).unwrap(), 990);
    }

    #[test]
    fn compute_claim_amount_zero_bitmap() {
        // Empty bitmap ⇒ zero payout, no error.
        assert_eq!(compute_claim_amount(1_000, 0).unwrap(), 0);
    }

    #[test]
    fn compute_claim_amount_overflow_caught() {
        // u64::MAX per-tranche * 10 = overflow. Synthesize by setting all 10 bits
        // and a per-tranche that, when multiplied by 10, overflows.
        // tranche_amount(u64::MAX) = u64::MAX / 10. count_ones max = 16 (any u16).
        // Need per_tranche * count > u64::MAX. Use a contrived scenario:
        // Setting all 16 bits (impossible from valid validate_and_pack output but
        // possible if `bitmap` is forged): 16 * (u64::MAX / 10) overflows.
        let bitmap: u16 = u16::MAX;
        let total = u64::MAX;
        let err = compute_claim_amount(total, bitmap).unwrap_err();
        assert!(matches!(err, MegadropError::ClaimAmountOverflow));
    }

    // -- bitmap helpers ------------------------------------------------------

    #[test]
    fn is_tranche_claimed_basic() {
        let bitmap: u16 = 0b0000_0000_0000_0101; // tranches 1 and 3
        assert!(is_tranche_claimed(bitmap, 1).unwrap());
        assert!(!is_tranche_claimed(bitmap, 2).unwrap());
        assert!(is_tranche_claimed(bitmap, 3).unwrap());
        assert!(!is_tranche_claimed(bitmap, 10).unwrap());
    }

    #[test]
    fn is_tranche_claimed_at_max_index() {
        let bitmap: u16 = 1u16 << 9; // bit 9 ⇒ tranche 10
        assert!(is_tranche_claimed(bitmap, 10).unwrap());
        assert!(!is_tranche_claimed(bitmap, 9).unwrap());
    }

    #[test]
    fn is_tranche_claimed_rejects_zero() {
        let err = is_tranche_claimed(0, 0).unwrap_err();
        assert!(matches!(err, MegadropError::TrancheIndexOutOfRange));
    }

    #[test]
    fn is_tranche_claimed_rejects_above_ten() {
        let err = is_tranche_claimed(0, 11).unwrap_err();
        assert!(matches!(err, MegadropError::TrancheIndexOutOfRange));
    }

    #[test]
    fn set_tranche_claimed_round_trip() {
        let mut bitmap: u16 = 0;
        for i in 1..=NUM_TRANCHES {
            bitmap = set_tranche_claimed(bitmap, i).unwrap();
            assert!(is_tranche_claimed(bitmap, i).unwrap());
        }
        // After setting all 10, popcount must be 10.
        assert_eq!(bitmap.count_ones(), 10);
    }

    #[test]
    fn set_tranche_claimed_is_idempotent() {
        let bitmap = set_tranche_claimed(0, 5).unwrap();
        let again = set_tranche_claimed(bitmap, 5).unwrap();
        assert_eq!(bitmap, again);
    }

    #[test]
    fn set_tranche_claimed_rejects_out_of_range() {
        assert!(set_tranche_claimed(0, 0).is_err());
        assert!(set_tranche_claimed(0, 11).is_err());
        assert!(set_tranche_claimed(0, 255).is_err());
    }

    // -- unlock check --------------------------------------------------------

    #[test]
    fn unlock_at_genesis_month_for_first_tranche() {
        // Tranche 1 unlocks at genesis_month exactly.
        assert!(is_tranche_unlocked(202605, 202605, 1).unwrap());
    }

    #[test]
    fn unlock_one_month_late_for_first_tranche() {
        // Tranche 1 still claimable in month 2 — this isn't an expiry.
        assert!(is_tranche_unlocked(202605, 202606, 1).unwrap());
    }

    #[test]
    fn unlock_blocked_before_genesis() {
        // Pre-genesis: tranche 1 not unlocked.
        assert!(!is_tranche_unlocked(202605, 202604, 1).unwrap());
    }

    #[test]
    fn unlock_tenth_tranche_at_correct_month() {
        // Tranche 10 unlocks at genesis + 9 = Feb 2027 (per docs/MEGADROP.md table).
        assert!(is_tranche_unlocked(202605, 202702, 10).unwrap());
        assert!(!is_tranche_unlocked(202605, 202701, 10).unwrap());
    }

    #[test]
    fn unlock_with_year_rollover() {
        // Tranche 9 unlocks at Jan 2027; verify the year-rollover gate.
        assert!(is_tranche_unlocked(202605, 202701, 9).unwrap());
        assert!(!is_tranche_unlocked(202605, 202612, 9).unwrap());
    }

    #[test]
    fn unlock_rejects_zero_index() {
        assert!(is_tranche_unlocked(202605, 202605, 0).is_err());
    }

    #[test]
    fn unlock_rejects_above_max() {
        assert!(is_tranche_unlocked(202605, 202605, 11).is_err());
    }

    // -- build_claim_message -------------------------------------------------

    #[test]
    fn claim_message_layout_pinned() {
        let holder = [0xABu8; 32];
        let total: u64 = 0x0102_0304_0506_0708;
        let tranches = vec![1u8, 2, 5];
        let program_id = [0xCDu8; 32];

        let msg = build_claim_message(&holder, total, &tranches, &program_id);

        // 20 + 32 + 8 + 1 + 3 + 32 = 96.
        assert_eq!(msg.len(), 96);
        assert_eq!(&msg[0..20], CLAIM_DOMAIN);
        assert_eq!(&msg[20..52], &holder);
        assert_eq!(&msg[52..60], &total.to_le_bytes());
        assert_eq!(msg[60], 3); // n_tranches
        assert_eq!(&msg[61..64], &tranches[..]);
        assert_eq!(&msg[64..96], &program_id);
    }

    #[test]
    fn claim_message_domain_is_exact_ascii() {
        // Pin the domain bytes — catches a "_V2" / casing slip in code review.
        assert_eq!(CLAIM_DOMAIN, b"STACCANA_MEGADROP_V1");
        assert_eq!(CLAIM_DOMAIN.len(), 20);
    }

    #[test]
    fn claim_message_changes_for_each_field() {
        // Flip each field; confirm the bytes differ. Smoke-tests that no field is
        // silently dropped from the message.
        let h0 = [0u8; 32];
        let h1 = [1u8; 32];
        let pid0 = [10u8; 32];
        let pid1 = [11u8; 32];
        let base = build_claim_message(&h0, 100, &[1, 2], &pid0);

        assert_ne!(base, build_claim_message(&h1, 100, &[1, 2], &pid0));
        assert_ne!(base, build_claim_message(&h0, 200, &[1, 2], &pid0));
        assert_ne!(base, build_claim_message(&h0, 100, &[3, 2], &pid0));
        assert_ne!(base, build_claim_message(&h0, 100, &[1, 2, 3], &pid0));
        assert_ne!(base, build_claim_message(&h0, 100, &[1, 2], &pid1));
    }

    #[test]
    fn claim_message_full_ten_tranche_request() {
        // Realistic worst case: holder requests all 10 tranches in one ix.
        let holder = [0u8; 32];
        let pid = [0u8; 32];
        let tranches: Vec<u8> = (1u8..=10).collect();
        let msg = build_claim_message(&holder, 1_000_000, &tranches, &pid);
        // 20 + 32 + 8 + 1 + 10 + 32 = 103.
        assert_eq!(msg.len(), 103);
        assert_eq!(msg[60], 10);
    }

    // -- validate_and_pack_tranches -----------------------------------------

    #[test]
    fn validate_and_pack_single_tranche() {
        let (sorted, bitmap) = validate_and_pack_tranches(&[5]).unwrap();
        assert_eq!(sorted, vec![5]);
        assert_eq!(bitmap, 1u16 << 4);
    }

    #[test]
    fn validate_and_pack_sorts_input() {
        let (sorted, _) = validate_and_pack_tranches(&[5, 1, 3]).unwrap();
        assert_eq!(sorted, vec![1, 3, 5]);
    }

    #[test]
    fn validate_and_pack_full_set() {
        let input: Vec<u8> = (1u8..=10).collect();
        let (sorted, bitmap) = validate_and_pack_tranches(&input).unwrap();
        assert_eq!(sorted, input);
        assert_eq!(bitmap, 0b0000_0011_1111_1111);
        assert_eq!(bitmap.count_ones(), 10);
    }

    #[test]
    fn validate_and_pack_rejects_empty() {
        let err = validate_and_pack_tranches(&[]).unwrap_err();
        assert!(matches!(err, MegadropError::EmptyTrancheList));
    }

    #[test]
    fn validate_and_pack_rejects_zero_index() {
        let err = validate_and_pack_tranches(&[1, 0, 3]).unwrap_err();
        assert!(matches!(err, MegadropError::TrancheIndexOutOfRange));
    }

    #[test]
    fn validate_and_pack_rejects_above_max() {
        let err = validate_and_pack_tranches(&[1, 11]).unwrap_err();
        assert!(matches!(err, MegadropError::TrancheIndexOutOfRange));
    }

    #[test]
    fn validate_and_pack_rejects_duplicate() {
        let err = validate_and_pack_tranches(&[1, 2, 2]).unwrap_err();
        assert!(matches!(err, MegadropError::DuplicateTrancheIndex));
    }

    #[test]
    fn validate_and_pack_back_tranches_scenario() {
        // Realistic: holder dormant for months 1-5, wakes up in month 5 and claims
        // all back-tranches. Per docs/MEGADROP.md "Vesting" — supported behavior.
        let (sorted, bitmap) = validate_and_pack_tranches(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(sorted, vec![1, 2, 3, 4, 5]);
        assert_eq!(bitmap.count_ones(), 5);
    }
}

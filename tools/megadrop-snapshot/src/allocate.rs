//! Apply the megadrop allocation model to per-holder cohort counts.
//!
//! Per `docs/MEGADROP.md` "Allocation parameters":
//!
//! ```text
//! per_holder_weight = w_based × f(based_stacc_0_count) + w_proofv3 × f(proofv3_balance)
//! per_holder_alloc  = total_megadrop_lamports × per_holder_weight / sum_of_all_weights
//! ```
//!
//! Where `f(x)` is determined by the allocation model:
//! - `uniform`: `1 if x > 0 else 0` (per holder, regardless of count)
//! - `linear`:  `x` (count or balance, raw)
//! - `sqrt`:    `floor(sqrt(x))` — a 100-unit holder gets ~10x a 1-unit holder, not 100x
//!
//! All allocation math is in u128 to avoid overflow at the production scales
//! (300M SOL = 3e17 lamports; tens of thousands of holders).

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

use crate::snapshot::HolderEntry;

/// Allocation model selector. CLI maps `--allocation-model` to this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[clap(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum AllocationModel {
    /// One vote per holder, regardless of count or balance.
    Uniform,
    /// Linear in count / balance.
    Linear,
    /// Square root of count / balance — the lean per the doc.
    Sqrt,
}

/// Final per-holder allocation breakdown — what `output::write_outputs` serializes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderAllocation {
    /// Holder pubkey (base58 in JSON, raw in tests).
    pub holder: Pubkey,
    /// Raw NFT count from `based_stacc_0`.
    pub based_stacc_0_count: u64,
    /// Raw token balance from `proofv3` (no decimal interpretation).
    pub proofv3_balance: u64,
    /// `f(based_stacc_0_count)` per the allocation model.
    pub based_weight: u128,
    /// `f(proofv3_balance)` per the allocation model.
    pub proofv3_weight: u128,
    /// Combined holder weight: `w_based × based_weight + w_proofv3 × proofv3_weight`.
    pub total_weight: u128,
    /// Final lamport allocation for this holder.
    pub allocation_lamports: u64,
}

/// Apply the allocation model `f(x)` to a raw count.
///
/// `sqrt` uses `u128::isqrt` (added in Rust 1.84) for an exact integer floor — no
/// floating-point rounding ambiguity.
pub fn apply_model(model: AllocationModel, raw: u64) -> u128 {
    let raw = raw as u128;
    match model {
        AllocationModel::Uniform => {
            if raw > 0 {
                1
            } else {
                0
            }
        }
        AllocationModel::Linear => raw,
        AllocationModel::Sqrt => integer_sqrt_u128(raw),
    }
}

/// Integer square root (truncated). Pure function — no allocations, no FP. Pinned
/// here rather than calling `u128::isqrt` directly so the build doesn't require
/// nightly / a 1.84+ toolchain (the workspace pins edition 2021 but rustc version
/// can be older).
pub fn integer_sqrt_u128(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    // Newton's method seeded with a power-of-two upper bound.
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Compute per-holder allocations for the full cohort.
///
/// `total_megadrop_lamports` — the SOL pool to distribute (e.g. 300M SOL = 3e17 lamports).
/// `weight_based`, `weight_proofv3` — the cohort-level weights from the spec
/// (default 60 + 40 of 100). The two values aren't required to sum to 100 — they're
/// just relative weights — but matching the spec's percentage convention keeps things
/// legible.
///
/// Truncation residue (≤ N lamports for N holders) stays in the treasury; the sum of
/// per-holder `allocation_lamports` may underrun `total_megadrop_lamports` by a few
/// lamports per holder.
pub fn compute_allocations(
    holders: &[HolderEntry],
    model: AllocationModel,
    weight_based: u32,
    weight_proofv3: u32,
    total_megadrop_lamports: u64,
) -> Vec<HolderAllocation> {
    if holders.is_empty() {
        return Vec::new();
    }

    let w_based = weight_based as u128;
    let w_proofv3 = weight_proofv3 as u128;

    // Pass 1: per-holder weights and grand total.
    let mut staged: Vec<(Pubkey, u64, u64, u128, u128, u128)> =
        Vec::with_capacity(holders.len());
    let mut grand_total_weight: u128 = 0;
    for h in holders {
        let bw = apply_model(model, h.based_stacc_0_count);
        let pw = apply_model(model, h.proofv3_balance);
        let total = w_based.saturating_mul(bw).saturating_add(w_proofv3.saturating_mul(pw));
        grand_total_weight = grand_total_weight.saturating_add(total);
        staged.push((
            h.holder,
            h.based_stacc_0_count,
            h.proofv3_balance,
            bw,
            pw,
            total,
        ));
    }

    // Pass 2: per-holder allocation share.
    let mut out: Vec<HolderAllocation> = Vec::with_capacity(staged.len());
    let pool = total_megadrop_lamports as u128;
    for (holder, base_count, proof_balance, bw, pw, total_w) in staged {
        let allocation: u64 = if grand_total_weight == 0 {
            0
        } else {
            // share = pool × total_w / grand_total_weight, clamped to u64.
            let numer = pool.saturating_mul(total_w);
            let share = numer / grand_total_weight;
            u64::try_from(share).unwrap_or(u64::MAX)
        };
        out.push(HolderAllocation {
            holder,
            based_stacc_0_count: base_count,
            proofv3_balance: proof_balance,
            based_weight: bw,
            proofv3_weight: pw,
            total_weight: total_w,
            allocation_lamports: allocation,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn entry(holder: Pubkey, based: u64, proof: u64) -> HolderEntry {
        HolderEntry {
            holder,
            based_stacc_0_count: based,
            proofv3_balance: proof,
        }
    }

    #[test]
    fn integer_sqrt_basic_values() {
        assert_eq!(integer_sqrt_u128(0), 0);
        assert_eq!(integer_sqrt_u128(1), 1);
        assert_eq!(integer_sqrt_u128(4), 2);
        assert_eq!(integer_sqrt_u128(9), 3);
        assert_eq!(integer_sqrt_u128(16), 4);
        assert_eq!(integer_sqrt_u128(100), 10);
        assert_eq!(integer_sqrt_u128(10_000), 100);
        assert_eq!(integer_sqrt_u128(1_000_000), 1_000);
    }

    #[test]
    fn integer_sqrt_truncates() {
        assert_eq!(integer_sqrt_u128(2), 1); // floor(sqrt(2)) = 1
        assert_eq!(integer_sqrt_u128(8), 2); // floor(sqrt(8)) = 2
        assert_eq!(integer_sqrt_u128(99), 9); // floor(sqrt(99)) = 9
    }

    #[test]
    fn integer_sqrt_handles_large_values() {
        let n = (u64::MAX as u128) - 1;
        let r = integer_sqrt_u128(n);
        // r * r <= n < (r+1) * (r+1)
        assert!(r.saturating_mul(r) <= n);
        let next = r + 1;
        assert!(next.saturating_mul(next) > n);
    }

    #[test]
    fn apply_model_uniform_is_indicator() {
        assert_eq!(apply_model(AllocationModel::Uniform, 0), 0);
        assert_eq!(apply_model(AllocationModel::Uniform, 1), 1);
        assert_eq!(apply_model(AllocationModel::Uniform, 100), 1);
    }

    #[test]
    fn apply_model_linear_is_passthrough() {
        assert_eq!(apply_model(AllocationModel::Linear, 0), 0);
        assert_eq!(apply_model(AllocationModel::Linear, 100), 100);
    }

    #[test]
    fn apply_model_sqrt_dampens_large_holdings() {
        // 100x holder gets only 10x the weight under sqrt.
        let small = apply_model(AllocationModel::Sqrt, 1);
        let large = apply_model(AllocationModel::Sqrt, 100);
        assert_eq!(small, 1);
        assert_eq!(large, 10);
    }

    #[test]
    fn empty_holders_produces_empty_output() {
        let got = compute_allocations(
            &[],
            AllocationModel::Sqrt,
            60,
            40,
            1_000_000,
        );
        assert!(got.is_empty());
    }

    #[test]
    fn single_holder_gets_entire_pool() {
        let holders = vec![entry(pk(1), 1, 0)];
        let got = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            1_000_000,
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].allocation_lamports, 1_000_000);
    }

    #[test]
    fn equal_weight_holders_split_evenly() {
        let holders = vec![entry(pk(1), 1, 0), entry(pk(2), 1, 0)];
        let got = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            1_000_000,
        );
        assert_eq!(got[0].allocation_lamports, 500_000);
        assert_eq!(got[1].allocation_lamports, 500_000);
    }

    #[test]
    fn linear_model_proportional_split() {
        // Holder A has 1 NFT, holder B has 3 NFTs ⇒ 1:3 split.
        let holders = vec![entry(pk(1), 1, 0), entry(pk(2), 3, 0)];
        let got = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            400,
        );
        assert_eq!(got[0].allocation_lamports, 100); // 400 * 1/4
        assert_eq!(got[1].allocation_lamports, 300); // 400 * 3/4
    }

    #[test]
    fn sqrt_model_dampens_concentration() {
        // Holder A = 1 NFT (sqrt=1), holder B = 100 NFTs (sqrt=10) ⇒ 1:10 split,
        // not 1:100. Pool = 1100; A gets 100, B gets 1000.
        let holders = vec![entry(pk(1), 1, 0), entry(pk(2), 100, 0)];
        let got = compute_allocations(
            &holders,
            AllocationModel::Sqrt,
            100,
            0,
            1100,
        );
        assert_eq!(got[0].allocation_lamports, 100);
        assert_eq!(got[1].allocation_lamports, 1000);
    }

    #[test]
    fn uniform_model_one_per_holder_regardless_of_count() {
        let holders = vec![entry(pk(1), 1, 0), entry(pk(2), 1_000_000, 0)];
        let got = compute_allocations(
            &holders,
            AllocationModel::Uniform,
            100,
            0,
            1_000,
        );
        assert_eq!(got[0].allocation_lamports, 500);
        assert_eq!(got[1].allocation_lamports, 500);
    }

    #[test]
    fn cross_cohort_weights_combine() {
        // Holder owns 100 NFTs (sqrt=10) AND 10000 tokens (sqrt=100).
        // weights: 60×10 + 40×100 = 600 + 4000 = 4600.
        // Compare against a pure-NFT holder with same NFT count: 60×10 = 600.
        // Ratio: 4600 / (4600 + 600) = 4600/5200 ≈ 88.46%.
        let holders = vec![
            entry(pk(1), 100, 10_000),
            entry(pk(2), 100, 0),
        ];
        let got = compute_allocations(
            &holders,
            AllocationModel::Sqrt,
            60,
            40,
            5_200,
        );
        assert_eq!(got[0].allocation_lamports, 4_600);
        assert_eq!(got[1].allocation_lamports, 600);
    }

    #[test]
    fn cohort_weight_zero_excludes_cohort() {
        // weight_proofv3 = 0 ⇒ only NFT count matters.
        let holders = vec![
            entry(pk(1), 1, 1_000_000), // big proofv3 holder, 1 NFT
            entry(pk(2), 1, 0),         // pure NFT holder, 1 NFT
        ];
        let got = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            1_000_000,
        );
        // Both have 1 NFT and proof weight is ignored ⇒ 50/50 split.
        assert_eq!(got[0].allocation_lamports, 500_000);
        assert_eq!(got[1].allocation_lamports, 500_000);
    }

    #[test]
    fn zero_weight_holder_gets_zero() {
        // Holder with zero count in both cohorts contributes nothing and gets nothing.
        let holders = vec![
            entry(pk(1), 1, 0),
            entry(pk(2), 0, 0), // empty
        ];
        let got = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            100,
            1_000,
        );
        assert_eq!(got[0].allocation_lamports, 1_000);
        assert_eq!(got[1].allocation_lamports, 0);
    }

    #[test]
    fn truncation_residue_stays_under_total() {
        // 3 equal-weight holders dividing 1000 → 333 each, 1 lamport residue.
        let holders = vec![
            entry(pk(1), 1, 0),
            entry(pk(2), 1, 0),
            entry(pk(3), 1, 0),
        ];
        let got = compute_allocations(
            &holders,
            AllocationModel::Linear,
            100,
            0,
            1_000,
        );
        let sum: u64 = got.iter().map(|h| h.allocation_lamports).sum();
        assert!(sum <= 1_000);
        for h in &got {
            assert_eq!(h.allocation_lamports, 333);
        }
    }

    #[test]
    fn realistic_300m_sol_pool_doesnt_overflow() {
        // 300M SOL = 3e17 lamports. Pretend 5 holders to confirm we stay in u128
        // bounds with the spec's default weights.
        let holders = vec![
            entry(pk(1), 100, 1_000_000),
            entry(pk(2), 50, 500_000),
            entry(pk(3), 1, 10_000),
            entry(pk(4), 200, 0),
            entry(pk(5), 0, 100_000),
        ];
        let pool = 300_000_000u64.saturating_mul(1_000_000_000);
        let got = compute_allocations(&holders, AllocationModel::Sqrt, 60, 40, pool);
        let sum: u128 = got.iter().map(|h| h.allocation_lamports as u128).sum();
        // Sum should be very close to (but at most equal to) the pool.
        assert!(sum <= pool as u128);
        // Within 5 lamports per holder of slack should be more than enough.
        assert!(pool as u128 - sum < 5 * holders.len() as u128);
    }
}

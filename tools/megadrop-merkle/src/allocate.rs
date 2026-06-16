//! Linear allocation policy with exact-total normalization.
//!
//! ## Policy (per `docs/MEGADROP.md` "Allocation parameters" + the operator's CLI
//!   `--base-allocation-sol`, `--per-nft-bonus-sol`, `--per-token-bonus-sol-per-million`)
//!
//! 1. **Raw allocation**: each holder gets a "raw lamport" budget computed as
//!    ```text
//!    raw = (base_sol
//!         + nft_count             * per_nft_bonus_sol
//!         + (token_balance / 1e6) * per_token_bonus_sol)
//!         * 1_000_000_000
//!    ```
//!    Computed in u128 so a whale with 100M proofv3 tokens can't overflow the
//!    intermediate.
//!
//! 2. **Normalize**: scale all raws so they sum to exactly `target_total_lamports =
//!    total_allocation_sol * 1_000_000_000`. Truncated division produces a few
//!    lamports of residue.
//!
//! 3. **Distribute residue**: the residue (in lamports) is added to the top-N
//!    holders (where N == residue) by descending raw allocation. This guarantees the
//!    final sum exactly equals `target_total_lamports` while keeping the rounding
//!    bias deterministic and easy to audit.
//!
//! ## Why "exact total"
//!
//! The on-chain `MegadropConfig.total_allocation` field is the operator's auditable
//! commitment to "we set aside exactly X SOL for this round". If the per-leaf
//! lamport sum drifts from that field even by 1 lamport, claim verification still
//! works (the bridge doesn't enforce it), but reconciliation reports look broken.
//! The exact-total guarantee is cheap (linear-time top-N pick) and operator-friendly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;

/// Allocation policy parameters from the CLI. All units are in SOL (the CLI does the
/// conversion to lamports internally where needed).
#[derive(Clone, Copy, Debug)]
pub struct AllocationParams {
    /// `--total-allocation-sol`: the round's total pool, in SOL. Multiplied by 1e9
    /// to get lamports.
    pub total_allocation_sol: u64,
    /// `--base-allocation-sol`: a fixed amount every eligible holder receives, in
    /// SOL. Sets the floor.
    pub base_allocation_sol: u64,
    /// `--per-nft-bonus-sol`: per-NFT bonus for `based_stacc_0` holders, in SOL.
    pub per_nft_bonus_sol: u64,
    /// `--per-token-bonus-sol-per-million`: per-million-token bonus for `proofv3`
    /// holders, in SOL. So a holder with 1M proofv3 tokens gets exactly this bonus.
    pub per_token_bonus_sol_per_million: u64,
}

/// What this holder contributed (NFT count + token balance) — preserved on the output
/// row for audit reporting.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HolderContributions {
    pub nft_count: u64,
    pub token_balance: u64,
}

/// One holder's final allocation. The Merkle tree is built from
/// `(owner, lamports)`; `nft_count` and `token_balance` are echoed on the output
/// rows for the operator's audit log.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HolderAllocation {
    pub owner: Pubkey,
    pub lamports: u64,
    pub contributions: HolderContributions,
}

/// Compute per-holder allocations given grouped inputs and the policy.
///
/// `nft_counts` and `token_balances` are the per-owner aggregates produced by
/// `input::load_*_holders`. The two sets are unioned by owner; an owner present in
/// only one set gets `0` for the other field.
///
/// Holders with zero contributions in BOTH cohorts are excluded entirely (their
/// raw allocation would equal `base_allocation_sol` * 1e9 — but if both counts are
/// zero they aren't really eligible). To include floor-only allocations, pre-populate
/// the input maps with zero values for those owners.
pub fn compute_allocations(
    nft_counts: &BTreeMap<Pubkey, u64>,
    token_balances: &BTreeMap<Pubkey, u64>,
    params: AllocationParams,
) -> Vec<HolderAllocation> {
    // Union of all owners present in either cohort.
    let mut owners: BTreeMap<Pubkey, HolderContributions> = BTreeMap::new();
    for (owner, count) in nft_counts {
        owners
            .entry(*owner)
            .or_insert(HolderContributions {
                nft_count: 0,
                token_balance: 0,
            })
            .nft_count = *count;
    }
    for (owner, bal) in token_balances {
        owners
            .entry(*owner)
            .or_insert(HolderContributions {
                nft_count: 0,
                token_balance: 0,
            })
            .token_balance = *bal;
    }

    if owners.is_empty() {
        return Vec::new();
    }

    let target_total_lamports: u128 =
        (params.total_allocation_sol as u128).saturating_mul(1_000_000_000);
    if target_total_lamports == 0 {
        // Operator passed `--total-allocation-sol 0` — emit zero-lamport rows so the
        // shape of the output is preserved (downstream tooling expects a row per
        // holder).
        return owners
            .into_iter()
            .map(|(owner, contrib)| HolderAllocation {
                owner,
                lamports: 0,
                contributions: contrib,
            })
            .collect();
    }

    // Pass 1: per-holder raw lamport-equivalent score.
    let raw_per_holder: Vec<(Pubkey, HolderContributions, u128)> = owners
        .into_iter()
        .map(|(owner, contrib)| {
            let raw_sol: u128 = (params.base_allocation_sol as u128)
                .saturating_add(
                    (contrib.nft_count as u128).saturating_mul(params.per_nft_bonus_sol as u128),
                )
                .saturating_add(
                    // token_balance / 1_000_000 gives the "millions of tokens"
                    // factor; we use integer division (floor) which mirrors the
                    // operator's intuition that a 999,999-balance holder gets the
                    // same per-token bonus as a 0-balance holder. This is part of
                    // the policy choice — sub-million holders contribute zero
                    // token-bonus.
                    ((contrib.token_balance as u128) / 1_000_000)
                        .saturating_mul(params.per_token_bonus_sol_per_million as u128),
                );
            let raw_lamports = raw_sol.saturating_mul(1_000_000_000);
            (owner, contrib, raw_lamports)
        })
        .collect();

    let raw_sum: u128 = raw_per_holder
        .iter()
        .fold(0u128, |acc, (_, _, r)| acc.saturating_add(*r));

    if raw_sum == 0 {
        // All holders have zero raw allocation (e.g. base=0, no NFT/token weights).
        // Distribute zeros and exit.
        return raw_per_holder
            .into_iter()
            .map(|(owner, contrib, _)| HolderAllocation {
                owner,
                lamports: 0,
                contributions: contrib,
            })
            .collect();
    }

    // Pass 2: scale each raw to the target total via truncated division.
    let mut allocations: Vec<HolderAllocation> = raw_per_holder
        .iter()
        .map(|(owner, contrib, raw)| {
            // share = target * raw / raw_sum, truncated. u128 intermediate.
            let scaled = target_total_lamports.saturating_mul(*raw) / raw_sum;
            HolderAllocation {
                owner: *owner,
                lamports: u64::try_from(scaled).unwrap_or(u64::MAX),
                contributions: *contrib,
            }
        })
        .collect();

    // Pass 3: distribute the truncation residue to the top-N holders by raw
    // allocation. Residue is bounded by `raw_per_holder.len()` lamports (each
    // truncation drops < 1 lamport).
    let assigned_sum: u128 = allocations
        .iter()
        .fold(0u128, |acc, a| acc.saturating_add(a.lamports as u128));
    let residue: u128 = target_total_lamports.saturating_sub(assigned_sum);

    if residue > 0 {
        // Sort indices by descending raw allocation (deterministic tie-break by
        // owner pubkey ascending — same convention as the Merkle leaf order).
        let mut order: Vec<usize> = (0..allocations.len()).collect();
        order.sort_by(|&a, &b| {
            let ra = raw_per_holder[a].2;
            let rb = raw_per_holder[b].2;
            rb.cmp(&ra)
                .then_with(|| allocations[a].owner.cmp(&allocations[b].owner))
        });
        // Hand out one residue lamport at a time, top to bottom, wrapping if there
        // are more residue lamports than holders (extreme edge case but possible
        // with adversarial inputs — bounds residue at one full pass per scan).
        let n = allocations.len() as u128;
        if n > 0 {
            let full_passes = residue / n;
            let leftover = (residue % n) as usize;
            if full_passes > 0 {
                let bump = u64::try_from(full_passes).unwrap_or(u64::MAX);
                for a in &mut allocations {
                    a.lamports = a.lamports.saturating_add(bump);
                }
            }
            for &idx in order.iter().take(leftover) {
                allocations[idx].lamports = allocations[idx].lamports.saturating_add(1);
            }
        }
    }

    allocations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn params(total_sol: u64, base_sol: u64, per_nft: u64, per_token: u64) -> AllocationParams {
        AllocationParams {
            total_allocation_sol: total_sol,
            base_allocation_sol: base_sol,
            per_nft_bonus_sol: per_nft,
            per_token_bonus_sol_per_million: per_token,
        }
    }

    #[test]
    fn empty_inputs_produce_empty_output() {
        let nfts = BTreeMap::new();
        let tokens = BTreeMap::new();
        let allocs = compute_allocations(&nfts, &tokens, params(30_000_000, 10, 100, 5));
        assert!(allocs.is_empty());
    }

    #[test]
    fn single_holder_gets_entire_pool() {
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(1), 5);
        let tokens = BTreeMap::new();
        let p = params(1_000, 10, 100, 5);
        let target = 1_000u128 * 1_000_000_000;
        let allocs = compute_allocations(&nfts, &tokens, p);
        assert_eq!(allocs.len(), 1);
        assert_eq!(allocs[0].owner, pk(1));
        assert_eq!(allocs[0].lamports as u128, target);
    }

    /// Core property: the sum of per-leaf lamports must EXACTLY equal
    /// `total_allocation_sol * 1e9`. This is the on-chain commitment.
    #[test]
    fn sum_of_allocations_exactly_equals_target() {
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(1), 1);
        nfts.insert(pk(2), 5);
        nfts.insert(pk(3), 100);
        let mut tokens = BTreeMap::new();
        tokens.insert(pk(1), 1_000_000);
        tokens.insert(pk(4), 50_000_000);

        let p = params(30_000_000, 10, 100, 5);
        let target = 30_000_000u128 * 1_000_000_000;

        let allocs = compute_allocations(&nfts, &tokens, p);
        let sum: u128 = allocs.iter().map(|a| a.lamports as u128).sum();
        assert_eq!(sum, target, "sum must EXACTLY equal target");
    }

    #[test]
    fn two_holder_50_50_split_for_equal_contributions() {
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(1), 3);
        nfts.insert(pk(2), 3);
        let tokens = BTreeMap::new();

        let p = params(2_000, 0, 100, 0); // base=0 so nft is the only signal
        let allocs = compute_allocations(&nfts, &tokens, p);
        assert_eq!(allocs.len(), 2);
        // Both holders identical; should split exactly in half.
        let target = 2_000u128 * 1_000_000_000;
        assert_eq!(
            allocs[0].lamports as u128 + allocs[1].lamports as u128,
            target
        );
        // Within 1 lamport of each other (residue might bump one).
        let diff = allocs[0].lamports.abs_diff(allocs[1].lamports);
        assert!(diff <= 1);
    }

    #[test]
    fn unioned_holders_include_owners_present_in_only_one_cohort() {
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(1), 1);
        let mut tokens = BTreeMap::new();
        tokens.insert(pk(2), 1_000_000);

        let p = params(1_000, 0, 100, 50);
        let allocs = compute_allocations(&nfts, &tokens, p);
        assert_eq!(allocs.len(), 2);
        let owners: Vec<Pubkey> = allocs.iter().map(|a| a.owner).collect();
        assert!(owners.contains(&pk(1)));
        assert!(owners.contains(&pk(2)));
    }

    #[test]
    fn nft_holder_with_more_nfts_gets_more_lamports() {
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(1), 1);
        nfts.insert(pk(2), 10);
        let tokens = BTreeMap::new();

        let p = params(1_100, 0, 100, 0); // pure NFT split
        let allocs = compute_allocations(&nfts, &tokens, p);
        let by_owner: BTreeMap<Pubkey, u64> =
            allocs.iter().map(|a| (a.owner, a.lamports)).collect();
        // pk(2) has 10x the NFTs, so should have ~10x the allocation.
        let a = by_owner[&pk(1)];
        let b = by_owner[&pk(2)];
        assert!(b >= 9 * a, "{b} should be ~10x {a}");
    }

    #[test]
    fn token_balance_sub_million_contributes_zero_token_bonus() {
        // Holder with 999_999 tokens contributes the same as a holder with 0
        // tokens (per the floor-division semantics in the policy).
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(1), 1);
        nfts.insert(pk(2), 1);
        let mut tokens = BTreeMap::new();
        tokens.insert(pk(2), 999_999); // sub-million

        let p = params(2_000, 0, 100, 1_000_000); // huge token bonus, NFT bonus modest
        let allocs = compute_allocations(&nfts, &tokens, p);
        let by_owner: BTreeMap<Pubkey, u64> =
            allocs.iter().map(|a| (a.owner, a.lamports)).collect();
        // Both holders have 1 NFT and zero effective token bonus, so they should
        // split 50/50 (within 1 lamport of residue).
        assert!(by_owner[&pk(1)].abs_diff(by_owner[&pk(2)]) <= 1);
    }

    #[test]
    fn token_balance_at_one_million_grants_one_unit_of_bonus() {
        let nfts = BTreeMap::new();
        let mut tokens = BTreeMap::new();
        tokens.insert(pk(1), 0); // 0 token bonus + base
        tokens.insert(pk(2), 1_000_000); // 1 unit of token bonus + base

        // Force the policy to make the bonus dominant: base=1, per-token=99.
        let p = params(101, 1, 0, 99);
        let allocs = compute_allocations(&nfts, &tokens, p);
        let by_owner: BTreeMap<Pubkey, u64> =
            allocs.iter().map(|a| (a.owner, a.lamports)).collect();
        // pk(1) raw = 1 SOL. pk(2) raw = 1 + 99 = 100 SOL. Pool = 101 SOL.
        // Expected: pk(1) ≈ 1 SOL, pk(2) ≈ 100 SOL, residue ≤ 1.
        let one_sol = 1_000_000_000u128;
        let pk1 = by_owner[&pk(1)] as u128;
        let pk2 = by_owner[&pk(2)] as u128;
        assert!(pk1.abs_diff(one_sol) <= 1, "pk1 lamports: {pk1}");
        assert!(pk2.abs_diff(100 * one_sol) <= 1, "pk2 lamports: {pk2}");
    }

    /// Edge case: total_allocation_sol = 0 → every holder gets zero lamports but
    /// the rows are still emitted (so the operator can audit the cohort).
    #[test]
    fn zero_total_allocation_produces_zero_lamport_rows() {
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(1), 5);
        let tokens = BTreeMap::new();
        let p = params(0, 10, 100, 0);
        let allocs = compute_allocations(&nfts, &tokens, p);
        assert_eq!(allocs.len(), 1);
        assert_eq!(allocs[0].lamports, 0);
    }

    #[test]
    fn at_production_scale_total_does_not_overflow() {
        // 30M SOL pool, 1000 holders each with realistic NFT/token counts.
        let mut nfts = BTreeMap::new();
        let mut tokens = BTreeMap::new();
        for i in 0u8..255 {
            nfts.insert(pk(i), (i as u64).saturating_mul(3));
            tokens.insert(pk(i), (i as u64).saturating_mul(5_000_000));
        }
        let p = params(30_000_000, 10, 100, 5);
        let target = 30_000_000u128 * 1_000_000_000;
        let allocs = compute_allocations(&nfts, &tokens, p);
        let sum: u128 = allocs.iter().map(|a| a.lamports as u128).sum();
        assert_eq!(sum, target);
    }

    #[test]
    fn output_is_deterministic() {
        // Two runs over the same inputs must produce identical allocations.
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(7), 3);
        nfts.insert(pk(2), 5);
        nfts.insert(pk(11), 1);
        let mut tokens = BTreeMap::new();
        tokens.insert(pk(2), 1_500_000);
        tokens.insert(pk(99), 7_000_000);

        let p = params(1_000_000, 10, 100, 5);
        let a = compute_allocations(&nfts, &tokens, p);
        let b = compute_allocations(&nfts, &tokens, p);
        assert_eq!(a, b);
    }

    #[test]
    fn residue_bumps_top_holder_first() {
        // 3 holders, identical raw → residue distribution determined by deterministic
        // tie-break (owner pubkey ascending). Confirm the top-bumped holder is the
        // smallest pubkey.
        let mut nfts = BTreeMap::new();
        nfts.insert(pk(3), 1);
        nfts.insert(pk(1), 1);
        nfts.insert(pk(2), 1);
        let tokens = BTreeMap::new();

        // Choose target so target_lamports % 3 != 0 → residue exists.
        // target = 100 lamports = 1.0e-7 SOL ⇒ use total_allocation_sol = 0
        // doesn't work. Use total = 10 SOL = 1e10 lamports; 1e10 / 3 = 3333333333 r 1.
        let p = params(10, 0, 100, 0);
        let target = 10u128 * 1_000_000_000;
        let allocs = compute_allocations(&nfts, &tokens, p);
        let sum: u128 = allocs.iter().map(|a| a.lamports as u128).sum();
        assert_eq!(sum, target);

        // The smallest-pubkey holder (pk(1)) should have at least the others.
        let by_owner: BTreeMap<Pubkey, u64> =
            allocs.iter().map(|a| (a.owner, a.lamports)).collect();
        assert!(by_owner[&pk(1)] >= by_owner[&pk(2)]);
        assert!(by_owner[&pk(1)] >= by_owner[&pk(3)]);
    }
}

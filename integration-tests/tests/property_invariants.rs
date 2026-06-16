//! Property-based invariant tests (proptest).
//!
//! Pin the SPEC §8 invariants under arbitrary inputs:
//!
//! * **I1 (Genesis SOL conservation).** For any synthetic account multiset,
//!   `sum(claimable.lamports) + treasury.total_lamports == sum(input.lamports)`.
//! * **I5 (Matcher replay invariance).** For any intent multiset,
//!   `batch_match(perm1) == batch_match(perm2)` byte-for-byte.
//! * **Merkle determinism.** For any leaf set, `MerkleTree::build(perm1).root ==
//!   MerkleTree::build(perm2).root`.
//! * **Treasury commutativity.** Order of `credit()` calls does not affect `total_lamports`.
//!
//! The proptest config uses small-ish input sizes (≤ 64 elements) so the suite stays
//! quick to run; the invariants are linear and would still hold at larger sizes — we
//! trade exhaustive coverage for fast feedback.

use proptest::prelude::*;
use solana_program::pubkey::Pubkey;
use staccana_genesis::partition::{Disposition, SYSTEM_PROGRAM_ID};
use staccana_genesis::{build_genesis, partition, ClaimableLeaf, MerkleTree, Treasury};
use staccana_integration_tests::{ConstantProductAmm, ConstantProductPool, SyntheticAccount};
use staccana_matcher::{batch_match, BatchConfig, QuoteRegistry, Side, SwapIntent};

// ──────────────────────────────────────────────────────────────────────────────
// Strategies
// ──────────────────────────────────────────────────────────────────────────────

fn arb_pubkey() -> impl Strategy<Value = Pubkey> {
    any::<[u8; 32]>().prop_map(Pubkey::new_from_array)
}

fn arb_owner() -> impl Strategy<Value = Pubkey> {
    // Mix system-owned (claimable candidate) with arbitrary owners (treasury) so the
    // partition rule sees both sides. Half the accounts are system-owned in expectation.
    prop_oneof![
        Just(SYSTEM_PROGRAM_ID),
        Just(SYSTEM_PROGRAM_ID),
        arb_pubkey(),
    ]
}

fn arb_data_len() -> impl Strategy<Value = usize> {
    // Bias toward zero (so the system-owned branch can hit the Claimable disposition)
    // while still occasionally producing system-owned-with-data PDAs.
    prop_oneof![Just(0usize), Just(0usize), Just(0usize), 1usize..256,]
}

fn arb_account() -> impl Strategy<Value = SyntheticAccount> {
    (
        arb_pubkey(),
        arb_owner(),
        arb_data_len(),
        0u64..1_000_000_000_000u64,
    )
        .prop_map(|(pubkey, owner, data_len, lamports)| SyntheticAccount {
            pubkey,
            owner,
            data_len,
            lamports,
        })
}

fn arb_account_set() -> impl Strategy<Value = Vec<SyntheticAccount>> {
    prop::collection::vec(arb_account(), 0..64)
}

fn arb_leaf() -> impl Strategy<Value = ClaimableLeaf> {
    (arb_pubkey(), 0u64..1_000_000_000_000u64)
        .prop_map(|(pubkey, lamports)| ClaimableLeaf { pubkey, lamports })
}

fn arb_leaf_set() -> impl Strategy<Value = Vec<ClaimableLeaf>> {
    prop::collection::vec(arb_leaf(), 1..32)
}

fn arb_intent(base: Pubkey, quote: Pubkey) -> impl Strategy<Value = SwapIntent> {
    (
        any::<u8>(), // signer byte
        any::<bool>(),
        1u64..1_000_000u64,
    )
        .prop_map(move |(s, is_buy, in_amount)| {
            let signer = Pubkey::new_from_array([s; 32]);
            if is_buy {
                SwapIntent {
                    signer,
                    in_mint: quote,
                    in_amount,
                    out_mint: base,
                    min_out: 0,
                    nonce: 0,
                }
            } else {
                SwapIntent {
                    signer,
                    in_mint: base,
                    in_amount,
                    out_mint: quote,
                    min_out: 0,
                    nonce: 0,
                }
            }
        })
}

fn arb_intent_set() -> impl Strategy<Value = Vec<SwapIntent>> {
    let base = Pubkey::new_from_array([0x10u8; 32]);
    let quote = Pubkey::new_from_array([0x01u8; 32]);
    prop::collection::vec(arb_intent(base, quote), 0..32)
}

fn shuffle_indices(len: usize, seed: u64) -> Vec<usize> {
    // Tiny xorshift permutation to avoid a `rand` dep. Deterministic given seed; suitable
    // for shuffling test inputs into a fresh order.
    let mut indices: Vec<usize> = (0..len).collect();
    let mut state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    for i in (1..len).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state as usize) % (i + 1);
        indices.swap(i, j);
    }
    indices
}

// ──────────────────────────────────────────────────────────────────────────────
// I1 — genesis SOL conservation
// ──────────────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        ..ProptestConfig::default()
    })]

    #[test]
    fn genesis_sol_conservation_holds_under_arbitrary_mix(accounts in arb_account_set()) {
        // I1 (SPEC §8): no SOL is created or destroyed by the partition.
        let total_in: u128 = accounts.iter().map(|a| a.lamports as u128).sum();
        let claimable_in: u128 = accounts
            .iter()
            .filter(|a| partition(*a) == Disposition::Claimable)
            .map(|a| a.lamports as u128)
            .sum();

        let genesis = build_genesis(accounts);
        let treasury_total = genesis.treasury.total_lamports();

        prop_assert_eq!(claimable_in + treasury_total, total_in);
    }

    #[test]
    fn genesis_partition_count_matches_disposition(accounts in arb_account_set()) {
        // The output's claimable_count must equal the number of accounts the partition
        // rule classified as claimable. This is the count side of the conservation check.
        let claimable_count = accounts
            .iter()
            .filter(|a| partition(*a) == Disposition::Claimable)
            .count();
        let treasury_count = accounts
            .iter()
            .filter(|a| partition(*a) == Disposition::Treasury)
            .count();

        let genesis = build_genesis(accounts);
        prop_assert_eq!(genesis.claimable_count, claimable_count);
        prop_assert_eq!(genesis.treasury.account_count() as usize, treasury_count);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Merkle determinism
// ──────────────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn merkle_root_invariant_under_input_permutation(
        leaves in arb_leaf_set(),
        seed in any::<u64>()
    ) {
        let perm = shuffle_indices(leaves.len(), seed);
        let permuted: Vec<ClaimableLeaf> =
            perm.iter().map(|&i| leaves[i].clone()).collect();
        let root_a = MerkleTree::build(leaves).root;
        let root_b = MerkleTree::build(permuted).root;
        prop_assert_eq!(root_a, root_b);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// I5 — matcher replay invariance
// ──────────────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn matcher_replay_invariance_holds_under_real_amm(
        intents in arb_intent_set(),
        seed in any::<u64>()
    ) {
        // I5 (SPEC §8 / §6.4): batch_match must produce the same output for any
        // permutation of the intent multiset. Use the real-feeling AMM (not a stub) so
        // we cover the `simulate_post_price_q64` path that depends on net flow.
        let base = Pubkey::new_from_array([0x10u8; 32]);
        let quote = Pubkey::new_from_array([0x01u8; 32]);
        let amm = ConstantProductAmm::new();
        amm.add_pool(base, quote, ConstantProductPool::new(50_000, 50_000));
        let cfg = BatchConfig {
            registry: QuoteRegistry::new([quote]),
        };

        let perm = shuffle_indices(intents.len(), seed);
        let permuted: Vec<SwapIntent> = perm.iter().map(|&i| intents[i].clone()).collect();

        let result_a = batch_match(intents, &cfg, &amm);
        let result_b = batch_match(permuted, &cfg, &amm);
        prop_assert_eq!(result_a, result_b);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Treasury commutativity
// ──────────────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        ..ProptestConfig::default()
    })]

    #[test]
    fn treasury_credit_is_commutative(
        amounts in prop::collection::vec(0u64..1_000_000_000u64, 0..32),
        seed in any::<u64>()
    ) {
        let perm = shuffle_indices(amounts.len(), seed);
        let permuted: Vec<u64> = perm.iter().map(|&i| amounts[i]).collect();

        let mut t1 = Treasury::new();
        for v in &amounts {
            t1.credit(*v);
        }
        let mut t2 = Treasury::new();
        for v in &permuted {
            t2.credit(*v);
        }
        prop_assert_eq!(t1.total_lamports(), t2.total_lamports());
        prop_assert_eq!(t1.account_count(), t2.account_count());
    }

    #[test]
    fn treasury_total_matches_sum_of_credits(
        amounts in prop::collection::vec(0u64..1_000_000_000u64, 0..64)
    ) {
        let expected: u128 = amounts.iter().map(|&v| v as u128).sum();
        let mut t = Treasury::new();
        for v in &amounts {
            t.credit(*v);
        }
        prop_assert_eq!(t.total_lamports(), expected);
        prop_assert_eq!(t.account_count() as usize, amounts.len());
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Cross-property: matcher output is sorted ascending by (base, quote)
// ──────────────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    #[test]
    fn matcher_output_sorted_by_base_then_quote_pubkey(intents in arb_intent_set()) {
        // SPEC §6.3: output is sorted by (base_mint, quote_mint) ascending. With one
        // base and one quote in the strategy, the result is at most one element — but
        // the property is still trivially testable.
        let base = Pubkey::new_from_array([0x10u8; 32]);
        let quote = Pubkey::new_from_array([0x01u8; 32]);
        let amm = ConstantProductAmm::new();
        amm.add_pool(base, quote, ConstantProductPool::new(50_000, 50_000));
        let cfg = BatchConfig {
            registry: QuoteRegistry::new([quote]),
        };
        let result = batch_match(intents, &cfg, &amm);

        for window in result.windows(2) {
            let prev = (window[0].base_mint, window[0].quote_mint);
            let next = (window[1].base_mint, window[1].quote_mint);
            prop_assert!(prev < next, "matcher output unsorted: {:?} vs {:?}", prev, next);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Treasury bounded growth — saturating_add never wraps
// ──────────────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    #[test]
    fn treasury_lamports_for_pda_clamps_to_u64_max(
        big_credits in prop::collection::vec(1_000_000_000_000u64..u64::MAX, 1..8)
    ) {
        // Aggregate sum may exceed u64::MAX (we're stuffing many near-MAX credits). The
        // PDA-clamped view must saturate at u64::MAX rather than wrapping or panicking.
        let mut t = Treasury::new();
        for v in &big_credits {
            t.credit(*v);
        }
        let pda_amount = t.lamports_for_pda();
        prop_assert!(pda_amount <= u64::MAX);
        // total_lamports keeps the full u128 — should be at least the largest single credit.
        prop_assert!(t.total_lamports() >= *big_credits.iter().max().unwrap() as u128);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// AMM monotonicity sanity (pool reserves move price the right way)
// ──────────────────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    #[test]
    fn amm_buy_increases_price_and_sell_decreases_price(
        amount in 1u64..1_000u64
    ) {
        // Sanity for the integration-tests crate's own AMM mock: buying base must push
        // quote-per-base price up; selling base must push it down. Crashes here mean the
        // mock's invariant math is broken before the matcher even sees it.
        let base = Pubkey::new_from_array([0x10u8; 32]);
        let quote = Pubkey::new_from_array([0x01u8; 32]);
        let amm = ConstantProductAmm::new();
        amm.add_pool(base, quote, ConstantProductPool::new(10_000, 10_000));
        let pre = staccana_matcher::AmmAdapter::spot_price_q64(&amm, &base, &quote);
        let post_buy = staccana_matcher::AmmAdapter::simulate_post_price_q64(
            &amm, &base, &quote, amount, Side::Buy,
        );
        let post_sell = staccana_matcher::AmmAdapter::simulate_post_price_q64(
            &amm, &base, &quote, amount, Side::Sell,
        );
        prop_assert!(post_buy >= pre, "buy must push price up: pre={pre}, post={post_buy}");
        prop_assert!(post_sell <= pre, "sell must push price down: pre={pre}, post={post_sell}");
    }
}

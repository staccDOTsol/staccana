//! Matcher cross-crate scenarios.
//!
//! Drives `staccana_matcher::batch_match` against the cross-crate
//! [`ConstantProductAmm`](staccana_integration_tests::ConstantProductAmm) (instead of
//! the always-1.0 stub used in `matcher/src/batch.rs`'s internal tests). Exercises:
//!
//! * Multi-mint batches (two distinct longtail mints quoted in the same quote, plus a
//!   second quote registry entry).
//! * Buy-heavy and sell-heavy distributions and the residual flow they produce.
//! * Multi-buyer/multi-seller mixes and the size-priority ordering.
//! * Replay invariance against the real-feeling AMM (the `StubAmm` in the matcher's own
//!   tests is a degenerate case; this one has reserves so price actually moves).
//!
//! These tests aren't proptest-driven (those live in `property_invariants.rs`); they pin
//! known scenarios so a regression in any matcher-side rule produces a small,
//! human-readable diff.

use solana_program::pubkey::Pubkey;
use staccana_integration_tests::*;
use staccana_matcher::{batch_match, BatchConfig, ClearingResult, QuoteRegistry, SwapIntent};

fn buy(signer: u8, base: Pubkey, quote: Pubkey, in_amount: u64) -> SwapIntent {
    SwapIntent {
        signer: pk(signer),
        in_mint: quote,
        in_amount,
        out_mint: base,
        min_out: 0,
        nonce: 0,
    }
}

fn sell(signer: u8, base: Pubkey, quote: Pubkey, in_amount: u64) -> SwapIntent {
    SwapIntent {
        signer: pk(signer),
        in_mint: base,
        in_amount,
        out_mint: quote,
        min_out: 0,
        nonce: 0,
    }
}

fn registry_with_quote(byte: u8) -> QuoteRegistry {
    QuoteRegistry::new([pk(byte)])
}

fn balanced_amm(base: Pubkey, quote: Pubkey) -> ConstantProductAmm {
    let amm = ConstantProductAmm::new();
    amm.add_pool(base, quote, ConstantProductPool::new(10_000, 10_000));
    amm
}

#[test]
fn balanced_pool_perfect_cross_clears_at_unit_price() {
    let base = pk(0x10);
    let quote = pk(0x01);
    let amm = balanced_amm(base, quote);
    let cfg = BatchConfig {
        registry: registry_with_quote(0x01),
    };

    let result = batch_match(
        vec![buy(20, base, quote, 100), sell(30, base, quote, 100)],
        &cfg,
        &amm,
    );
    assert_eq!(result.len(), 1);
    let r = &result[0];
    assert_eq!(r.matches.len(), 1);
    assert_eq!(r.matches[0].base_amount, 100);
    assert_eq!(r.matches[0].quote_amount, 100);
    assert!(r.residual.is_empty());
    assert_eq!(r.clearing_price_q64, 1u128 << 64);
}

#[test]
fn buy_heavy_batch_pushes_residual_to_amm() {
    // 3 buyers totaling 250 quote, only 100 base for sale — most of the quote can't
    // cross and falls through to residual. Exact residual quote depends on the clearing
    // price (which is determined by net flow against the AMM), so we assert bounds
    // rather than an exact number.
    let base = pk(0x10);
    let quote = pk(0x01);
    let amm = balanced_amm(base, quote);
    let cfg = BatchConfig {
        registry: registry_with_quote(0x01),
    };

    let intents = vec![
        buy(20, base, quote, 50),
        buy(21, base, quote, 100),
        buy(22, base, quote, 100),
        sell(40, base, quote, 100),
    ];
    let result = batch_match(intents, &cfg, &amm);
    let r = &result[0];

    // At least one match (the big buyer crosses with the lone seller).
    assert!(
        !r.matches.is_empty(),
        "buy-heavy batch should still produce a cross"
    );
    // Residual must include leftover quote — at least the smaller buyer (50) and
    // probably one of the 100-quote buyers, depending on rounding.
    let total_residual_quote: u64 = r
        .residual
        .iter()
        .filter(|x| x.in_mint == quote)
        .map(|x| x.in_amount)
        .sum();
    assert!(
        total_residual_quote >= 100 && total_residual_quote <= 200,
        "residual quote should be in [100, 200], got {total_residual_quote}",
    );
    // Most of the sell-side base should have been matched (demand > supply); residual
    // sell base is bounded by the rounding loss at the clearing price.
    let residual_sell_base: u64 = r
        .residual
        .iter()
        .filter(|x| x.in_mint == base)
        .map(|x| x.in_amount)
        .sum();
    assert!(
        residual_sell_base < 50,
        "most of the sell-side base should match: residual={residual_sell_base}",
    );
}

#[test]
fn sell_heavy_batch_drives_clearing_price_down() {
    // Heavy net sell pressure should drag the post-trade price below 1.0, so the
    // midpoint clearing price < 1.0.
    let base = pk(0x10);
    let quote = pk(0x01);
    let amm = balanced_amm(base, quote);
    let cfg = BatchConfig {
        registry: registry_with_quote(0x01),
    };

    let intents = vec![
        sell(20, base, quote, 1_000),
        sell(21, base, quote, 1_500),
        buy(30, base, quote, 200),
    ];
    let result = batch_match(intents, &cfg, &amm);
    let r = &result[0];
    assert!(
        r.clearing_price_q64 < (1u128 << 64),
        "sell pressure must push clearing < 1.0 (got {})",
        r.clearing_price_q64
    );
}

#[test]
fn multi_pair_batches_clear_independently_and_in_order() {
    // Two longtail mints, both quoted in pk(0x01). Each pair should produce its own
    // ClearingResult, ordered by base pubkey ascending.
    let quote = pk(0x01);
    let base_a = pk(0x10);
    let base_b = pk(0x11);

    let amm = ConstantProductAmm::new();
    amm.add_pool(base_a, quote, ConstantProductPool::new(10_000, 10_000));
    amm.add_pool(base_b, quote, ConstantProductPool::new(5_000, 20_000));
    let cfg = BatchConfig {
        registry: registry_with_quote(0x01),
    };

    let intents = vec![
        buy(20, base_a, quote, 100),
        sell(30, base_a, quote, 100),
        buy(40, base_b, quote, 200),
        sell(50, base_b, quote, 50),
    ];
    let result: Vec<ClearingResult> = batch_match(intents, &cfg, &amm);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].base_mint, base_a);
    assert_eq!(result[1].base_mint, base_b);

    // base_a is balanced ⇒ unit price.
    assert_eq!(result[0].clearing_price_q64, 1u128 << 64);
    // base_b pool starts at 4 quote per base (20_000/5_000) — clearing price > 1.0.
    assert!(result[1].clearing_price_q64 > (1u128 << 64));
}

#[test]
fn multi_quote_registry_classifies_pairs_distinctly() {
    // Two quote mints — pk(0x01) and pk(0x02). A base/quote2 pair should classify
    // independently of a base/quote1 pair even with the same base mint.
    let base = pk(0x10);
    let quote_1 = pk(0x01);
    let quote_2 = pk(0x02);

    let amm = ConstantProductAmm::new();
    amm.add_pool(base, quote_1, ConstantProductPool::new(10_000, 10_000));
    amm.add_pool(base, quote_2, ConstantProductPool::new(10_000, 10_000));
    let cfg = BatchConfig {
        registry: QuoteRegistry::new([quote_1, quote_2]),
    };

    let intents = vec![
        buy(20, base, quote_1, 100),
        sell(30, base, quote_1, 100),
        buy(40, base, quote_2, 200),
        sell(50, base, quote_2, 200),
    ];
    let result = batch_match(intents, &cfg, &amm);
    assert_eq!(result.len(), 2, "one ClearingResult per (base, quote) pair");
}

#[test]
fn replay_invariance_against_real_pool_holds_for_random_permutations() {
    // The matcher's own test uses a stub AMM where post-price == pre-price; this version
    // uses an AMM with reserves so post-price actually depends on the net flow. The
    // replay invariant must still hold (intent set permutation does not change output).
    let base = pk(0x10);
    let quote = pk(0x01);
    let amm = ConstantProductAmm::new();
    amm.add_pool(base, quote, ConstantProductPool::new(50_000, 50_000));
    let cfg = BatchConfig {
        registry: registry_with_quote(0x01),
    };

    let intents_a = vec![
        buy(10, base, quote, 100),
        sell(20, base, quote, 75),
        buy(30, base, quote, 200),
        sell(40, base, quote, 150),
        buy(50, base, quote, 50),
        sell(60, base, quote, 25),
    ];
    let mut intents_b = intents_a.clone();
    intents_b.reverse();
    let mut intents_c = intents_a.clone();
    intents_c.swap(0, 5);
    intents_c.swap(1, 4);

    let r_a = batch_match(intents_a, &cfg, &amm);
    let r_b = batch_match(intents_b, &cfg, &amm);
    let r_c = batch_match(intents_c, &cfg, &amm);
    assert_eq!(r_a, r_b);
    assert_eq!(r_a, r_c);
}

#[test]
fn size_priority_ordering_is_preserved_under_real_amm() {
    // Largest-amount-first matching applies regardless of which AMM is plugged in.
    let base = pk(0x10);
    let quote = pk(0x01);
    let amm = balanced_amm(base, quote);
    let cfg = BatchConfig {
        registry: registry_with_quote(0x01),
    };

    let result = batch_match(
        vec![
            buy(10, base, quote, 50),
            buy(11, base, quote, 100),
            buy(12, base, quote, 200),
            sell(20, base, quote, 200),
        ],
        &cfg,
        &amm,
    );
    let r = &result[0];
    assert_eq!(r.matches[0].buyer, pk(12));
    assert_eq!(r.matches[0].seller, pk(20));
    assert!(r.matches[0].base_amount > 0);
    assert!(r.matches[0].quote_amount > 0);
    // AMM-anchored clearing can move the fill size away from the unit-price 200, but
    // the largest buyer still takes priority and no seller dust is left unmatched.
    assert_eq!(r.residual.len(), 2);
    assert!(r.residual.iter().all(|intent| intent.in_mint == quote));
    assert!(r.residual.iter().any(|intent| intent.signer == pk(10)));
    assert!(r.residual.iter().any(|intent| intent.signer == pk(11)));
}

#[test]
fn empty_intent_set_produces_empty_output_against_real_amm() {
    let base = pk(0x10);
    let quote = pk(0x01);
    let amm = balanced_amm(base, quote);
    let cfg = BatchConfig {
        registry: registry_with_quote(0x01),
    };
    let result = batch_match(vec![], &cfg, &amm);
    assert!(result.is_empty());
}

#[test]
fn lopsided_pool_clears_above_unity_with_balanced_intent_set() {
    // Pool has 4x more quote than base ⇒ pre-trade price is 4.0. With balanced buy/sell
    // flow the clearing price should still be ~4.0 (post-price equals pre-price for a
    // balanced net flow on this AMM, so midpoint == 4.0).
    let base = pk(0x10);
    let quote = pk(0x01);
    let amm = ConstantProductAmm::new();
    amm.add_pool(base, quote, ConstantProductPool::new(2_500, 10_000));
    let cfg = BatchConfig {
        registry: registry_with_quote(0x01),
    };

    let result = batch_match(
        vec![buy(20, base, quote, 400), sell(30, base, quote, 100)],
        &cfg,
        &amm,
    );
    let r = &result[0];
    let four_q64 = 4u128 << 64;
    assert_eq!(
        r.clearing_price_q64, four_q64,
        "balanced flow on a 4.0-priced pool should clear at exactly 4.0",
    );
}

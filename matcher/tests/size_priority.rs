//! Extensive coverage of the size-priority matching order.
//!
//! Per `batch.rs`, both buys and sells are sorted by `(in_amount desc, signer asc)` before
//! pair-matching. This file pushes that ordering with five-plus participants per side, equal-
//! amount tie-breaks resolved by signer pubkey, and verifies match emission order tracks
//! that sort exactly.

use solana_program::pubkey::Pubkey;
use staccana_matcher::*;

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn registry() -> QuoteRegistry {
    QuoteRegistry::new([pk(1)])
}

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

/// Static AMM with P_pre == P_post == 1.0 keeps clearing price exactly 1.0 so amounts are
/// trivially translatable between base and quote.
struct UnitAmm;

impl AmmAdapter for UnitAmm {
    fn spot_price_q64(&self, _: &Pubkey, _: &Pubkey) -> u128 {
        1u128 << 64
    }
    fn simulate_post_price_q64(&self, _: &Pubkey, _: &Pubkey, _: u64, _: Side) -> u128 {
        1u128 << 64
    }
}

#[test]
fn varied_amounts_match_largest_first_on_both_sides() {
    let amm = UnitAmm;
    let base = pk(2);
    let quote = pk(1);

    // Five buyers and five sellers with varied amounts. Input order is intentionally
    // shuffled — output ordering should reflect the size-priority sort, not the input order.
    let result = batch_match(
        vec![
            buy(31, base, quote, 50),
            buy(32, base, quote, 500),
            buy(33, base, quote, 100),
            buy(34, base, quote, 200),
            buy(35, base, quote, 300),
            sell(41, base, quote, 200),
            sell(42, base, quote, 50),
            sell(43, base, quote, 500),
            sell(44, base, quote, 100),
            sell(45, base, quote, 300),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert!(!r.matches.is_empty());

    // Largest buyer (32, 500) crosses largest seller (43, 500) first.
    assert_eq!(r.matches[0].buyer, pk(32));
    assert_eq!(r.matches[0].seller, pk(43));
    assert_eq!(r.matches[0].base_amount, 500);

    // Then 35 (300) with 45 (300).
    assert_eq!(r.matches[1].buyer, pk(35));
    assert_eq!(r.matches[1].seller, pk(45));
    assert_eq!(r.matches[1].base_amount, 300);

    // Then 34 (200) with 41 (200).
    assert_eq!(r.matches[2].buyer, pk(34));
    assert_eq!(r.matches[2].seller, pk(41));
    assert_eq!(r.matches[2].base_amount, 200);

    // Then 33 (100) with 44 (100).
    assert_eq!(r.matches[3].buyer, pk(33));
    assert_eq!(r.matches[3].seller, pk(44));
    assert_eq!(r.matches[3].base_amount, 100);

    // Smallest pair last.
    assert_eq!(r.matches[4].buyer, pk(31));
    assert_eq!(r.matches[4].seller, pk(42));
    assert_eq!(r.matches[4].base_amount, 50);

    assert!(
        r.residual.is_empty(),
        "perfectly matched amounts ⇒ no residual"
    );
}

#[test]
fn ties_break_by_signer_pubkey_ascending() {
    // Six buyers all with in_amount = 100; six sellers all with in_amount = 100. The match
    // sequence must walk the sorted-by-signer-ascending order on both sides.
    let amm = UnitAmm;
    let base = pk(2);
    let quote = pk(1);

    let result = batch_match(
        vec![
            // Buyers in scrambled signer order.
            buy(15, base, quote, 100),
            buy(11, base, quote, 100),
            buy(14, base, quote, 100),
            buy(10, base, quote, 100),
            buy(13, base, quote, 100),
            buy(12, base, quote, 100),
            // Sellers in scrambled signer order.
            sell(25, base, quote, 100),
            sell(21, base, quote, 100),
            sell(24, base, quote, 100),
            sell(20, base, quote, 100),
            sell(23, base, quote, 100),
            sell(22, base, quote, 100),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert_eq!(r.matches.len(), 6);

    // Signer ascending on both sides: (10,20), (11,21), (12,22), (13,23), (14,24), (15,25).
    let expected_buyers = [pk(10), pk(11), pk(12), pk(13), pk(14), pk(15)];
    let expected_sellers = [pk(20), pk(21), pk(22), pk(23), pk(24), pk(25)];
    for (i, m) in r.matches.iter().enumerate() {
        assert_eq!(m.buyer, expected_buyers[i], "match {} buyer mismatch", i);
        assert_eq!(m.seller, expected_sellers[i], "match {} seller mismatch", i);
        assert_eq!(m.base_amount, 100);
    }
}

#[test]
fn mixed_size_and_tie_break_order() {
    // Two buyers at 200, two at 100, one at 300. Sorted: (300), (200, lower-signer),
    // (200, higher-signer), (100, lower-signer), (100, higher-signer).
    let amm = UnitAmm;
    let base = pk(2);
    let quote = pk(1);

    let result = batch_match(
        vec![
            buy(15, base, quote, 200),   // tied at 200, higher signer
            buy(10, base, quote, 200),   // tied at 200, lower signer ⇒ matches first of the pair
            buy(20, base, quote, 100),   // tied at 100, lower signer
            buy(25, base, quote, 100),   // tied at 100, higher signer
            buy(5, base, quote, 300),    // largest
            sell(50, base, quote, 1000), // single seller absorbs everyone
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert_eq!(r.matches.len(), 5);
    let expected_buyer_order = [pk(5), pk(10), pk(15), pk(20), pk(25)];
    let expected_amount_order = [300, 200, 200, 100, 100];
    for (i, m) in r.matches.iter().enumerate() {
        assert_eq!(m.buyer, expected_buyer_order[i]);
        assert_eq!(m.base_amount, expected_amount_order[i]);
    }
}

#[test]
fn input_reordering_does_not_change_match_sequence() {
    // The size-priority sort is order-invariant on the input. Two different input orderings
    // of the same multiset must produce byte-identical matches.
    let amm = UnitAmm;
    let base = pk(2);
    let quote = pk(1);

    let order_a = vec![
        buy(10, base, quote, 100),
        buy(11, base, quote, 200),
        buy(12, base, quote, 300),
        buy(13, base, quote, 400),
        buy(14, base, quote, 500),
        sell(20, base, quote, 500),
        sell(21, base, quote, 400),
        sell(22, base, quote, 300),
        sell(23, base, quote, 200),
        sell(24, base, quote, 100),
    ];
    let order_b = vec![
        sell(24, base, quote, 100),
        buy(14, base, quote, 500),
        sell(20, base, quote, 500),
        buy(10, base, quote, 100),
        sell(22, base, quote, 300),
        buy(12, base, quote, 300),
        sell(21, base, quote, 400),
        buy(11, base, quote, 200),
        sell(23, base, quote, 200),
        buy(13, base, quote, 400),
    ];

    let cfg = BatchConfig {
        registry: registry(),
    };
    let result_a = batch_match(order_a, &cfg, &amm);
    let result_b = batch_match(order_b, &cfg, &amm);

    assert_eq!(
        result_a, result_b,
        "size-priority sort is order-invariant ⇒ byte-identical output"
    );
}

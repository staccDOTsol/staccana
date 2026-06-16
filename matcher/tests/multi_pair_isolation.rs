//! Per-(base, quote) pair isolation.
//!
//! `batch_match` groups intents into separate buckets per (base, quote) pair and clears each
//! bucket independently. A flow imbalance in pair A must not bleed into pair B's clearing
//! price or matches. This test exercises that with three distinct base mints sharing one
//! quote mint, plus a fourth pair using a different quote.

use solana_program::pubkey::Pubkey;
use staccana_matcher::*;

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn registry() -> QuoteRegistry {
    // Two quote mints: pk(1) and pk(50).
    QuoteRegistry::new([pk(1), pk(50)])
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

/// Per-pair AMM mock — different reserves per (base, quote) so each pair has a distinct
/// P_pre that we can use to detect cross-contamination.
struct PerPairAmm;

impl AmmAdapter for PerPairAmm {
    fn spot_price_q64(&self, base: &Pubkey, _quote: &Pubkey) -> u128 {
        // Encode "P_pre depends on base mint" by using the first byte of the base pubkey.
        // base pk(2) → price 1.0; base pk(3) → price 2.0; base pk(4) → price 3.0; etc.
        let factor = base.as_ref()[0] as u128 - 1;
        factor << 64
    }

    fn simulate_post_price_q64(
        &self,
        base: &Pubkey,
        quote: &Pubkey,
        _amount: u64,
        _side: Side,
    ) -> u128 {
        // Static — keeps the test focused on isolation rather than CPMM dynamics.
        self.spot_price_q64(base, quote)
    }
}

#[test]
fn three_distinct_base_mints_one_shared_quote_clear_independently() {
    let amm = PerPairAmm;
    let quote = pk(1);
    let base_a = pk(2);
    let base_b = pk(3);
    let base_c = pk(4);

    let result = batch_match(
        vec![
            // Pair A: 1 buy + 1 sell, perfectly balanced.
            buy(10, base_a, quote, 100),
            sell(20, base_a, quote, 100),
            // Pair B: heavy buy imbalance.
            buy(11, base_b, quote, 500),
            buy(12, base_b, quote, 500),
            sell(21, base_b, quote, 100),
            // Pair C: heavy sell imbalance.
            sell(22, base_c, quote, 500),
            sell(23, base_c, quote, 500),
            buy(13, base_c, quote, 100),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    // BTreeMap sort by (base, quote): pk(2) < pk(3) < pk(4).
    assert_eq!(result.len(), 3, "one ClearingResult per distinct base mint");
    assert_eq!(result[0].base_mint, base_a);
    assert_eq!(result[1].base_mint, base_b);
    assert_eq!(result[2].base_mint, base_c);

    // Each pair carries its own AMM-derived clearing price (encoded by base pubkey).
    // Static AMM ⇒ clearing == P_pre per pair.
    assert_eq!(result[0].clearing_price_q64, 1u128 << 64);
    assert_eq!(result[1].clearing_price_q64, 2u128 << 64);
    assert_eq!(result[2].clearing_price_q64, 3u128 << 64);

    // Pair A: balanced ⇒ no residual.
    assert!(result[0].residual.is_empty(), "pair A is balanced");
    // Pair B: buy-heavy ⇒ residual on the buy side.
    assert!(!result[1].residual.is_empty(), "pair B has buy residual");
    // Pair C: sell-heavy ⇒ residual on the sell side.
    assert!(!result[2].residual.is_empty(), "pair C has sell residual");
}

#[test]
fn pair_with_different_quote_is_isolated() {
    // Same base mint, two different quote mints. Two distinct (base, quote) tuples ⇒ two
    // ClearingResults.
    let amm = PerPairAmm;
    let base = pk(2);
    let quote_x = pk(1);
    let quote_y = pk(50);

    let result = batch_match(
        vec![
            buy(10, base, quote_x, 100),
            sell(20, base, quote_x, 100),
            buy(11, base, quote_y, 100),
            sell(21, base, quote_y, 100),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    assert_eq!(result.len(), 2, "distinct quote mints get distinct buckets");
    assert_eq!(result[0].base_mint, base);
    assert_eq!(result[1].base_mint, base);
    // Quote mints are sorted ascending by pubkey: pk(1) < pk(50).
    assert_eq!(result[0].quote_mint, quote_x);
    assert_eq!(result[1].quote_mint, quote_y);
}

#[test]
fn one_pair_imbalance_does_not_affect_other_pair_matches() {
    // If pair A and pair B are clearing concurrently, pair A's match should be identical
    // whether pair B is balanced or wildly imbalanced.
    let amm = PerPairAmm;
    let quote = pk(1);
    let base_a = pk(2);
    let base_b = pk(3);

    let pair_a_intents = vec![buy(10, base_a, quote, 100), sell(20, base_a, quote, 100)];

    // Run 1: pair A alone.
    let alone = batch_match(
        pair_a_intents.clone(),
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    // Run 2: pair A + a heavily skewed pair B in the same batch.
    let mut combined_intents = pair_a_intents;
    combined_intents.extend(vec![
        buy(11, base_b, quote, 1_000_000),
        buy(12, base_b, quote, 1_000_000),
        sell(21, base_b, quote, 1),
    ]);
    let combined = batch_match(
        combined_intents,
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    // Find pair A in both outputs.
    let alone_a = alone.iter().find(|r| r.base_mint == base_a).unwrap();
    let combined_a = combined.iter().find(|r| r.base_mint == base_a).unwrap();

    assert_eq!(
        alone_a, combined_a,
        "pair A's clearing must be byte-identical regardless of pair B's flow"
    );
}

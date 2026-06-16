//! Replay-invariance stress test.
//!
//! SPEC §6.4 ("Replay invariant"): for any input intent multiset `S`,
//! `batch_match(S, config, amm) == batch_match(any_permutation(S), config, amm)` byte-for-
//! byte.
//!
//! This test fixes a baseline intent set and runs `batch_match` against 100 random
//! permutations of it, asserting strict equality with the baseline output every time. The
//! shuffling PRNG is seeded so the test itself is reproducible.

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use solana_program::pubkey::Pubkey;
use staccana_matcher::*;

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn registry() -> QuoteRegistry {
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

struct StubAmm;

impl AmmAdapter for StubAmm {
    fn spot_price_q64(&self, base: &Pubkey, _quote: &Pubkey) -> u128 {
        // Make the spot price depend on the base mint so multi-pair determinism is exercised
        // (each pair has its own non-trivial price).
        let factor = base.as_ref()[0] as u128;
        factor << 63
    }
    fn simulate_post_price_q64(
        &self,
        base: &Pubkey,
        quote: &Pubkey,
        _amount: u64,
        _side: Side,
    ) -> u128 {
        // Static post-price keeps the test focused on input-ordering invariance, not AMM
        // dynamics. The clearing midpoint == P_pre under this adapter.
        self.spot_price_q64(base, quote)
    }
}

fn baseline_intents() -> Vec<SwapIntent> {
    let q1 = pk(1);
    let q2 = pk(50);
    let base_a = pk(2);
    let base_b = pk(3);
    let base_c = pk(4);
    vec![
        // Pair (base_a, q1): mixed sizes, multiple participants per side.
        buy(10, base_a, q1, 1000),
        buy(11, base_a, q1, 250),
        buy(12, base_a, q1, 750),
        sell(20, base_a, q1, 800),
        sell(21, base_a, q1, 400),
        sell(22, base_a, q1, 1200),
        // Pair (base_b, q1): unbalanced (sell-heavy).
        buy(13, base_b, q1, 100),
        sell(23, base_b, q1, 500),
        sell(24, base_b, q1, 600),
        // Pair (base_c, q2): different quote mint, balanced.
        buy(14, base_c, q2, 333),
        buy(15, base_c, q2, 333),
        sell(25, base_c, q2, 333),
        sell(26, base_c, q2, 333),
        // Pair (base_a, q2): same base, different quote — distinct bucket.
        buy(16, base_a, q2, 555),
        sell(27, base_a, q2, 555),
        // Tied amounts — exercise signer-ascending tiebreak.
        buy(30, base_b, q1, 200),
        buy(31, base_b, q1, 200),
        sell(40, base_b, q1, 200),
        sell(41, base_b, q1, 200),
    ]
}

#[test]
fn one_hundred_permutations_byte_identical_output() {
    let amm = StubAmm;
    let cfg = BatchConfig {
        registry: registry(),
    };
    let baseline_input = baseline_intents();
    let baseline_output = batch_match(baseline_input.clone(), &cfg, &amm);

    // Sanity: baseline produced something non-trivial — otherwise the test would
    // tautologically pass on empty output.
    assert!(
        !baseline_output.is_empty(),
        "baseline must produce at least one ClearingResult"
    );
    let total_matches: usize = baseline_output.iter().map(|r| r.matches.len()).sum();
    assert!(total_matches > 0, "baseline must contain matches");

    let mut rng = StdRng::seed_from_u64(42);
    for trial in 0..100 {
        let mut shuffled = baseline_input.clone();
        shuffled.shuffle(&mut rng);
        let permuted_output = batch_match(shuffled, &cfg, &amm);
        assert_eq!(
            permuted_output, baseline_output,
            "replay invariant failed on trial {}",
            trial
        );
    }
}

#[test]
fn empty_input_deterministic() {
    let amm = StubAmm;
    let cfg = BatchConfig {
        registry: registry(),
    };
    let a = batch_match(vec![], &cfg, &amm);
    let b = batch_match(vec![], &cfg, &amm);
    assert_eq!(a, b);
    assert!(a.is_empty());
}

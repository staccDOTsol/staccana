//! Integration tests covering the per-mint clearing algorithm against a realistic
//! constant-product AMM mock.
//!
//! Where the in-source unit tests use a fixed-spot stub (P_pre == P_post), this file
//! drives a mock AMM whose post-batch price actually moves with the net flow. That
//! exercises the midpoint(P_pre, P_post) clearing logic end-to-end and confirms the
//! algorithm behaves as the SPEC §6 contract describes when the AMM responds to the
//! batch's net direction.

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

/// Constant-product AMM: x * y = k. Spot price of base in quote = quote_reserve / base_reserve.
/// `simulate_post_price_q64` applies the swap to a copy of the reserves and returns the new
/// post-swap spot price. Pure simulation — does not mutate.
struct CpmmAmm {
    base_reserve: u128,
    quote_reserve: u128,
}

impl CpmmAmm {
    fn new(base_reserve: u128, quote_reserve: u128) -> Self {
        Self {
            base_reserve,
            quote_reserve,
        }
    }
}

impl AmmAdapter for CpmmAmm {
    fn spot_price_q64(&self, _base: &Pubkey, _quote: &Pubkey) -> u128 {
        // Q64.64 representation of quote_reserve / base_reserve.
        if self.base_reserve == 0 {
            return 0;
        }
        (self.quote_reserve << 64) / self.base_reserve
    }

    fn simulate_post_price_q64(
        &self,
        _base: &Pubkey,
        _quote: &Pubkey,
        amount: u64,
        side: Side,
    ) -> u128 {
        // x * y = k. After buying `amount` base out of the pool: base_reserve shrinks,
        // quote_reserve grows. After selling `amount` base into the pool: opposite.
        let amount = amount as u128;
        let (new_base, new_quote) = match side {
            Side::Buy => {
                let new_base = self.base_reserve.saturating_sub(amount);
                if new_base == 0 {
                    return u128::MAX;
                }
                let k = self.base_reserve * self.quote_reserve;
                let new_quote = k / new_base;
                (new_base, new_quote)
            }
            Side::Sell => {
                let new_base = self.base_reserve + amount;
                let k = self.base_reserve * self.quote_reserve;
                let new_quote = k / new_base;
                (new_base, new_quote)
            }
        };
        if new_base == 0 {
            return 0;
        }
        (new_quote << 64) / new_base
    }
}

#[test]
fn single_pair_no_residual() {
    // Net flow is zero (1000 buy quote vs 1000 sell base at price 1.0); P_pre == P_post.
    let amm = CpmmAmm::new(1_000_000, 1_000_000);
    let base = pk(2);
    let quote = pk(1);

    let result = batch_match(
        vec![buy(10, base, quote, 1000), sell(20, base, quote, 1000)],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    assert_eq!(result.len(), 1);
    let r = &result[0];
    assert_eq!(r.matches.len(), 1);
    assert_eq!(r.matches[0].base_amount, 1000);
    assert!(r.residual.is_empty(), "no residual when net flow is zero");
}

#[test]
fn partial_match_leaves_residual_for_amm() {
    // Buyer wants 200 quote of base, seller offers only 100 base. The excess 100 quote
    // becomes residual that the executor sends to the AMM.
    let amm = CpmmAmm::new(1_000_000, 1_000_000);
    let base = pk(2);
    let quote = pk(1);

    let result = batch_match(
        vec![buy(10, base, quote, 200), sell(20, base, quote, 100)],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert_eq!(r.matches.len(), 1);
    assert_eq!(r.matches[0].base_amount, 100);
    assert_eq!(r.residual.len(), 1, "buy excess goes to residual");
    assert_eq!(r.residual[0].signer, pk(10));
    // Residual amount is the leftover quote after the cross.
    assert!(r.residual[0].in_amount > 0);
}

#[test]
fn multiple_buyers_and_sellers_match_in_size_priority() {
    // Three buyers (300, 200, 100) and three sellers (250, 150, 100). Total buy quote ≈ 600,
    // total sell base = 500 — buy-heavy by 100 base. Largest pair off first.
    let amm = CpmmAmm::new(1_000_000, 1_000_000);
    let base = pk(2);
    let quote = pk(1);

    let result = batch_match(
        vec![
            buy(10, base, quote, 100),
            buy(11, base, quote, 200),
            buy(12, base, quote, 300),
            sell(20, base, quote, 100),
            sell(21, base, quote, 150),
            sell(22, base, quote, 250),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert!(!r.matches.is_empty(), "should produce at least one match");
    // Size-priority: the largest buyer (signer 12, 300 quote) crosses with the largest
    // seller (signer 22, 250 base) first.
    assert_eq!(r.matches[0].buyer, pk(12));
    assert_eq!(r.matches[0].seller, pk(22));
}

#[test]
fn asymmetric_more_buyers_than_sellers() {
    // Five buyers, one small seller. The vast majority of buy quote falls through as
    // residual to the AMM.
    let amm = CpmmAmm::new(1_000_000, 1_000_000);
    let base = pk(2);
    let quote = pk(1);

    let result = batch_match(
        vec![
            buy(10, base, quote, 100),
            buy(11, base, quote, 100),
            buy(12, base, quote, 100),
            buy(13, base, quote, 100),
            buy(14, base, quote, 100),
            sell(20, base, quote, 50),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    // One match (largest buyer with the seller's 50 base), the other four buyers + leftover
    // from the matched buyer all go to residual.
    assert_eq!(r.matches.len(), 1);
    assert!(r.residual.len() >= 4, "unmatched buyers go to residual");
}

#[test]
fn asymmetric_more_sellers_than_buyers() {
    // One small buyer, five sellers. Most sell base goes to AMM as residual.
    let amm = CpmmAmm::new(1_000_000, 1_000_000);
    let base = pk(2);
    let quote = pk(1);

    let result = batch_match(
        vec![
            buy(10, base, quote, 50),
            sell(20, base, quote, 100),
            sell(21, base, quote, 100),
            sell(22, base, quote, 100),
            sell(23, base, quote, 100),
            sell(24, base, quote, 100),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert_eq!(r.matches.len(), 1);
    assert!(r.residual.len() >= 4, "unmatched sellers go to residual");
}

#[test]
fn buy_heavy_clearing_price_above_pre() {
    // Heavy net buy pressure pushes the AMM's post-price up; the clearing midpoint should
    // sit above P_pre.
    let amm = CpmmAmm::new(1_000_000, 1_000_000);
    let base = pk(2);
    let quote = pk(1);
    let p_pre = amm.spot_price_q64(&base, &quote);

    let result = batch_match(
        vec![
            buy(10, base, quote, 100_000),
            buy(11, base, quote, 100_000),
            sell(20, base, quote, 1_000),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert!(
        r.clearing_price_q64 > p_pre,
        "buy-heavy batch must clear above P_pre (got {} vs pre {})",
        r.clearing_price_q64,
        p_pre
    );
}

#[test]
fn sell_heavy_clearing_price_below_pre() {
    // Heavy net sell pressure pulls the AMM's post-price down; midpoint sits below P_pre.
    let amm = CpmmAmm::new(1_000_000, 1_000_000);
    let base = pk(2);
    let quote = pk(1);
    let p_pre = amm.spot_price_q64(&base, &quote);

    let result = batch_match(
        vec![
            sell(20, base, quote, 100_000),
            sell(21, base, quote, 100_000),
            buy(10, base, quote, 1_000),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert!(
        r.clearing_price_q64 < p_pre,
        "sell-heavy batch must clear below P_pre (got {} vs pre {})",
        r.clearing_price_q64,
        p_pre
    );
}

#[test]
fn balanced_flow_clearing_price_at_pre() {
    // Net flow is exactly zero — buyers and sellers cancel — so P_post == P_pre and the
    // clearing midpoint == P_pre.
    let amm = CpmmAmm::new(1_000_000, 1_000_000);
    let base = pk(2);
    let quote = pk(1);
    let p_pre = amm.spot_price_q64(&base, &quote);

    let result = batch_match(
        vec![
            buy(10, base, quote, 1000),
            buy(11, base, quote, 1000),
            sell(20, base, quote, 1000),
            sell(21, base, quote, 1000),
        ],
        &BatchConfig {
            registry: registry(),
        },
        &amm,
    );

    let r = &result[0];
    assert_eq!(
        r.clearing_price_q64, p_pre,
        "zero-net flow should clear at P_pre"
    );
}

//! Per-mint batch matcher.
//!
//! Algorithm (per (base, quote) group within a slot):
//!
//! 1. Partition intents into buys (out_mint == base) and sells (in_mint == base).
//! 2. Compute net base demand: Σ(buys translated to base via P_pre) − Σ(sells in base).
//! 3. Hit the AMM with the net only → P_post.
//! 4. Clearing price = midpoint of P_pre and P_post.
//! 5. Sort each side by (amount desc, signer asc); pair-match largest-first at the
//!    clearing price.
//! 6. Whatever doesn't match becomes residual that the executor sends to the AMM.
//!
//! All math is integer (Q64.64 fixed-point); no floats, no `f64`, no nondeterminism.

use crate::amm::AmmAdapter;
use crate::intent::{Side, SwapIntent};
use crate::quote_registry::QuoteRegistry;
use serde::{Deserialize, Serialize};
use solana_program::pubkey::Pubkey;
use std::collections::BTreeMap;

/// A single bilateral cross between a buyer and a seller at the batch's clearing price.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub buyer: Pubkey,
    pub seller: Pubkey,
    pub base_amount: u64,
    pub quote_amount: u64,
}

/// Output of clearing one (base, quote) pair within a slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearingResult {
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub clearing_price_q64: u128,
    pub matches: Vec<Match>,
    pub residual: Vec<SwapIntent>,
}

#[derive(Clone, Debug)]
pub struct BatchConfig {
    pub registry: QuoteRegistry,
}

/// Group intents by (base_mint, quote_mint) and run the per-pair clearing for each.
///
/// Output is sorted deterministically (by base then quote pubkey) so any validator
/// replaying the same input set produces the same byte-identical result.
pub fn batch_match<A: AmmAdapter>(
    intents: Vec<SwapIntent>,
    config: &BatchConfig,
    amm: &A,
) -> Vec<ClearingResult> {
    let mut groups: BTreeMap<(Pubkey, Pubkey), Vec<SwapIntent>> = BTreeMap::new();

    for intent in intents {
        let Some((base, quote)) = classify_pair(&intent, &config.registry) else {
            continue;
        };
        groups.entry((base, quote)).or_default().push(intent);
    }

    groups
        .into_iter()
        .filter_map(|((base, quote), intents)| clear_pair(base, quote, intents, amm))
        .collect()
}

/// Decide which mint is base (longtail) and which is quote, given the registry.
///
/// Rules:
/// * Exactly one is a registered quote → that's the quote, the other is base.
/// * Both quote (e.g. USDC/USDT) → smaller lex pubkey is base, larger is quote.
///   Will replace lex tiebreak with a liquidity-rank tiebreak in production.
/// * Neither quote → smaller lex is base. Same caveat.
fn classify_pair(intent: &SwapIntent, registry: &QuoteRegistry) -> Option<(Pubkey, Pubkey)> {
    let in_quote = registry.is_quote(&intent.in_mint);
    let out_quote = registry.is_quote(&intent.out_mint);

    let pair = match (in_quote, out_quote) {
        (true, false) => (intent.out_mint, intent.in_mint),
        (false, true) => (intent.in_mint, intent.out_mint),
        _ => {
            if intent.in_mint < intent.out_mint {
                (intent.in_mint, intent.out_mint)
            } else {
                (intent.out_mint, intent.in_mint)
            }
        }
    };
    Some(pair)
}

fn clear_pair<A: AmmAdapter>(
    base: Pubkey,
    quote: Pubkey,
    intents: Vec<SwapIntent>,
    amm: &A,
) -> Option<ClearingResult> {
    if intents.is_empty() {
        return None;
    }

    // Partition by side relative to base mint. Anything that isn't a clean
    // base↔quote intent (shouldn't happen post-classify) is dropped.
    let mut buys: Vec<SwapIntent> = Vec::new();
    let mut sells: Vec<SwapIntent> = Vec::new();
    for intent in intents {
        if intent.out_mint == base && intent.in_mint == quote {
            buys.push(intent);
        } else if intent.in_mint == base && intent.out_mint == quote {
            sells.push(intent);
        }
    }

    let p_pre = amm.spot_price_q64(&base, &quote);

    // Convert to base units to compute net flow.
    let base_demanded: u128 = buys
        .iter()
        .map(|b| quote_to_base_q64(b.in_amount, p_pre))
        .sum();
    let base_offered: u128 = sells.iter().map(|s| s.in_amount as u128).sum();

    let (net_amount, net_side) = if base_demanded > base_offered {
        (base_demanded - base_offered, Side::Buy)
    } else {
        (base_offered - base_demanded, Side::Sell)
    };

    // Truncation here is fine — `net_amount` fits in u64 in any practical batch and the
    // AMM adapter is responsible for clamping if the net would blow past liquidity.
    let net_clamped = net_amount.min(u64::MAX as u128) as u64;
    let p_post = amm.simulate_post_price_q64(&base, &quote, net_clamped, net_side);

    // Clearing price: midpoint of pre and post, computed without overflow.
    let clearing_price_q64 = midpoint(p_pre, p_post);

    // Size-priority sort: largest amount first, signer pubkey ascending as tiebreak.
    buys.sort_by(|a, b| {
        b.in_amount
            .cmp(&a.in_amount)
            .then_with(|| a.signer.cmp(&b.signer))
    });
    sells.sort_by(|a, b| {
        b.in_amount
            .cmp(&a.in_amount)
            .then_with(|| a.signer.cmp(&b.signer))
    });

    let mut matches: Vec<Match> = Vec::new();
    let mut residual: Vec<SwapIntent> = Vec::new();

    let mut buy_iter = buys.into_iter();
    let mut sell_iter = sells.into_iter();

    let mut buy_remaining_quote: u64 = 0;
    let mut sell_remaining_base: u64 = 0;
    let mut current_buyer: Option<Pubkey> = None;
    let mut current_seller: Option<Pubkey> = None;

    loop {
        if buy_remaining_quote == 0 {
            match buy_iter.next() {
                Some(b) => {
                    buy_remaining_quote = b.in_amount;
                    current_buyer = Some(b.signer);
                }
                None => break,
            }
        }
        if sell_remaining_base == 0 {
            match sell_iter.next() {
                Some(s) => {
                    sell_remaining_base = s.in_amount;
                    current_seller = Some(s.signer);
                }
                None => {
                    if buy_remaining_quote > 0 {
                        residual.push(SwapIntent {
                            signer: current_buyer.expect("buyer set above"),
                            in_mint: quote,
                            in_amount: buy_remaining_quote,
                            out_mint: base,
                            min_out: 0,
                            nonce: 0,
                        });
                    }
                    break;
                }
            }
        }

        let buyer_base_capacity =
            quote_to_base_q64(buy_remaining_quote, clearing_price_q64).min(u64::MAX as u128) as u64;
        let cross_base = buyer_base_capacity.min(sell_remaining_base);

        if cross_base == 0 {
            // Buyer's remaining quote can't afford a single base unit at the clearing
            // price — push the dust to residual and move on.
            residual.push(SwapIntent {
                signer: current_buyer.expect("buyer set above"),
                in_mint: quote,
                in_amount: buy_remaining_quote,
                out_mint: base,
                min_out: 0,
                nonce: 0,
            });
            buy_remaining_quote = 0;
            continue;
        }

        let cross_quote = base_to_quote_q64_ceil(cross_base, clearing_price_q64)
            .min(buy_remaining_quote as u128)
            .min(u64::MAX as u128) as u64;
        if cross_quote == 0 {
            residual.push(SwapIntent {
                signer: current_buyer.expect("buyer set above"),
                in_mint: quote,
                in_amount: buy_remaining_quote,
                out_mint: base,
                min_out: 0,
                nonce: 0,
            });
            buy_remaining_quote = 0;
            continue;
        }

        matches.push(Match {
            buyer: current_buyer.expect("buyer set above"),
            seller: current_seller.expect("seller set above"),
            base_amount: cross_base,
            quote_amount: cross_quote,
        });

        buy_remaining_quote = buy_remaining_quote.saturating_sub(cross_quote);
        sell_remaining_base = sell_remaining_base.saturating_sub(cross_base);
    }

    if sell_remaining_base > 0 {
        residual.push(SwapIntent {
            signer: current_seller.expect("seller set above"),
            in_mint: base,
            in_amount: sell_remaining_base,
            out_mint: quote,
            min_out: 0,
            nonce: 0,
        });
    }
    for s in sell_iter {
        residual.push(s);
    }
    for b in buy_iter {
        residual.push(b);
    }

    Some(ClearingResult {
        base_mint: base,
        quote_mint: quote,
        clearing_price_q64,
        matches,
        residual,
    })
}

#[inline]
fn quote_to_base_q64(quote_amount: u64, price_q64: u128) -> u128 {
    if price_q64 == 0 {
        return 0;
    }
    ((quote_amount as u128) << 64) / price_q64
}

#[inline]
fn base_to_quote_q64_ceil(base_amount: u64, price_q64: u128) -> u128 {
    let numerator = (base_amount as u128).saturating_mul(price_q64);
    let whole = numerator >> 64;
    let remainder = numerator & ((1u128 << 64) - 1);
    whole.saturating_add(u128::from(remainder != 0))
}

#[inline]
fn midpoint(a: u128, b: u128) -> u128 {
    // a/2 + b/2 + (a%2 + b%2)/2 — overflow-safe.
    (a >> 1) + (b >> 1) + ((a & 1) + (b & 1)) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    struct StubAmm {
        spot_q64: u128,
        post_q64: u128,
    }

    impl AmmAdapter for StubAmm {
        fn spot_price_q64(&self, _: &Pubkey, _: &Pubkey) -> u128 {
            self.spot_q64
        }
        fn simulate_post_price_q64(&self, _: &Pubkey, _: &Pubkey, _: u64, _: Side) -> u128 {
            self.post_q64
        }
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

    #[test]
    fn empty_batch_returns_no_results() {
        let amm = StubAmm {
            spot_q64: 1u128 << 64,
            post_q64: 1u128 << 64,
        };
        let result = batch_match(
            vec![],
            &BatchConfig {
                registry: registry(),
            },
            &amm,
        );
        assert!(result.is_empty());
    }

    #[test]
    fn perfect_cross_no_residual() {
        let amm = StubAmm {
            spot_q64: 1u128 << 64,
            post_q64: 1u128 << 64,
        };
        let base = pk(2);
        let quote = pk(1);

        let result = batch_match(
            vec![buy(10, base, quote, 100), sell(20, base, quote, 100)],
            &BatchConfig {
                registry: registry(),
            },
            &amm,
        );

        assert_eq!(result.len(), 1);
        let r = &result[0];
        assert_eq!(r.base_mint, base);
        assert_eq!(r.quote_mint, quote);
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].base_amount, 100);
        assert_eq!(r.matches[0].quote_amount, 100);
        assert!(r.residual.is_empty());
    }

    #[test]
    fn buy_heavy_pushes_excess_to_residual() {
        let amm = StubAmm {
            spot_q64: 1u128 << 64,
            post_q64: 1u128 << 64,
        };
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
        assert_eq!(r.residual.len(), 1);
        assert_eq!(r.residual[0].signer, pk(10));
        assert_eq!(r.residual[0].in_amount, 100);
    }

    #[test]
    fn sub_unit_price_cross_never_settles_zero_quote() {
        let amm = StubAmm {
            spot_q64: 1u128 << 63,
            post_q64: 1u128 << 63,
        };
        let base = pk(2);
        let quote = pk(1);

        let result = batch_match(
            vec![buy(10, base, quote, 1), sell(20, base, quote, 1)],
            &BatchConfig {
                registry: registry(),
            },
            &amm,
        );

        let r = &result[0];
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].base_amount, 1);
        assert_eq!(r.matches[0].quote_amount, 1);
    }

    #[test]
    fn determinism_under_input_reordering() {
        let amm = StubAmm {
            spot_q64: 1u128 << 64,
            post_q64: 1u128 << 64,
        };
        let base = pk(2);
        let quote = pk(1);

        let intents_a = vec![
            buy(10, base, quote, 100),
            sell(20, base, quote, 100),
            buy(30, base, quote, 50),
            sell(40, base, quote, 50),
        ];
        let intents_b = vec![
            sell(40, base, quote, 50),
            sell(20, base, quote, 100),
            buy(30, base, quote, 50),
            buy(10, base, quote, 100),
        ];

        let cfg = BatchConfig {
            registry: registry(),
        };
        let result_a = batch_match(intents_a, &cfg, &amm);
        let result_b = batch_match(intents_b, &cfg, &amm);

        assert_eq!(
            result_a, result_b,
            "replay invariant: same input set ⇒ same output regardless of order"
        );
    }

    #[test]
    fn size_priority_largest_buyer_matches_first() {
        let amm = StubAmm {
            spot_q64: 1u128 << 64,
            post_q64: 1u128 << 64,
        };
        let base = pk(2);
        let quote = pk(1);

        let result = batch_match(
            vec![
                buy(10, base, quote, 50),
                buy(11, base, quote, 100),
                buy(12, base, quote, 200),
                sell(20, base, quote, 200),
            ],
            &BatchConfig {
                registry: registry(),
            },
            &amm,
        );

        let r = &result[0];
        assert_eq!(r.matches[0].buyer, pk(12));
        assert_eq!(r.matches[0].base_amount, 200);
        assert_eq!(r.residual.len(), 2);
    }

    #[test]
    fn separate_pairs_clear_independently() {
        // Two distinct base mints (pk(2), pk(3)) both quoted in pk(1) — get separate
        // ClearingResults, each deterministic.
        let amm = StubAmm {
            spot_q64: 1u128 << 64,
            post_q64: 1u128 << 64,
        };
        let quote = pk(1);
        let base_a = pk(2);
        let base_b = pk(3);

        let result = batch_match(
            vec![
                buy(10, base_a, quote, 100),
                sell(20, base_a, quote, 100),
                buy(30, base_b, quote, 50),
                sell(40, base_b, quote, 50),
            ],
            &BatchConfig {
                registry: registry(),
            },
            &amm,
        );

        assert_eq!(result.len(), 2);
        // BTreeMap orders by (base, quote) ascending — pk(2) before pk(3).
        assert_eq!(result[0].base_mint, base_a);
        assert_eq!(result[1].base_mint, base_b);
    }
}

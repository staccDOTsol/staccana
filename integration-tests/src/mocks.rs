//! Mocks for the integration tests.
//!
//! The headline mock is [`ConstantProductAmm`] — a multi-pool, real-feeling x*y=k AMM
//! that implements [`staccana_matcher::AmmAdapter`]. Unlike the `StubAmm` in
//! `matcher/src/batch.rs`'s unit tests (which returns the same `spot` and `post` price
//! regardless of input), `ConstantProductAmm` carries actual reserves per (base, quote)
//! pair and computes a meaningful post-trade price using the canonical product invariant.
//!
//! The AMM is deliberately stateless from the matcher's perspective: `spot_price_q64`
//! and `simulate_post_price_q64` are read-only — the matcher never mutates the AMM. Tests
//! that want to model "the residual hit the AMM" can update reserves manually after
//! [`staccana_matcher::batch_match`] returns.

use solana_program::pubkey::Pubkey;
use staccana_matcher::{AmmAdapter, Side};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Reserves for one (base, quote) pool. Both balances are u128 so we don't have to worry
/// about edge-case overflows when computing `(base * 1<<64) / quote` for the spot price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstantProductPool {
    pub base_reserve: u128,
    pub quote_reserve: u128,
}

impl ConstantProductPool {
    pub fn new(base_reserve: u128, quote_reserve: u128) -> Self {
        Self {
            base_reserve,
            quote_reserve,
        }
    }

    /// Spot price of base in quote, expressed in Q64.64.
    ///
    /// `price = quote_reserve / base_reserve`, scaled up by 2^64 so the integer division
    /// keeps a usable fractional part. Empty pool returns 0.
    pub fn spot_price_q64(&self) -> u128 {
        if self.base_reserve == 0 {
            return 0;
        }
        // (quote_reserve << 64) / base_reserve. We assume reserves fit comfortably below
        // 2^64 in any realistic test fixture; on overflow we saturate, which is loud
        // enough for test failures to point at the right place.
        self.quote_reserve
            .checked_shl(64)
            .map(|num| num / self.base_reserve)
            .unwrap_or(u128::MAX)
    }

    /// Simulate a swap of `amount` units of base on `side` and return the resulting spot
    /// price (Q64.64). Pure simulation — `self` is not mutated.
    ///
    /// `side == Buy` ⇒ the trader removes `amount` base from the pool (and adds the
    /// matching quote, computed from the constant-product invariant `k = base * quote`).
    /// `side == Sell` ⇒ the trader adds `amount` base to the pool (and removes quote).
    pub fn simulate_post_price_q64(&self, amount: u64, side: Side) -> u128 {
        let amt = amount as u128;
        if self.base_reserve == 0 || self.quote_reserve == 0 {
            return self.spot_price_q64();
        }
        let k = self.base_reserve.saturating_mul(self.quote_reserve);
        let (new_base, new_quote) = match side {
            Side::Buy => {
                if amt >= self.base_reserve {
                    // Trying to drain the pool — clamp to one unit remaining so we still
                    // produce a finite price for the test.
                    let new_base = 1u128;
                    let new_quote = k / new_base;
                    (new_base, new_quote)
                } else {
                    let new_base = self.base_reserve - amt;
                    let new_quote = k / new_base;
                    (new_base, new_quote)
                }
            }
            Side::Sell => {
                let new_base = self.base_reserve.saturating_add(amt);
                let new_quote = k / new_base;
                (new_base, new_quote)
            }
        };
        let provisional = ConstantProductPool {
            base_reserve: new_base,
            quote_reserve: new_quote,
        };
        provisional.spot_price_q64()
    }
}

/// Multi-pool constant-product AMM. Pools are keyed by `(base, quote)` ordered exactly
/// the way the matcher will request them — no automatic mirror-pool lookup, because that
/// would let two equally-valid keying schemes coexist silently.
pub struct ConstantProductAmm {
    pools: Mutex<BTreeMap<(Pubkey, Pubkey), ConstantProductPool>>,
}

impl ConstantProductAmm {
    pub fn new() -> Self {
        Self {
            pools: Mutex::new(BTreeMap::new()),
        }
    }

    /// Insert (or overwrite) a pool keyed by `(base, quote)`.
    pub fn add_pool(&self, base: Pubkey, quote: Pubkey, pool: ConstantProductPool) {
        self.pools
            .lock()
            .expect("amm pool lock")
            .insert((base, quote), pool);
    }

    /// Read a pool back for assertions. Returns `None` if the pool isn't present.
    pub fn pool(&self, base: &Pubkey, quote: &Pubkey) -> Option<ConstantProductPool> {
        self.pools
            .lock()
            .expect("amm pool lock")
            .get(&(*base, *quote))
            .copied()
    }

    /// Apply a swap to a pool, mutating its reserves. Tests use this to simulate the
    /// residual flow after the matcher returns.
    ///
    /// `Buy` removes base and adds quote; `Sell` adds base and removes quote. The
    /// quote-leg amount is computed from the constant-product invariant just as
    /// `simulate_post_price_q64` does internally.
    pub fn apply_swap(&self, base: &Pubkey, quote: &Pubkey, amount: u64, side: Side) {
        let mut guard = self.pools.lock().expect("amm pool lock");
        let pool = guard.get_mut(&(*base, *quote)).expect("pool present");
        let amt = amount as u128;
        let k = pool.base_reserve.saturating_mul(pool.quote_reserve);
        match side {
            Side::Buy => {
                let new_base = pool.base_reserve.saturating_sub(amt).max(1);
                pool.base_reserve = new_base;
                pool.quote_reserve = k / new_base;
            }
            Side::Sell => {
                let new_base = pool.base_reserve.saturating_add(amt);
                pool.base_reserve = new_base;
                pool.quote_reserve = k / new_base;
            }
        }
    }
}

impl Default for ConstantProductAmm {
    fn default() -> Self {
        Self::new()
    }
}

impl AmmAdapter for ConstantProductAmm {
    fn spot_price_q64(&self, base: &Pubkey, quote: &Pubkey) -> u128 {
        self.pools
            .lock()
            .expect("amm pool lock")
            .get(&(*base, *quote))
            .copied()
            .map(|p| p.spot_price_q64())
            .unwrap_or(1u128 << 64) // 1.0 if no pool — keeps tests with empty AMMs sensible.
    }

    fn simulate_post_price_q64(
        &self,
        base: &Pubkey,
        quote: &Pubkey,
        amount: u64,
        side: Side,
    ) -> u128 {
        self.pools
            .lock()
            .expect("amm pool lock")
            .get(&(*base, *quote))
            .copied()
            .map(|p| p.simulate_post_price_q64(amount, side))
            .unwrap_or(1u128 << 64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn empty_amm_returns_one_for_unknown_pair() {
        let amm = ConstantProductAmm::new();
        assert_eq!(amm.spot_price_q64(&pk(1), &pk(2)), 1u128 << 64);
    }

    #[test]
    fn balanced_pool_has_unit_price() {
        let amm = ConstantProductAmm::new();
        amm.add_pool(pk(1), pk(2), ConstantProductPool::new(1_000, 1_000));
        assert_eq!(amm.spot_price_q64(&pk(1), &pk(2)), 1u128 << 64);
    }

    #[test]
    fn buy_increases_quote_per_base_price() {
        // Removing base shrinks the supply → price (quote per base) should rise.
        let amm = ConstantProductAmm::new();
        amm.add_pool(pk(1), pk(2), ConstantProductPool::new(1_000, 1_000));
        let pre = amm.spot_price_q64(&pk(1), &pk(2));
        let post = amm.simulate_post_price_q64(&pk(1), &pk(2), 100, Side::Buy);
        assert!(
            post > pre,
            "buy should push price up: pre={pre} post={post}"
        );
    }

    #[test]
    fn sell_decreases_quote_per_base_price() {
        let amm = ConstantProductAmm::new();
        amm.add_pool(pk(1), pk(2), ConstantProductPool::new(1_000, 1_000));
        let pre = amm.spot_price_q64(&pk(1), &pk(2));
        let post = amm.simulate_post_price_q64(&pk(1), &pk(2), 100, Side::Sell);
        assert!(
            post < pre,
            "sell should push price down: pre={pre} post={post}"
        );
    }

    #[test]
    fn simulate_does_not_mutate_pool() {
        let amm = ConstantProductAmm::new();
        amm.add_pool(pk(1), pk(2), ConstantProductPool::new(1_000, 1_000));
        let before = amm.pool(&pk(1), &pk(2)).unwrap();
        let _ = amm.simulate_post_price_q64(&pk(1), &pk(2), 100, Side::Buy);
        let after = amm.pool(&pk(1), &pk(2)).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn apply_swap_does_mutate_pool() {
        let amm = ConstantProductAmm::new();
        amm.add_pool(pk(1), pk(2), ConstantProductPool::new(1_000, 1_000));
        amm.apply_swap(&pk(1), &pk(2), 100, Side::Buy);
        let after = amm.pool(&pk(1), &pk(2)).unwrap();
        assert!(after.base_reserve < 1_000);
        assert!(after.quote_reserve > 1_000);
    }
}

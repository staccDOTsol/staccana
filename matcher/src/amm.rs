//! Trait the matcher uses to interact with whatever AMM provides residual liquidity.
//!
//! The matcher itself is AMM-agnostic — it asks for spot price and simulates a net-flow swap
//! to get the post-batch price, then clears at the midpoint. Implementations wrap forked
//! Raydium (AMM v4, CLMM, CPMM) for v1.

use crate::intent::Side;
use solana_program::pubkey::Pubkey;

pub trait AmmAdapter {
    /// Spot price of `base` in units of `quote`, expressed in Q64.64 fixed point.
    ///
    /// "Q64.64" means the high 64 bits are the integer part and the low 64 bits are the
    /// fractional part. A price of exactly 1.0 is `1u128 << 64`.
    fn spot_price_q64(&self, base: &Pubkey, quote: &Pubkey) -> u128;

    /// Simulate a swap of `amount` units of base on `side` and return the resulting spot
    /// price (Q64.64). Pure simulation — does not mutate AMM state.
    fn simulate_post_price_q64(
        &self,
        base: &Pubkey,
        quote: &Pubkey,
        amount: u64,
        side: Side,
    ) -> u128;
}

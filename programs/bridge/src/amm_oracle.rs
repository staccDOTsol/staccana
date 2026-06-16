//! AMM oracle for native staccana SOL ↔ wSOL conversions.
//!
//! The bridge uses an on-chain constant-product AMM pool (target: secret-ray's
//! `wSOL ↔ native-SOL` pool) as the price oracle for the conversion instructions
//! [`crate::instructions::convert_native_to_wsol`] and
//! [`crate::instructions::convert_wsol_to_native`].
//!
//! This is **not a peg**. Per `docs/BRIDGE.md` §"Native SOL ↔ mainnet SOL via the
//! bridge (uncorrelated, AMM-quoted)", the bridge always quotes at the current AMM
//! mid-price. Round-trips close at AMM slippage + 2× bridge fees, identical to a
//! direct AMM trade. There is no fixed rate to defend, no arbitrage to extract,
//! no UST/LUNA-style death spiral surface.
//!
//! ## Pool reading
//!
//! In production the bridge reads reserves from the secret-ray pool account.
//! secret-ray hasn't been built yet (see workspace `Cargo.toml` — the crate is
//! commented out as a "Planned crate"). The conversion instructions therefore
//! accept the pool account as `UncheckedAccount` and the actual reserve-decoding
//! call site is marked TODO. The pure math here is reserve-shape agnostic and is
//! exercised by unit tests with hand-rolled reserve pairs.
//!
//! ## Pool placeholder
//!
//! While secret-ray is in development, deployments wire the conversion ixs to
//! the `PLACEHOLDER_POOL` PDA below. Mainnet rollout MUST replace this with the
//! real pool key before the wSOL ↔ native-SOL flow goes live.

use crate::error::BridgeError;
use anchor_lang::prelude::{
    AccountInfo, AnchorDeserialize, AnchorSerialize,
};

/// Placeholder PDA seed for the secret-ray `wSOL ↔ native-SOL` pool. The real pool
/// account doesn't exist yet — the bridge accepts `UncheckedAccount<'info>` for the
/// pool and a follow-up PR will:
///
/// 1. Replace this seed with secret-ray's actual pool derivation.
/// 2. Decode reserves from the pool account data instead of taking them as ix args.
/// 3. CPI into secret-ray's `swap` ix instead of the manual quote-then-transfer flow.
pub const PLACEHOLDER_POOL_SEED: &[u8] = b"secret_ray_wsol_native_pool";

/// AMM pool reserves snapshot, expressed in lamports for both legs (both wSOL and
/// native staccana SOL share SOL's 9-decimal scale, so reserve units match).
///
/// The constant-product invariant `reserve_in * reserve_out = k` is preserved across
/// swaps (modulo fees). For the bridge oracle we only need the spot quote, not the
/// post-swap reserves, so the math here is the standard `dy = (y * dx) / (x + dx)`
/// with no protocol fee deducted (the AMM's own swap-fee math runs inside secret-ray).
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct PoolReserves {
    /// wSOL side reserve, in lamports.
    pub reserve_wsol: u64,
    /// Native staccana SOL side reserve, in lamports.
    pub reserve_native: u64,
}

/// Quote `dx` lamports of native staccana SOL → lamports of wSOL using the
/// constant-product formula:
///
/// ```text
/// dy = (reserve_wsol * dx_native) / (reserve_native + dx_native)
/// ```
///
/// Returns the gross output in lamports (no bridge fee applied). Returns
/// [`BridgeError::AmmEmptyReserves`] if either reserve is zero.
pub fn quote_native_to_wsol(
    reserves: PoolReserves,
    dx_native: u64,
) -> Result<u64, BridgeError> {
    if reserves.reserve_wsol == 0 || reserves.reserve_native == 0 {
        return Err(BridgeError::AmmEmptyReserves);
    }
    if dx_native == 0 {
        return Ok(0);
    }
    let dx = dx_native as u128;
    let rw = reserves.reserve_wsol as u128;
    let rn = reserves.reserve_native as u128;

    // numerator = rw * dx; denominator = rn + dx. Both fit in u128 trivially since each
    // input is u64.
    let numer = rw
        .checked_mul(dx)
        .ok_or(BridgeError::AmmQuoteOverflow)?;
    let denom = rn
        .checked_add(dx)
        .ok_or(BridgeError::AmmQuoteOverflow)?;
    let dy = numer / denom;
    u64::try_from(dy).map_err(|_| BridgeError::AmmQuoteOverflow)
}

/// Quote `dx` lamports of wSOL → lamports of native staccana SOL. Mirror of
/// [`quote_native_to_wsol`] with reserves swapped:
///
/// ```text
/// dy = (reserve_native * dx_wsol) / (reserve_wsol + dx_wsol)
/// ```
pub fn quote_wsol_to_native(
    reserves: PoolReserves,
    dx_wsol: u64,
) -> Result<u64, BridgeError> {
    if reserves.reserve_wsol == 0 || reserves.reserve_native == 0 {
        return Err(BridgeError::AmmEmptyReserves);
    }
    if dx_wsol == 0 {
        return Ok(0);
    }
    let dx = dx_wsol as u128;
    let rw = reserves.reserve_wsol as u128;
    let rn = reserves.reserve_native as u128;

    let numer = rn
        .checked_mul(dx)
        .ok_or(BridgeError::AmmQuoteOverflow)?;
    let denom = rw
        .checked_add(dx)
        .ok_or(BridgeError::AmmQuoteOverflow)?;
    let dy = numer / denom;
    u64::try_from(dy).map_err(|_| BridgeError::AmmQuoteOverflow)
}

/// TODO(secret-ray): once the AMM crate lands, replace the `_pool: &AccountInfo` form
/// with a typed `Account<'info, SecretRayPool>` (or similar) and decode reserves from
/// the account data rather than accepting them as ix args. Until then this helper is
/// a documentation hook and the conversion ixs accept `PoolReserves` directly.
#[allow(dead_code)]
pub fn read_reserves_placeholder(_pool: &AccountInfo) -> Result<PoolReserves, BridgeError> {
    // Intentionally unimplemented — would deserialize secret-ray's pool layout.
    Err(BridgeError::AmmEmptyReserves)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_native_to_wsol_at_unit_pool_is_near_par() {
        // Equal reserves → spot price is 1.0. A small dx should return ~dx (less the
        // tiny price-impact term). dx == 1, rw == rn == 1_000_000 → dy = 1*1_000_000 /
        // (1_000_000 + 1) = 0 (truncates). dx == 100 → 100*1_000_000 / 1_000_100 = 99.
        let r = PoolReserves {
            reserve_wsol: 1_000_000,
            reserve_native: 1_000_000,
        };
        assert_eq!(quote_native_to_wsol(r, 100).unwrap(), 99);
    }

    #[test]
    fn quote_native_to_wsol_cheap_chain_returns_few_wsol() {
        // Worthless-staccana scenario from the doc: P = wSOL per native = ~1e-6.
        // Pool: 1 wSOL : 1_000_000 native. Selling 1_000 native gets you ~1e-3 wSOL.
        // dy = 1 * 1_000 / (1_000_000 + 1_000) = 0 (truncates because the units are
        // 1 lamport-wSOL : 1M lamport-native — single lamports vanish at this rate).
        // Use a more realistic 1e9 : 1e15 reserves so the 1e-6 quote produces non-zero.
        let r = PoolReserves {
            reserve_wsol: 1_000_000_000,       // 1 SOL of wSOL
            reserve_native: 1_000_000_000_000_000, // 1M SOL of native
        };
        // Sell 1_000 lamports of native → expect ~ 1_000 * 1e-6 = 0.001 lamports →
        // truncates to 0. Sell 1_000_000_000 lamports → expect ~ 1_000 lamport wSOL.
        let dy = quote_native_to_wsol(r, 1_000_000_000).unwrap();
        // Constant-product: dy = (1e9 * 1e9) / (1e15 + 1e9) ≈ 999 (slight slippage).
        assert!(dy > 0 && dy < 1_001, "dy was {}", dy);
    }

    #[test]
    fn quote_wsol_to_native_inverse_of_native_to_wsol() {
        // For tiny dx relative to reserves the price is symmetric: selling 100 native
        // for wSOL and then selling that wSOL back for native should recover ≈ 100
        // native (minus ~2× the slippage term).
        let r = PoolReserves {
            reserve_wsol: 100_000_000_000,
            reserve_native: 100_000_000_000,
        };
        let dx_native = 1_000_000;
        let dy_wsol = quote_native_to_wsol(r, dx_native).unwrap();
        let dx_native_back = quote_wsol_to_native(r, dy_wsol).unwrap();
        // Two-leg slippage on a 1M / 100B trade should be < 100 lamports.
        assert!(
            dx_native.abs_diff(dx_native_back) < 100,
            "round-trip drift {} vs {}",
            dx_native,
            dx_native_back
        );
    }

    #[test]
    fn quote_native_to_wsol_zero_dx_returns_zero() {
        let r = PoolReserves {
            reserve_wsol: 1_000_000,
            reserve_native: 1_000_000,
        };
        assert_eq!(quote_native_to_wsol(r, 0).unwrap(), 0);
        assert_eq!(quote_wsol_to_native(r, 0).unwrap(), 0);
    }

    #[test]
    fn quote_with_empty_reserve_rejects() {
        let r = PoolReserves {
            reserve_wsol: 0,
            reserve_native: 1_000_000,
        };
        assert!(matches!(
            quote_native_to_wsol(r, 100),
            Err(BridgeError::AmmEmptyReserves)
        ));
        assert!(matches!(
            quote_wsol_to_native(r, 100),
            Err(BridgeError::AmmEmptyReserves)
        ));
    }

    #[test]
    fn quote_native_to_wsol_handles_max_inputs_without_overflow() {
        // Adversarial: max reserves and max dx. Should not panic; should return either
        // a u64 result or AmmQuoteOverflow. With u128 intermediate the multiplication
        // u64 * u64 fits (≤ u128::MAX), but the result might overflow u64.
        let r = PoolReserves {
            reserve_wsol: u64::MAX,
            reserve_native: u64::MAX,
        };
        // Equal reserves, large dx → dy = MAX * MAX / (MAX + MAX) ≈ MAX/2, fits in u64.
        let dy = quote_native_to_wsol(r, u64::MAX).unwrap();
        assert!(dy <= u64::MAX);
    }

    #[test]
    fn round_trip_loses_money_to_slippage_only() {
        // Sanity: the doc claims a round-trip closes at AMM slippage + 2× bridge fees.
        // Here we test the AMM-only leg (no bridge fee). Two opposite quotes against
        // the SAME reserves snapshot should produce slightly less than `dx` back. In
        // production the second quote would be against post-swap reserves — but the
        // bridge issues both legs in a single tx so reserves are read once.
        let r = PoolReserves {
            reserve_wsol: 1_000_000_000_000,
            reserve_native: 1_000_000_000_000,
        };
        let dx = 10_000_000_000;
        let mid = quote_native_to_wsol(r, dx).unwrap();
        let back = quote_wsol_to_native(r, mid).unwrap();
        assert!(back <= dx, "round-trip should not gain: {} > {}", back, dx);
    }
}

//! Pure bonding-curve math for secret-pump.
//!
//! All functions in this module are deterministic, side-effect-free, and operate solely on
//! integers — no floats, no Solana types, no allocator pressure. That makes the curve
//! exhaustively unit-testable on the host without spinning up a validator.
//!
//! # Curve model
//!
//! Pump.fun-style virtual constant-product. The bonding curve maintains the invariant
//!
//! ```text
//!     (VIRTUAL_SOL + real_sol_reserves) * real_token_reserves = K
//! ```
//!
//! where `K = VIRTUAL_SOL * VIRTUAL_TOKENS` is fixed at curve creation. Reserves track:
//!
//! * `real_sol_reserves` — actual lamports that have entered the curve. Starts at `0`,
//!   grows on buys, shrinks on sells, and graduates the curve once it crosses
//!   [`GRADUATION_THRESHOLD_SOL`].
//! * `real_token_reserves` — token smallest-units currently held by the curve PDA. Starts
//!   at [`VIRTUAL_TOKENS`] (the full supply minted to the curve at creation; we use the
//!   "fully virtual" allocation model where the entire mint supply is the curve's initial
//!   token-side liquidity). Shrinks on buys, grows on sells.
//!
//! The "virtual" reserves are constants that pad the AMM pool — they make the curve start
//! with a finite price even though no real SOL has been deposited yet. They never move.
//!
//! # Algebra
//!
//! ## Buy: `dx` SOL in (after fee), `dy` tokens out
//!
//! Pre:  `(V_SOL + S) * T = K`
//! Post: `(V_SOL + S + dx) * (T - dy) = K`
//!
//! Solve for `dy`:
//!
//! ```text
//!     T - dy = K / (V_SOL + S + dx)
//!     dy     = T - K / (V_SOL + S + dx)
//! ```
//!
//! ## Sell: `dy` tokens in, `dx` SOL out (before fee)
//!
//! Pre:  `(V_SOL + S) * T = K`
//! Post: `(V_SOL + S - dx) * (T + dy) = K`
//!
//! Solve for `dx`:
//!
//! ```text
//!     V_SOL + S - dx = K / (T + dy)
//!     dx             = (V_SOL + S) - K / (T + dy)
//! ```
//!
//! Both formulas use only integer arithmetic. Intermediate products use `u128`; `K` itself
//! is `u128` because `VIRTUAL_SOL * VIRTUAL_TOKENS ≈ 3.22 × 10^28` overflows `u64`.
//!
//! # Fees
//!
//! [`FEE_BPS`] (1%) is taken on the **input** of a buy and the **output** of a sell. The
//! fee always denominates in lamports — buyers pay the fee in SOL out of their input, and
//! sellers receive their proceeds in SOL net of the fee. Token-side fees are intentionally
//! avoided so curve token-reserve accounting stays exact.
//!
//! # Graduation
//!
//! When `real_sol_reserves >= GRADUATION_THRESHOLD_SOL`, the curve is eligible to graduate
//! to a full Raydium pool. The on-chain program emits an event and flips a flag; the actual
//! pool migration is out of scope for this crate.
//!
//! # Confidentiality note
//!
//! Token amounts on the wire are confidential because the underlying mint is Token-22 with
//! the Confidential Transfer Extension active. SOL movements are NOT confidential — the
//! Solana protocol exposes lamport balances at the account level. This module does not and
//! cannot hide SOL flows; it only computes the deterministic exchange rate.

/// Virtual SOL reserves seeded into the curve at creation, in lamports.
pub const VIRTUAL_SOL: u64 = 30_000_000_000;

/// Virtual token reserves seeded into the curve at creation, in smallest token-units
/// (token has 9 decimals → 1.073e9 whole tokens × 1e9 = 1.073e18 smallest units).
pub const VIRTUAL_TOKENS: u64 = 1_073_000_000_000_000_000;

/// `K = VIRTUAL_SOL * VIRTUAL_TOKENS`, the constant-product invariant for the curve.
///
/// `30e9 * 1.073e18 ≈ 3.22e28`, which overflows `u64` (max ≈ 1.84e19) but fits comfortably
/// in `u128` (max ≈ 3.4e38). All buy/sell math therefore lifts to `u128` early.
pub const K: u128 = (VIRTUAL_SOL as u128) * (VIRTUAL_TOKENS as u128);

/// SOL threshold (lamports) at which the curve becomes eligible to graduate to Raydium.
pub const GRADUATION_THRESHOLD_SOL: u64 = 85_000_000_000;

/// Fee in basis points charged on every buy (input side, SOL) and sell (output side, SOL).
/// 100 bps = 1%.
pub const FEE_BPS: u16 = 100;

/// Basis-point denominator. `bps / BPS_DENOM` is the dimensionless rate.
pub const BPS_DENOM: u64 = 10_000;

/// Errors that the pure curve math can return.
///
/// These are independent of Solana's `ProgramError` so the curve module can be tested in a
/// pure-Rust context. The Anchor instruction handlers convert these to program errors at
/// the boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveError {
    /// Caller supplied an input amount of zero.
    ZeroInput,
    /// Output would be zero — input was below the smallest representable swap.
    ZeroOutput,
    /// Sell would extract more SOL than the curve currently holds, or buy would consume
    /// more tokens than the curve currently holds.
    InsufficientReserves,
    /// Slippage check failed (computed output below the caller's `min_out`).
    SlippageExceeded,
    /// Integer arithmetic overflow. Should be unreachable for any reasonable input given
    /// `u128` headroom, but we surface it explicitly rather than panicking.
    Overflow,
    /// Curve has already graduated; no further trades permitted on the bonding curve.
    Graduated,
}

/// Snapshot of the curve's mutable reserves. Pure value type; convenient to pass into and
/// out of the math functions without coupling them to the on-chain account layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reserves {
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,
}

impl Reserves {
    /// Initial reserves at curve creation: zero real SOL, full virtual token allocation.
    pub const fn initial() -> Self {
        Self {
            real_sol_reserves: 0,
            real_token_reserves: VIRTUAL_TOKENS,
        }
    }
}

/// Result of a successful buy quote.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuyQuote {
    /// Tokens to be transferred to the buyer (smallest units).
    pub tokens_out: u64,
    /// Lamports of SOL deducted from the buyer's input as protocol fee.
    pub sol_fee: u64,
    /// Lamports of SOL that actually enter the curve reserves (input − fee).
    pub sol_into_curve: u64,
    /// Reserves AFTER applying the swap. Caller persists these.
    pub new_reserves: Reserves,
    /// Whether `new_reserves.real_sol_reserves >= GRADUATION_THRESHOLD_SOL`.
    pub graduates: bool,
}

/// Result of a successful sell quote.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SellQuote {
    /// Lamports the curve removes from its reserves before fee.
    pub sol_out_gross: u64,
    /// Lamports of SOL retained as protocol fee.
    pub sol_fee: u64,
    /// Lamports actually paid to the seller (gross − fee).
    pub sol_to_seller: u64,
    /// Reserves AFTER applying the swap. Caller persists these.
    pub new_reserves: Reserves,
}

/// Compute the fee component of a SOL amount at [`FEE_BPS`].
///
/// Uses `u128` intermediate so multiplying a near-`u64::MAX` amount by `FEE_BPS` cannot
/// overflow. The result is a strict subset of `amount` so it always fits back in `u64`.
#[inline]
pub fn fee_on(amount: u64) -> u64 {
    let prod = (amount as u128) * (FEE_BPS as u128);
    (prod / (BPS_DENOM as u128)) as u64
}

/// Quote a buy of `sol_in` lamports against `reserves`.
///
/// Returns the tokens the curve will pay out, fee components, and the post-trade reserves.
/// Pure — does not mutate any inputs.
///
/// # Math
///
/// Let `dx_net = sol_in - fee`. The new effective SOL pool is `V + S + dx_net`. The new
/// token reserves are `T_new = K / (V + S + dx_net)` (integer division, which floors).
/// Tokens out are `T - T_new`. Floor division means `T_new` is slightly smaller than the
/// real-valued answer, so the buyer receives at most one token-ulp more than the
/// fractional answer would dictate — dust-level, in the buyer's favor, intentionally
/// matching pump.fun's rounding behavior.
pub fn quote_buy(
    reserves: Reserves,
    sol_in: u64,
    min_tokens_out: u64,
    graduated: bool,
) -> Result<BuyQuote, CurveError> {
    if graduated {
        return Err(CurveError::Graduated);
    }
    if sol_in == 0 {
        return Err(CurveError::ZeroInput);
    }

    let sol_fee = fee_on(sol_in);
    // sol_fee < sol_in since FEE_BPS < BPS_DENOM, so this never underflows.
    let sol_into_curve = sol_in - sol_fee;
    if sol_into_curve == 0 {
        // Caller paid only enough for the fee, nothing reaches the curve.
        return Err(CurveError::ZeroInput);
    }

    let v_plus_s = (VIRTUAL_SOL as u128)
        .checked_add(reserves.real_sol_reserves as u128)
        .ok_or(CurveError::Overflow)?;
    let new_eff_sol = v_plus_s
        .checked_add(sol_into_curve as u128)
        .ok_or(CurveError::Overflow)?;
    if new_eff_sol == 0 {
        return Err(CurveError::Overflow);
    }

    // Integer division — truncates downward. Since K = V*T_initial and reserves only grow
    // monotonically on a buy, T_new < T always, so the subtraction below is well-defined.
    let new_token_reserves_u128 = K / new_eff_sol;
    if new_token_reserves_u128 > reserves.real_token_reserves as u128 {
        // Curve invariant broken — should be unreachable given monotonic reserves, but we
        // surface it rather than underflow.
        return Err(CurveError::InsufficientReserves);
    }
    let tokens_out_u128 = (reserves.real_token_reserves as u128) - new_token_reserves_u128;
    if tokens_out_u128 == 0 {
        return Err(CurveError::ZeroOutput);
    }
    if tokens_out_u128 > u64::MAX as u128 {
        return Err(CurveError::Overflow);
    }
    let tokens_out = tokens_out_u128 as u64;

    if tokens_out < min_tokens_out {
        return Err(CurveError::SlippageExceeded);
    }

    let new_real_sol = reserves
        .real_sol_reserves
        .checked_add(sol_into_curve)
        .ok_or(CurveError::Overflow)?;
    let new_real_tokens = reserves
        .real_token_reserves
        .checked_sub(tokens_out)
        .ok_or(CurveError::InsufficientReserves)?;

    let new_reserves = Reserves {
        real_sol_reserves: new_real_sol,
        real_token_reserves: new_real_tokens,
    };
    let graduates = new_reserves.real_sol_reserves >= GRADUATION_THRESHOLD_SOL;

    Ok(BuyQuote {
        tokens_out,
        sol_fee,
        sol_into_curve,
        new_reserves,
        graduates,
    })
}

/// Quote a sell of `tokens_in` smallest-units against `reserves`.
///
/// Returns gross SOL out, fee components, and post-trade reserves. Pure.
///
/// # Math
///
/// New token reserves are `T_new = T + dy`. New effective SOL pool is
/// `eff_new = K / T_new` (integer division, floors). Gross SOL out is `(V + S) - eff_new`.
/// Fee is taken from the gross output. Floor division here means `eff_new` is slightly
/// smaller than the real-valued answer, so `gross` (and the seller's payout) is at most
/// one lamport more than fractional math gives — dust-level, in the seller's favor.
pub fn quote_sell(
    reserves: Reserves,
    tokens_in: u64,
    min_sol_out: u64,
    graduated: bool,
) -> Result<SellQuote, CurveError> {
    if graduated {
        return Err(CurveError::Graduated);
    }
    if tokens_in == 0 {
        return Err(CurveError::ZeroInput);
    }

    let new_token_reserves_u128 = (reserves.real_token_reserves as u128)
        .checked_add(tokens_in as u128)
        .ok_or(CurveError::Overflow)?;
    if new_token_reserves_u128 > VIRTUAL_TOKENS as u128 {
        // The curve only ever held VIRTUAL_TOKENS — selling MORE than what was bought
        // would mean tokens minted outside the curve are being dumped in. Reject so the
        // curve invariant stays intact.
        return Err(CurveError::InsufficientReserves);
    }
    if new_token_reserves_u128 == 0 {
        return Err(CurveError::Overflow);
    }
    let new_eff_sol_u128 = K / new_token_reserves_u128;

    let v_plus_s = (VIRTUAL_SOL as u128) + (reserves.real_sol_reserves as u128);
    if new_eff_sol_u128 > v_plus_s {
        // Curve invariant broken — selling tokens cannot increase the effective SOL pool.
        return Err(CurveError::Overflow);
    }
    let sol_out_gross_u128 = v_plus_s - new_eff_sol_u128;
    if sol_out_gross_u128 == 0 {
        return Err(CurveError::ZeroOutput);
    }
    if sol_out_gross_u128 > reserves.real_sol_reserves as u128 {
        // Math says we'd extract more real SOL than the curve actually holds. This can
        // happen near the edge if dust accumulation puts the curve slightly off-invariant;
        // we never let on-chain SOL go negative, so we cap at what's actually there. But
        // surfacing as an error keeps the caller honest.
        return Err(CurveError::InsufficientReserves);
    }
    let sol_out_gross = sol_out_gross_u128 as u64;

    let sol_fee = fee_on(sol_out_gross);
    let sol_to_seller = sol_out_gross - sol_fee;

    if sol_to_seller < min_sol_out {
        return Err(CurveError::SlippageExceeded);
    }

    let new_real_sol = reserves
        .real_sol_reserves
        .checked_sub(sol_out_gross)
        .ok_or(CurveError::InsufficientReserves)?;
    let new_real_tokens = reserves
        .real_token_reserves
        .checked_add(tokens_in)
        .ok_or(CurveError::Overflow)?;

    Ok(SellQuote {
        sol_out_gross,
        sol_fee,
        sol_to_seller,
        new_reserves: Reserves {
            real_sol_reserves: new_real_sol,
            real_token_reserves: new_real_tokens,
        },
    })
}

/// Whether the curve is eligible to graduate based on its current SOL reserves.
#[inline]
pub fn is_graduated(reserves: &Reserves) -> bool {
    reserves.real_sol_reserves >= GRADUATION_THRESHOLD_SOL
}

/// Spot price expressed as Q64.64 fixed-point: `(V_SOL + S) / T`, lamports per token.
///
/// Useful for off-chain quoting / display; not consumed by the swap path itself. Returns
/// `0` if `T = 0` (which means the curve is empty of tokens — graduated state).
pub fn spot_price_q64(reserves: &Reserves) -> u128 {
    if reserves.real_token_reserves == 0 {
        return 0;
    }
    let num = ((VIRTUAL_SOL as u128) + (reserves.real_sol_reserves as u128)) << 64;
    num / (reserves.real_token_reserves as u128)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `K` should equal exactly `30e9 * 1.073e18`, computed at compile time.
    #[test]
    fn k_is_constant_product_of_virtual_reserves() {
        assert_eq!(K, (VIRTUAL_SOL as u128) * (VIRTUAL_TOKENS as u128));
        // 30e9 * 1.073e18 = 3.219e28
        assert_eq!(K, 32_190_000_000_000_000_000_000_000_000u128);
    }

    /// At rest, the invariant `(V+S)*T = K` holds.
    #[test]
    fn initial_reserves_satisfy_invariant() {
        let r = Reserves::initial();
        let lhs = (VIRTUAL_SOL as u128 + r.real_sol_reserves as u128)
            * r.real_token_reserves as u128;
        assert_eq!(lhs, K);
    }

    #[test]
    fn fee_one_percent() {
        assert_eq!(fee_on(0), 0);
        assert_eq!(fee_on(10_000), 100);
        assert_eq!(fee_on(1_000_000_000), 10_000_000); // 1% of 1 SOL = 0.01 SOL
        // Truncates downward: 99 * 0.01 = 0 in integer math.
        assert_eq!(fee_on(99), 0);
    }

    /// A trivially small buy returns nonzero tokens and updates reserves consistently.
    #[test]
    fn simple_buy_updates_reserves() {
        let r = Reserves::initial();
        // 1 SOL in.
        let q = quote_buy(r, 1_000_000_000, 0, false).expect("buy ok");

        // 1% fee on 1 SOL = 0.01 SOL.
        assert_eq!(q.sol_fee, 10_000_000);
        assert_eq!(q.sol_into_curve, 990_000_000);

        // Reserves grew on SOL side, shrank on token side.
        assert_eq!(q.new_reserves.real_sol_reserves, 990_000_000);
        assert!(q.new_reserves.real_token_reserves < VIRTUAL_TOKENS);
        assert_eq!(
            q.new_reserves.real_token_reserves + q.tokens_out,
            VIRTUAL_TOKENS
        );

        // Post-trade the curve invariant still (approximately) holds. Integer truncation
        // means LHS = (V+S)*T can drift below K by at most one effective-SOL ulp per swap
        // (we floor the K/(V+S+dx) division, which makes new T very slightly smaller and
        // tokens-out very slightly larger than the real-valued answer — the buyer is the
        // beneficiary of the dust). Strictly: K - LHS < new_eff_sol.
        let lhs = (VIRTUAL_SOL as u128 + q.new_reserves.real_sol_reserves as u128)
            * q.new_reserves.real_token_reserves as u128;
        assert!(lhs <= K, "post-buy LHS cannot exceed K under floor division");
        let v_plus_s_new = VIRTUAL_SOL as u128 + q.new_reserves.real_sol_reserves as u128;
        assert!(K - lhs < v_plus_s_new, "drift within 1 effective-SOL ulp");

        assert!(!q.graduates);
    }

    /// Sell after a buy returns approximately the SOL just put in (less two fees).
    #[test]
    fn round_trip_buy_then_sell() {
        let r0 = Reserves::initial();
        let buy = quote_buy(r0, 1_000_000_000, 0, false).expect("buy ok");

        let sell = quote_sell(buy.new_reserves, buy.tokens_out, 0, false).expect("sell ok");

        // The seller cannot get back more than they put in (no money printer).
        assert!(sell.sol_to_seller < 1_000_000_000);
        // Round-trip cost is roughly two 1% fees plus integer-truncation dust. We give a
        // generous bound: at most 3% loss for the round trip.
        let loss = 1_000_000_000 - sell.sol_to_seller;
        assert!(loss < 30_000_000, "round-trip loss too high: {loss}");
        // Reserves return to (approximately) initial after a full round trip.
        assert!(sell.new_reserves.real_sol_reserves <= r0.real_sol_reserves + 100);
    }

    /// A direct sell against a curve that has tokens dispersed: returns positive SOL,
    /// applies the fee, and updates reserves consistently.
    #[test]
    fn simple_sell_updates_reserves() {
        // Pre-state: someone bought 5 SOL worth, then a different seller sells 1/4 of
        // those tokens.
        let r0 = Reserves::initial();
        let buy = quote_buy(r0, 5_000_000_000, 0, false).expect("buy ok");
        let pre = buy.new_reserves;

        let sell_amount = buy.tokens_out / 4;
        let sell = quote_sell(pre, sell_amount, 0, false).expect("sell ok");

        assert!(sell.sol_out_gross > 0);
        assert_eq!(sell.sol_fee, fee_on(sell.sol_out_gross));
        assert_eq!(sell.sol_to_seller + sell.sol_fee, sell.sol_out_gross);

        // Reserves: SOL down by gross, tokens up by sell_amount.
        assert_eq!(
            sell.new_reserves.real_sol_reserves + sell.sol_out_gross,
            pre.real_sol_reserves
        );
        assert_eq!(
            sell.new_reserves.real_token_reserves,
            pre.real_token_reserves + sell_amount
        );
    }

    /// Buy of 0 lamports is rejected with ZeroInput.
    #[test]
    fn buy_zero_in_rejected() {
        let r = Reserves::initial();
        assert_eq!(quote_buy(r, 0, 0, false), Err(CurveError::ZeroInput));
    }

    /// Sell of 0 tokens is rejected with ZeroInput.
    #[test]
    fn sell_zero_in_rejected() {
        let r = Reserves::initial();
        assert_eq!(quote_sell(r, 0, 0, false), Err(CurveError::ZeroInput));
    }

    /// Slippage protection trips when buyer demands more than the curve gives.
    #[test]
    fn buy_slippage_exceeded() {
        let r = Reserves::initial();
        let q = quote_buy(r, 1_000_000_000, 0, false).expect("baseline");
        // Asking for 1 more token than the curve delivers should fail.
        assert_eq!(
            quote_buy(r, 1_000_000_000, q.tokens_out + 1, false),
            Err(CurveError::SlippageExceeded)
        );
    }

    /// Slippage protection trips when seller demands more SOL than the curve pays.
    #[test]
    fn sell_slippage_exceeded() {
        let r0 = Reserves::initial();
        let buy = quote_buy(r0, 1_000_000_000, 0, false).expect("buy ok");
        let sell = quote_sell(buy.new_reserves, buy.tokens_out, 0, false).expect("sell ok");
        assert_eq!(
            quote_sell(buy.new_reserves, buy.tokens_out, sell.sol_to_seller + 1, false),
            Err(CurveError::SlippageExceeded)
        );
    }

    /// Selling more tokens than the curve has ever issued is rejected (would imply tokens
    /// minted outside the curve, which the program structurally prohibits).
    #[test]
    fn sell_exceeds_dispersed_supply() {
        let r = Reserves::initial();
        // Initial T = VIRTUAL_TOKENS, so dispersing 0 means the curve cannot accept any
        // sells (T + dy would exceed VIRTUAL_TOKENS).
        assert_eq!(
            quote_sell(r, 1, 0, false),
            Err(CurveError::InsufficientReserves)
        );
    }

    /// A graduated curve refuses both buys and sells.
    #[test]
    fn graduated_curve_refuses_trades() {
        let r = Reserves::initial();
        assert_eq!(
            quote_buy(r, 1_000_000_000, 0, true),
            Err(CurveError::Graduated)
        );
        assert_eq!(quote_sell(r, 1, 0, true), Err(CurveError::Graduated));
    }

    /// A buy large enough to push reserves past the graduation threshold flags it.
    #[test]
    fn large_buy_triggers_graduation_flag() {
        let r = Reserves::initial();
        // 100 SOL in (gross). After 1% fee, 99 SOL enter the curve, well past the 85 SOL
        // graduation threshold.
        let q = quote_buy(r, 100_000_000_000, 0, false).expect("buy ok");
        assert!(q.new_reserves.real_sol_reserves >= GRADUATION_THRESHOLD_SOL);
        assert!(q.graduates);
        assert!(is_graduated(&q.new_reserves));
    }

    /// A sequence of small buys monotonically grows real_sol_reserves and shrinks
    /// real_token_reserves (no jitter from rounding flipping the direction).
    #[test]
    fn many_small_buys_are_monotonic() {
        let mut r = Reserves::initial();
        let mut last_sol = r.real_sol_reserves;
        let mut last_tokens = r.real_token_reserves;
        for _ in 0..50 {
            let q = quote_buy(r, 100_000_000, 0, false).expect("buy ok");
            assert!(q.new_reserves.real_sol_reserves > last_sol);
            assert!(q.new_reserves.real_token_reserves < last_tokens);
            last_sol = q.new_reserves.real_sol_reserves;
            last_tokens = q.new_reserves.real_token_reserves;
            r = q.new_reserves;
        }
    }

    /// Each successive buy of the same size yields fewer tokens — price moves with the
    /// curve, exactly as a constant-product AMM should.
    #[test]
    fn buy_price_increases_along_the_curve() {
        let mut r = Reserves::initial();
        let mut last_out: u64 = u64::MAX;
        for _ in 0..10 {
            let q = quote_buy(r, 1_000_000_000, 0, false).expect("buy ok");
            assert!(
                q.tokens_out <= last_out,
                "buy slippage non-monotonic: prev={last_out}, now={}",
                q.tokens_out
            );
            last_out = q.tokens_out;
            r = q.new_reserves;
        }
    }

    /// Spot price increases monotonically as the curve fills up.
    #[test]
    fn spot_price_grows_with_buys() {
        let r0 = Reserves::initial();
        let p0 = spot_price_q64(&r0);
        let q = quote_buy(r0, 5_000_000_000, 0, false).expect("buy ok");
        let p1 = spot_price_q64(&q.new_reserves);
        assert!(p1 > p0);
    }

    /// A buy that exactly hits the threshold also flags graduation.
    #[test]
    fn buy_exactly_at_threshold_graduates() {
        let r = Reserves::initial();
        // We need sol_into_curve == GRADUATION_THRESHOLD_SOL. That requires
        // gross = threshold / (1 - 0.01) = threshold * 10000 / 9900 (rounded up).
        let threshold = GRADUATION_THRESHOLD_SOL as u128;
        let gross = ((threshold * BPS_DENOM as u128 + (BPS_DENOM as u128 - FEE_BPS as u128 - 1))
            / (BPS_DENOM as u128 - FEE_BPS as u128)) as u64;
        let q = quote_buy(r, gross, 0, false).expect("buy ok");
        assert!(q.new_reserves.real_sol_reserves >= GRADUATION_THRESHOLD_SOL);
        assert!(q.graduates);
    }

    /// Multiple buys and sells leave the curve in a valid state (no underflow / panic).
    #[test]
    fn random_walk_stays_consistent() {
        let mut r = Reserves::initial();
        // Buy in.
        let b = quote_buy(r, 10_000_000_000, 0, false).expect("buy ok");
        r = b.new_reserves;
        // Sell half back.
        let s = quote_sell(r, b.tokens_out / 2, 0, false).expect("sell ok");
        r = s.new_reserves;
        // Buy more.
        let b2 = quote_buy(r, 5_000_000_000, 0, false).expect("buy ok");
        r = b2.new_reserves;

        // Reserves still self-consistent: V*T product is at most K (truncation-floor
        // means each swap can reduce the product by ≤ 1 ulp; we just need it not to
        // overflow upward or break sign).
        let lhs = (VIRTUAL_SOL as u128 + r.real_sol_reserves as u128)
            * r.real_token_reserves as u128;
        assert!(lhs <= K);
        // And the product hasn't drifted by more than a few ulps of the effective SOL pool
        // across these three trades.
        let v_plus_s = VIRTUAL_SOL as u128 + r.real_sol_reserves as u128;
        assert!(K - lhs < 4 * v_plus_s);
    }

    /// Token-side reserves never exceed the initial virtual allocation.
    #[test]
    fn sell_capped_at_initial_dispersed_supply() {
        let r = Reserves::initial();
        // Buy a lot.
        let b = quote_buy(r, 5_000_000_000, 0, false).expect("buy ok");
        // Try to sell back more than was issued.
        assert_eq!(
            quote_sell(b.new_reserves, b.tokens_out + 1_000, 0, false).err(),
            Some(CurveError::InsufficientReserves)
        );
    }

    /// Fee math at the tiniest input that still produces a non-zero fee.
    #[test]
    fn smallest_fee_emitting_buy() {
        // 100 lamports: 1% = 1 lamport fee, 99 enter curve. Curve might still emit zero
        // tokens at this size (1 lamport / virtual SOL is below 1 token-unit), but the
        // function should return a clean error rather than panic.
        let r = Reserves::initial();
        let result = quote_buy(r, 100, 0, false);
        // Either a tiny success or ZeroOutput — both are acceptable. Panic is not.
        match result {
            Ok(q) => {
                assert_eq!(q.sol_fee, 1);
                assert_eq!(q.sol_into_curve, 99);
                assert!(q.tokens_out > 0);
            }
            Err(CurveError::ZeroOutput) => { /* fine */ }
            Err(other) => panic!("unexpected error on tiny buy: {other:?}"),
        }
    }
}

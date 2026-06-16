//! # secret-pump
//!
//! Staccana's confidential bonding-curve launchpad. Mechanically a pump.fun-style virtual
//! constant-product curve, layered on top of Token-22 mints with the **Confidential
//! Transfer Extension (CTE)** active by default. The CTE makes per-trade *token amounts*
//! opaque to outside observers; SOL deltas remain visible at the protocol level (Solana
//! does not provide confidentiality for native lamports).
//!
//! ## Why this matters
//!
//! Anti-snipe and copy-trading defense fall out for free. The curve still has a single
//! deterministic price function known to all participants; what's hidden is **how many
//! tokens any given trade moved**. Mempool-watching bots can't size their copy trades
//! against an unobservable token quantity.
//!
//! ## Program surface
//!
//! Three instructions, mirroring pump.fun's minimal interface:
//!
//! | Ix       | Effect                                                                            |
//! |----------|-----------------------------------------------------------------------------------|
//! | `create` | Mint a new Token-22 with CTE active, init the [`BondingCurve`] PDA, fund vault.   |
//! | `buy`    | Swap SOL → token along the curve. 1% fee skimmed to the treasury PDA.             |
//! | `sell`   | Swap token → SOL along the curve. 1% fee skimmed to the treasury PDA.             |
//!
//! ## Architectural touchpoints
//!
//! * Treasury PDA collects all curve fees. Address is **TBD** per `docs/SPEC.md` §2.1; the
//!   placeholder constant [`TREASURY_PUBKEY_PLACEHOLDER`] must be replaced before mainnet
//!   activation.
//! * Graduation: when `real_sol_reserves >= 85 SOL`, the program emits a
//!   [`crate::state::GraduationEvent`] and latches `BondingCurve::graduated = true`. The
//!   actual Raydium pool migration is a downstream service consuming the event; it is **not
//!   implemented in this crate**.
//! * Curve math is isolated in [`crate::curve`] and 100% pure / unit-testable.

use anchor_lang::prelude::*;

pub mod curve;
pub mod error;
pub mod instructions;
pub mod state;

use instructions::*;

// Placeholder program ID. Replace with the real deployed address before mainnet launch;
// SPEC.md §2.1 lists `SECRET_PUMP_ID = TBD`.
declare_id!("SPump11111111111111111111111111111111111111");

/// The staccana genesis treasury PDA. All secret-pump curve fees are routed here.
///
/// This is `find_program_address(&[b"treasury"], staccana_validator_subsidy::ID)`
/// = `D3FcFs85BAzroHzwWp1CEgnjCku4bPKMFAScrtfAdo83`, the same PDA the
/// `validator-subsidy` program owns + drains for `bootstrap_distribute` /
/// `distribute_yield`. Per README and `docs/SPEC.md` §2.1: there's ONE
/// genesis treasury (485M SOL pre-credited), funding ops + bonding-curve
/// seed liquidity + validator subsidies. Secret-pump fees are an
/// accretion source for it.
///
/// (Const name kept as `_PLACEHOLDER` for the moment so call-sites don't
/// move; once that's not needed we can rename. The actual address is now
/// the real PDA, not the ASCII string `"staccana_treasury_placeholder___"`
/// it used to be — the rename of the binding stays a follow-up.)
pub const TREASURY_PUBKEY_PLACEHOLDER: Pubkey =
    Pubkey::new_from_array([
        0xb2, 0xdf, 0xec, 0x01, 0xc6, 0xe1, 0x71, 0xbc,
        0x18, 0x53, 0x48, 0x6a, 0xe0, 0xc0, 0x10, 0x6a,
        0x11, 0xe6, 0x02, 0x2c, 0xc0, 0x89, 0x4d, 0xdb,
        0xdb, 0x39, 0x93, 0xf2, 0x3c, 0xf4, 0xb9, 0x40,
    ]);

#[program]
pub mod staccana_secret_pump {
    use super::*;

    /// Create a new bonding curve.
    ///
    /// Initializes a Token-22 mint with the Confidential Transfer extension active by
    /// default, mints the curve's full virtual token allocation into the curve vault, and
    /// initializes the [`crate::state::BondingCurve`] PDA. The transaction's payer covers
    /// rent for the mint, vault, and curve PDA accounts.
    pub fn create(ctx: Context<CreateCurve>, args: CreateArgs) -> Result<()> {
        instructions::create::handler(ctx, args)
    }

    /// Swap SOL for the curve's token along the constant-product curve.
    ///
    /// `sol_in` is the gross amount the buyer commits in lamports; the protocol fee is
    /// taken from this input and the remainder enters the curve. `min_tokens_out` is the
    /// caller-supplied slippage floor — the instruction reverts if the curve would deliver
    /// fewer tokens. If this trade pushes `real_sol_reserves` past the graduation
    /// threshold, the curve is latched closed and a [`crate::state::GraduationEvent`] is
    /// emitted.
    pub fn buy(ctx: Context<Buy>, sol_in: u64, min_tokens_out: u64) -> Result<()> {
        instructions::buy::handler(ctx, sol_in, min_tokens_out)
    }

    /// Swap the curve's token for SOL along the constant-product curve.
    ///
    /// `tokens_in` is the amount of token (smallest units) being sold back. `min_sol_out`
    /// is the slippage floor — the instruction reverts if proceeds (post-fee) fall below
    /// it. The protocol fee is taken from the curve's gross output, so the seller receives
    /// `gross - fee` lamports.
    pub fn sell(ctx: Context<Sell>, tokens_in: u64, min_sol_out: u64) -> Result<()> {
        instructions::sell::handler(ctx, tokens_in, min_sol_out)
    }
}

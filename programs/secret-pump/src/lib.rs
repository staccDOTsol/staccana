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
/// `find_program_address(&[b"treasury"], LAZY_CLAIM_PROGRAM_ID)` =
/// `P75Pswx4w4FKW6orSw2GQVLqrBchNTQCVEagUpUwUZq` (bump 254). The treasury is derived
/// under — and owned by — lazy-claim at genesis (the gas-exempt claim path direct-debits
/// it), and handed to the Squads governance multisig post-launch. The removed
/// validator-subsidy program no longer consumes it (no bridge, no yield — see
/// `docs/AUDIT_SCOPE.md`). Per README and `docs/SPEC.md`: ONE genesis treasury (485M SOL
/// pre-credited); secret-pump fees are an accretion source for it.
///
/// Both program IDs are placeholders today; when real keypairs are generated this must be
/// recomputed against the real lazy-claim program ID. The
/// `treasury_pubkey_matches_lazy_claim_derivation` test below guards against silent drift.
///
/// (Const name kept as `_PLACEHOLDER` so call-sites don't move; rename is a follow-up.)
pub const TREASURY_PUBKEY_PLACEHOLDER: Pubkey =
    Pubkey::new_from_array([
        0x05, 0xa9, 0xa5, 0xcf, 0x0e, 0xc4, 0x9b, 0xa0,
        0x24, 0x6b, 0x11, 0x71, 0x66, 0x01, 0x59, 0x69,
        0x8a, 0x85, 0xe0, 0xf4, 0x24, 0x57, 0xac, 0xa0,
        0xbf, 0x72, 0x78, 0xc6, 0xb2, 0xe3, 0xd4, 0xfc,
    ]);

#[cfg(test)]
mod treasury_addr_tests {
    use super::*;

    /// lazy-claim's program ID (placeholder). MUST stay in sync with
    /// `staccana_lazy_claim::id()` and `genesis-bake`'s `LAZY_CLAIM_PROGRAM_ID`
    /// (ASCII `"LAZY_CLAIM_PROGRAM_PLACEHOLDER11"`).
    const LAZY_CLAIM_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
        b'L', b'A', b'Z', b'Y', b'_', b'C', b'L', b'A', b'I', b'M', b'_', b'P', b'R', b'O', b'G',
        b'R', b'A', b'M', b'_', b'P', b'L', b'A', b'C', b'E', b'H', b'O', b'L', b'D', b'E', b'R',
        b'1', b'1',
    ]);

    /// The fee-destination constant must equal the genesis treasury PDA derived under
    /// lazy-claim. If the lazy-claim program ID changes (real keypair finalization),
    /// `TREASURY_PUBKEY_PLACEHOLDER` must be recomputed and this test will fail until it is.
    #[test]
    fn treasury_pubkey_matches_lazy_claim_derivation() {
        let (derived, _bump) = Pubkey::find_program_address(&[b"treasury"], &LAZY_CLAIM_PROGRAM_ID);
        assert_eq!(
            derived, TREASURY_PUBKEY_PLACEHOLDER,
            "secret-pump fee treasury must equal find_program_address([treasury], lazy-claim)"
        );
    }
}

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

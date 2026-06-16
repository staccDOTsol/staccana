//! Persistent on-chain state for the secret-pump program.
//!
//! There is exactly one [`BondingCurve`] PDA per token mint, derived as
//! `find_program_address(&[b"bonding_curve", mint.key().as_ref()], program_id)`. The PDA
//! holds the curve's mutable reserve counters, a graduation flag, and bookkeeping fields
//! consumed by off-chain indexers.
//!
//! All scalar fields are sized for `u64` to match Solana's native lamport / token-amount
//! types. The reserves never exceed `u64` in practice because both `VIRTUAL_SOL` and
//! `VIRTUAL_TOKENS` themselves fit in `u64`; only the constant-product `K` requires `u128`.

use anchor_lang::prelude::*;

/// Per-mint bonding curve state.
///
/// The PDA is also the **vault authority** for the curve's Token-22 vault and the **escrow
/// holder** for the lamports that have entered the curve. SOL movements bypass the
/// `BondingCurve` data field — they go to/from the PDA's lamport balance directly via
/// `try_borrow_mut_lamports` (see `instructions::buy` / `instructions::sell`).
#[account]
#[derive(Default)]
pub struct BondingCurve {
    /// Token mint this curve trades.
    pub mint: Pubkey,
    /// Whoever invoked `create`. Recorded for off-chain attribution; the protocol does not
    /// grant the creator any privileged authority.
    pub creator: Pubkey,
    /// Real lamports held by the curve PDA. Mirrors the PDA's lamport balance modulo
    /// rent-exempt minimum (we don't include rent in the reserve accounting).
    pub real_sol_reserves: u64,
    /// Real token-units held by the curve vault.
    pub real_token_reserves: u64,
    /// Total tokens ever bought from the curve (does not subtract sells). Useful for
    /// indexers; not consumed by curve math.
    pub total_tokens_dispensed: u64,
    /// Total lamports ever paid as protocol fee since curve creation.
    pub total_fees_collected: u64,
    /// Set true once `real_sol_reserves` first crosses the graduation threshold. Latches —
    /// once true, the curve refuses further buys / sells until external migration logic
    /// (out of scope for this program) settles the curve.
    pub graduated: bool,
    /// Slot at which graduation was first triggered, or `0` if not graduated.
    pub graduation_slot: u64,
    /// PDA bump for the bonding-curve account itself.
    pub bump: u8,
    /// PDA bump for the curve's token vault (an associated PDA token account).
    pub vault_bump: u8,
}

impl BondingCurve {
    /// Discriminator (8) + `Pubkey * 2` (64) + `u64 * 5` (40) + `bool` (1) + `u8 * 2` (2)
    /// = 115. Padded to 192 for forward-compat headroom (room for ~9 more u64 fields
    /// without a migration).
    pub const SPACE: usize = 192;

    /// Seed prefix for the bonding-curve PDA.
    pub const SEED: &'static [u8] = b"bonding_curve";

    /// Seed prefix for the per-curve token vault PDA.
    pub const VAULT_SEED: &'static [u8] = b"bonding_curve_vault";

    /// Seeds for signing on the curve PDA's behalf. Used by CPI calls that move tokens
    /// out of the vault during a buy.
    pub fn signer_seeds<'a>(mint: &'a Pubkey, bump: &'a [u8; 1]) -> [&'a [u8]; 3] {
        [Self::SEED, mint.as_ref(), bump.as_ref()]
    }

    /// Snapshot the mutable reserves into the pure [`crate::curve::Reserves`] type.
    pub fn reserves(&self) -> crate::curve::Reserves {
        crate::curve::Reserves {
            real_sol_reserves: self.real_sol_reserves,
            real_token_reserves: self.real_token_reserves,
        }
    }

    /// Apply a freshly computed reserves snapshot back to the on-chain account.
    pub fn apply_reserves(&mut self, r: crate::curve::Reserves) {
        self.real_sol_reserves = r.real_sol_reserves;
        self.real_token_reserves = r.real_token_reserves;
    }
}

/// Treasury PDA seed prefix. The treasury is a fixed protocol PDA that collects all curve
/// fees. The actual treasury PDA address is **TBD per `docs/SPEC.md` §2.1**; for v0 we use
/// a deterministic placeholder (see [`crate::TREASURY_PUBKEY_PLACEHOLDER`]).
pub const TREASURY_SEED: &[u8] = b"staccana_treasury";

/// Emitted on every buy.
#[event]
pub struct BuyEvent {
    pub mint: Pubkey,
    pub buyer: Pubkey,
    pub sol_in: u64,
    pub sol_fee: u64,
    pub tokens_out: u64,
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub graduated: bool,
}

/// Emitted on every sell.
#[event]
pub struct SellEvent {
    pub mint: Pubkey,
    pub seller: Pubkey,
    pub tokens_in: u64,
    pub sol_out_gross: u64,
    pub sol_fee: u64,
    pub sol_to_seller: u64,
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,
}

/// Emitted exactly once per curve, the first time `real_sol_reserves` crosses the threshold.
/// External services (Raydium pool migrator) listen for this and execute the actual pool
/// creation; that pipeline is out of scope for this program.
#[event]
pub struct GraduationEvent {
    pub mint: Pubkey,
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub slot: u64,
}

/// Emitted on `create`.
#[event]
pub struct CurveCreatedEvent {
    pub mint: Pubkey,
    pub creator: Pubkey,
    pub virtual_sol: u64,
    pub virtual_tokens: u64,
}

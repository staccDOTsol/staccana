//! `convert_native_to_wsol` — swap native staccana SOL → wSOL via the on-chain AMM.
//!
//! User flow per `docs/BRIDGE.md` §"Native SOL ↔ mainnet SOL via the bridge":
//!
//! 1. User wants to exit staccana's native SOL back to mainnet SOL.
//! 2. They call this ix with `dx_native` lamports → bridge quotes wSOL out from the
//!    secret-ray pool, then CPIs the swap (lamports go into the pool, wSOL comes out
//!    to the user's wSOL ATA).
//! 3. The user then calls the standard [`crate::instructions::burn`] on wSOL to
//!    receive mainnet SOL on the mainnet vault side.
//!
//! On-chain effects:
//! 1. Validate the supplied wSOL `AssetConfig` is R-locked (sanity — only the wSOL
//!    asset participates in this flow).
//! 2. Read pool reserves (TODO: from secret-ray pool account; currently passed in).
//! 3. Quote `dy_wsol` via [`crate::amm_oracle::quote_native_to_wsol`].
//! 4. Apply the wSOL `mint_fee_bps` to `dy_wsol` (the conversion is logically a mint
//!    of wSOL to the user, so the same fee class applies).
//! 5. Enforce `min_out_wsol` slippage protection.
//! 6. TODO(secret-ray): CPI into secret-ray's `swap` ix to atomically transfer
//!    `dx_native` lamports → pool and receive `dy_wsol` → user wSOL ATA. Until the
//!    AMM crate exists this ix only emits the quote event; integrators must run the
//!    swap manually.
//! 7. Emit [`ConvertNativeToWsolEvent`].
//!
//! **Not a peg.** This ix always quotes at the current AMM rate. Round-trip
//! (`convert_native_to_wsol` → `burn` on wSOL) closes at AMM slippage + 2× bridge
//! fees. There is no fixed rate to defend.

use crate::amm_oracle::{quote_native_to_wsol, PoolReserves};
use crate::attestation::apply_bps_fee;
use crate::error::BridgeError;
use crate::state::AssetConfig;
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ConvertNativeToWsolArgs {
    /// Asset id of the wSOL bridge asset (R-locked at 1.0).
    pub asset_id: u32,
    /// Native staccana SOL the user is selling, in lamports.
    pub dx_native: u64,
    /// Minimum acceptable wSOL output (slippage guard). The handler computes the
    /// AMM quote, deducts the bridge fee, and rejects with
    /// [`BridgeError::AmmSlippageExceeded`] if the result is below this.
    pub min_out_wsol: u64,
    /// Pool reserves snapshot the off-chain client read from the secret-ray pool. This
    /// is a temporary plumbing arg until the AMM crate lands and the handler can
    /// decode reserves directly from the pool account. See `amm_oracle.rs`.
    pub reserves: PoolReserves,
}

#[derive(Accounts)]
#[instruction(args: ConvertNativeToWsolArgs)]
pub struct ConvertNativeToWsol<'info> {
    /// User initiating the conversion. Pays for any rent + the dx_native lamports
    /// that flow into the AMM pool.
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"asset", args.asset_id.to_le_bytes().as_ref()],
        bump = asset_config.bump,
    )]
    pub asset_config: Account<'info, AssetConfig>,

    /// CHECK: secret-ray pool account. While the AMM crate is unbuilt this is unchecked
    /// and the handler reads reserves from `args.reserves` instead. When secret-ray
    /// lands this becomes a typed account and the address is enforced via PDA seeds
    /// (see [`crate::amm_oracle::PLACEHOLDER_POOL_SEED`]).
    pub amm_pool: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Handler — quote, fee-deduct, slippage-check, emit event.
pub fn handler(
    ctx: Context<ConvertNativeToWsol>,
    args: ConvertNativeToWsolArgs,
) -> Result<()> {
    require!(args.dx_native > 0, BridgeError::BadInstructionData);

    let cfg = &ctx.accounts.asset_config;
    // Sanity: only the R-locked wSOL asset participates in this conversion. Catches
    // a fat-finger where a relayer passes the stSOL asset_id.
    require!(cfg.is_r_locked(), BridgeError::AssetIdMismatch);

    let gross_out = quote_native_to_wsol(args.reserves, args.dx_native)?;
    let net_out = apply_bps_fee(gross_out, cfg.mint_fee_bps);

    require!(
        net_out >= args.min_out_wsol,
        BridgeError::AmmSlippageExceeded
    );

    // TODO(secret-ray): CPI into the AMM swap ix here. The required choreography:
    //   1. System-transfer `args.dx_native` lamports from `user` to pool's native
    //      reserve account (or wrap-then-deposit if the pool holds wSOL only on both
    //      sides — secret-ray's exact pool shape is TBD).
    //   2. CPI `secret_ray::swap { amount_in: dx_native, min_amount_out: net_out, ... }`.
    //   3. The swap ix transfers `net_out` lamports of wSOL to the user's wSOL ATA.
    //
    // Until then this ix is a quote-and-emit shim. Off-chain integrators must run the
    // swap themselves and then claim wSOL via the standard mint flow; the bridge fee
    // is informational only in this transitional state.

    emit!(ConvertNativeToWsolEvent {
        asset_id: args.asset_id,
        user: ctx.accounts.user.key(),
        dx_native: args.dx_native,
        gross_out_wsol: gross_out,
        net_out_wsol: net_out,
        reserve_wsol: args.reserves.reserve_wsol,
        reserve_native: args.reserves.reserve_native,
    });

    Ok(())
}

/// Emitted on every successful native → wSOL conversion. Indexers consume this to
/// surface the AMM-quoted price the user actually received.
#[event]
pub struct ConvertNativeToWsolEvent {
    pub asset_id: u32,
    pub user: Pubkey,
    pub dx_native: u64,
    pub gross_out_wsol: u64,
    pub net_out_wsol: u64,
    pub reserve_wsol: u64,
    pub reserve_native: u64,
}

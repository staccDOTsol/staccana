//! `convert_wsol_to_native` — swap wSOL → native staccana SOL via the on-chain AMM.
//!
//! User flow per `docs/BRIDGE.md` §"Native SOL ↔ mainnet SOL via the bridge":
//!
//! 1. User holds mainnet SOL and wants native staccana SOL.
//! 2. They deposit mainnet SOL into the wSOL mainnet vault → federation attests →
//!    they call the standard [`crate::instructions::mint`] for the wSOL asset and
//!    receive wSOL on staccana.
//! 3. They call THIS ix with `dx_wsol` lamports → bridge quotes native SOL out of the
//!    secret-ray pool, then CPIs the swap (wSOL goes into pool, native SOL comes
//!    out to the user).
//!
//! Mirror of [`crate::instructions::convert_native_to_wsol`]; same AMM math, swapped
//! reserve sides, `burn_fee_bps` instead of `mint_fee_bps` because the user is
//! exiting wSOL.

use crate::amm_oracle::{quote_wsol_to_native, PoolReserves};
use crate::attestation::apply_bps_fee;
use crate::error::BridgeError;
use crate::state::AssetConfig;
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct ConvertWsolToNativeArgs {
    /// Asset id of the wSOL bridge asset (R-locked at 1.0).
    pub asset_id: u32,
    /// wSOL the user is selling, in lamports.
    pub dx_wsol: u64,
    /// Minimum acceptable native staccana SOL output (slippage guard).
    pub min_out_native: u64,
    /// Pool reserves snapshot. See `amm_oracle.rs`. Goes away once secret-ray lands
    /// and the handler decodes reserves from the pool account directly.
    pub reserves: PoolReserves,
}

#[derive(Accounts)]
#[instruction(args: ConvertWsolToNativeArgs)]
pub struct ConvertWsolToNative<'info> {
    /// User initiating the conversion.
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        seeds = [b"asset", args.asset_id.to_le_bytes().as_ref()],
        bump = asset_config.bump,
    )]
    pub asset_config: Account<'info, AssetConfig>,

    /// CHECK: secret-ray pool account. See [`crate::amm_oracle::PLACEHOLDER_POOL_SEED`].
    pub amm_pool: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

/// Handler — quote, fee-deduct, slippage-check, emit event.
pub fn handler(
    ctx: Context<ConvertWsolToNative>,
    args: ConvertWsolToNativeArgs,
) -> Result<()> {
    require!(args.dx_wsol > 0, BridgeError::BadInstructionData);

    let cfg = &ctx.accounts.asset_config;
    require!(cfg.is_r_locked(), BridgeError::AssetIdMismatch);

    let gross_out = quote_wsol_to_native(args.reserves, args.dx_wsol)?;
    let net_out = apply_bps_fee(gross_out, cfg.burn_fee_bps);

    require!(
        net_out >= args.min_out_native,
        BridgeError::AmmSlippageExceeded
    );

    // TODO(secret-ray): CPI into the AMM swap ix. Required choreography:
    //   1. CPI `token_2022::burn` (or transfer) of `dx_wsol` from the user's wSOL
    //      ATA into the pool's wSOL reserve.
    //   2. CPI `secret_ray::swap { amount_in: dx_wsol, min_amount_out: net_out, ... }`
    //      which atomically credits `net_out` native lamports to `user`.
    //
    // Until secret-ray lands the ix is a quote-and-emit shim; off-chain integrators
    // perform the swap manually.

    emit!(ConvertWsolToNativeEvent {
        asset_id: args.asset_id,
        user: ctx.accounts.user.key(),
        dx_wsol: args.dx_wsol,
        gross_out_native: gross_out,
        net_out_native: net_out,
        reserve_wsol: args.reserves.reserve_wsol,
        reserve_native: args.reserves.reserve_native,
    });

    Ok(())
}

/// Emitted on every successful wSOL → native conversion.
#[event]
pub struct ConvertWsolToNativeEvent {
    pub asset_id: u32,
    pub user: Pubkey,
    pub dx_wsol: u64,
    pub gross_out_native: u64,
    pub net_out_native: u64,
    pub reserve_wsol: u64,
    pub reserve_native: u64,
}

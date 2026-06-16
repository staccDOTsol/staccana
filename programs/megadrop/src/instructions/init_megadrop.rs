//! `init_megadrop` — governance-gated one-shot bootstrap of the megadrop singleton.
//!
//! Initializes the [`MegadropConfig`] PDA at `["megadrop_config"]` with the snapshot
//! Merkle root, the genesis month (yyyymm — first tranche unlock), the total
//! allocation summed across all leaves (sanity check), and the treasury authority PDA.
//!
//! Idempotency: the PDA is created via Anchor's `init` constraint; a second call
//! rejects with the standard "account already in use" error.
//!
//! Authorization: any signer can call this in v1 — for production deployment the
//! governance multisig should be the upgrade authority, and the deploy-time tooling
//! should call `init_megadrop` from that key. The handler does not enforce a specific
//! authority because no other PDA exists at deploy time to validate against.

use crate::error::MegadropError;
use crate::state::{MegadropConfig, MEGADROP_CONFIG_SEED};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct InitMegadropArgs {
    /// Snapshot Merkle root over `(holder_pubkey, total_allocation_lamports)` leaves.
    /// Produced by `tools/megadrop-snapshot`.
    pub claimable_root: [u8; 32],

    /// First tranche unlock month, ISO `yyyymm` (e.g. `202605` = May 2026).
    pub genesis_month: u32,

    /// Sum of all leaf allocations. Stored as a sanity field; not used in claim math.
    pub total_allocation_lamports: u64,

    /// PDA-derived authority that signs treasury debits. The genesis builder MUST set
    /// the treasury PDA's owner / debit policy so this authority can drain it — see
    /// `state.rs` module doc.
    pub treasury_authority: Pubkey,
}

#[derive(Accounts)]
pub struct InitMegadrop<'info> {
    /// Must equal `crate::ADMIN_AUTHORITY` (staccana's BPF upgrade-authority).
    /// Originally bare `Signer` — the doc claimed "any signer can call this
    /// in v1, deployer's responsibility." But the singleton MegadropConfig
    /// PDA isn't init'd at deploy time, so anyone could front-run with their
    /// own `claimable_root` + `treasury_authority` and permanently siphon
    /// the megadrop allocation. Mirrors the patch applied to
    /// `init_subsidy` / `update_megadrop`.
    #[account(
        mut,
        constraint = authority.key() == crate::ADMIN_AUTHORITY @ MegadropError::Unauthorized,
    )]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = MegadropConfig::SPACE,
        seeds = [MEGADROP_CONFIG_SEED],
        bump,
    )]
    pub megadrop_config: Account<'info, MegadropConfig>,

    pub system_program: Program<'info, System>,
}

/// Handler — sanity-check args, populate the config.
pub fn handler(ctx: Context<InitMegadrop>, args: InitMegadropArgs) -> Result<()> {
    require!(args.genesis_month > 0, MegadropError::BadInitArgs);
    require!(
        args.total_allocation_lamports > 0,
        MegadropError::BadInitArgs
    );
    // Plausibility check on the genesis month: must look like a `yyyymm` value (year
    // in [1970, 9999], month in [1, 12]). Catches an obvious calling error like
    // passing a Unix timestamp instead of a yyyymm integer.
    let year = args.genesis_month / 100;
    let month = args.genesis_month % 100;
    require!(
        (1970..=9999).contains(&year) && (1..=12).contains(&month),
        MegadropError::BadInitArgs
    );

    let cfg = &mut ctx.accounts.megadrop_config;
    cfg.claimable_root = args.claimable_root;
    cfg.genesis_month = args.genesis_month;
    cfg.total_allocation_lamports = args.total_allocation_lamports;
    cfg.treasury_authority = args.treasury_authority;
    cfg.bump = ctx.bumps.megadrop_config;

    Ok(())
}

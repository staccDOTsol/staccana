//! `register_asset` — governance-gated one-shot registration of a new bridge asset.
//!
//! Initializes three PDAs in one transaction:
//! - [`AssetConfig`] at `["asset", asset_id_le]`
//! - [`RatioState`]  at `["ratio", asset_id_le]` — initialized with R = 1.0 (Q64.64)
//!   and `last_published_slot = 0` so the first `update_ratio` can land any time
//! - [`NonceOutCounter`] at `["nonce_out", asset_id_le]` — starts at `next_nonce = 0`
//!
//! Also initializes the [`FederationSet`] PDA on first call (i.e. if it does not yet
//! exist). Subsequent `register_asset` calls reuse the existing federation set; rotating
//! the federation is a separate ix not in scope for v1.

use crate::error::BridgeError;
use crate::state::{
    AssetConfig, FederationSet, NonceOutCounter, RatioState, MAX_FEDERATION_MEMBERS,
};
use anchor_lang::prelude::*;

/// Args for `register_asset`. All fields are static configuration; the federation set
/// (M, N, members) is supplied here so first-asset bootstrap doesn't need a separate ix.
///
/// On second-and-later calls, the federation parameters are ignored — the existing set
/// is reused. (Rationale: simplest possible v1 bootstrap. Federation rotation is a
/// separate governance ix in v2.)
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct RegisterAssetArgs {
    pub asset_id: u32,
    pub underlying_label: [u8; 32],
    pub mainnet_vault_program: Pubkey,
    pub staccana_mint: Pubkey,
    pub decimals: u8,
    pub mint_fee_bps: u16,
    pub burn_fee_bps: u16,

    /// M-of-N threshold. Ignored if `FederationSet` PDA already exists.
    pub federation_m: u8,
    /// Active federation member count. Ignored if `FederationSet` PDA already exists.
    pub federation_n: u8,
    /// Federation pubkeys. Only the first `federation_n` slots are read. Ignored if
    /// `FederationSet` PDA already exists.
    pub federation_members: [Pubkey; MAX_FEDERATION_MEMBERS],

    /// Per-asset behaviour flags. See [`crate::state::AssetFlag`]. For wSOL this MUST
    /// have `AssetFlag::R_LOCKED` set so R is pinned at 1.0 forever.
    pub flags: u8,
}

#[derive(Accounts)]
#[instruction(args: RegisterAssetArgs)]
pub struct RegisterAsset<'info> {
    /// Must equal `crate::ADMIN_AUTHORITY` (staccana's BPF upgrade-authority).
    /// Originally bare `Signer` with the comment "real deployment will gate this
    /// further" — but on a fresh deploy with the AssetConfig + FederationSet
    /// PDAs not yet initialized, anyone could front-run and bind their own
    /// pubkeys as the federation set, taking permanent control of every
    /// subsequent `update_ratio` and `mint` attestation. The constraint below
    /// closes that hole.
    #[account(
        mut,
        constraint = authority.key() == crate::ADMIN_AUTHORITY @ BridgeError::Unauthorized,
    )]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = AssetConfig::SPACE,
        seeds = [b"asset", args.asset_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub asset_config: Account<'info, AssetConfig>,

    #[account(
        init,
        payer = authority,
        space = RatioState::SPACE,
        seeds = [b"ratio", args.asset_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub ratio_state: Account<'info, RatioState>,

    #[account(
        init,
        payer = authority,
        space = NonceOutCounter::SPACE,
        seeds = [b"nonce_out", args.asset_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub nonce_out: Account<'info, NonceOutCounter>,

    /// Federation set is `init_if_needed` so the first asset registration also
    /// bootstraps the federation; subsequent registrations reuse the existing set.
    #[account(
        init_if_needed,
        payer = authority,
        space = FederationSet::SPACE,
        seeds = [b"federation"],
        bump,
    )]
    pub federation_set: Account<'info, FederationSet>,

    pub system_program: Program<'info, System>,
}

/// Handler — initialize the asset configuration and (if first call) the federation set.
pub fn handler(ctx: Context<RegisterAsset>, args: RegisterAssetArgs) -> Result<()> {
    // Cap fees at 100% just in case governance fat-fingers it. Spec defaults are 10 bps
    // each; UI should refuse anything north of low single-digit percent.
    require!(args.mint_fee_bps <= 10_000, BridgeError::BadInstructionData);
    require!(args.burn_fee_bps <= 10_000, BridgeError::BadInstructionData);

    let cfg = &mut ctx.accounts.asset_config;
    cfg.asset_id = args.asset_id;
    cfg.underlying_label = args.underlying_label;
    cfg.mainnet_vault_program = args.mainnet_vault_program;
    cfg.staccana_mint = args.staccana_mint;
    cfg.decimals = args.decimals;
    cfg.mint_fee_bps = args.mint_fee_bps;
    cfg.burn_fee_bps = args.burn_fee_bps;
    cfg.bump = ctx.bumps.asset_config;
    cfg.flags = args.flags;

    // R starts at 1.0 (Q64.64) and is bumped on every `update_ratio`. Setting
    // `last_published_slot = 0` lets the first update land at any future slot.
    let ratio = &mut ctx.accounts.ratio_state;
    ratio.asset_id = args.asset_id;
    ratio.r_q64 = 1u128 << 64;
    ratio.last_published_slot = 0;
    ratio.last_nonce = 0;
    ratio.bump = ctx.bumps.ratio_state;

    let nonce_out = &mut ctx.accounts.nonce_out;
    nonce_out.asset_id = args.asset_id;
    nonce_out.next_nonce = 0;
    nonce_out.bump = ctx.bumps.nonce_out;

    // First-call bootstrap of the federation set. We detect "already initialized" by
    // looking at `n` — `init_if_needed` populates with `Default` (n == 0) on creation.
    let fed = &mut ctx.accounts.federation_set;
    if fed.n == 0 {
        require!(
            args.federation_m > 0
                && args.federation_n > 0
                && args.federation_n as usize <= MAX_FEDERATION_MEMBERS
                && args.federation_m <= args.federation_n,
            BridgeError::BadFederationParams
        );
        fed.m = args.federation_m;
        fed.n = args.federation_n;
        fed.members = args.federation_members;
        fed.bump = ctx.bumps.federation_set;
    }

    Ok(())
}

//! `update_megadrop` — authority-gated update of fields on the singleton
//! `MegadropConfig` PDA.
//!
//! Why this exists: `init_megadrop` is one-shot (Anchor `init` constraint). On
//! a fresh cluster the deployment tooling sometimes seeds the PDA with a
//! placeholder Merkle root + provisional allocation total before the real
//! snapshot finalizes. Without an update path we'd have to either close the
//! PDA (no `close_megadrop` ix exists; would also strand any per-holder
//! claimed-tranche bits already minted against this config) or rebuild
//! genesis. Both are heavy. This ix lets the update happen in-place via
//! `solana program deploy`-style operator workflow.
//!
//! Authorization: signer MUST equal `crate::ADMIN_AUTHORITY` (a hardcoded
//! pubkey baked into the program at compile time — currently the staccana
//! BPF upgrade-authority key). Originally this ix accepted any signer and
//! the comment said "production deployments should gate this off the
//! upgrade authority", but no enforcement existed on chain — anyone could
//! call `update_megadrop` with their own `claimable_root` and replace the
//! snapshot, siphoning the entire allocation through claims that match
//! THEIR root. The constraint below now closes that hole.
//!
//! The `Option<...>` per field means callers can patch only the field(s)
//! that actually changed, e.g. update only `claimable_root` while leaving
//! `genesis_month` alone.

use crate::error::MegadropError;
use crate::state::{MegadropConfig, MEGADROP_CONFIG_SEED};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct UpdateMegadropArgs {
    /// New snapshot Merkle root, or `None` to leave unchanged.
    pub claimable_root: Option<[u8; 32]>,
    /// New genesis month (`yyyymm`), or `None` to leave unchanged.
    pub genesis_month: Option<u32>,
    /// New total allocation lamports, or `None` to leave unchanged. This is a
    /// sanity field only — claim math doesn't read it — so callers can patch
    /// it after a re-snapshot to keep the on-chain stash honest.
    pub total_allocation_lamports: Option<u64>,
    /// New treasury authority PDA, or `None` to leave unchanged.
    pub treasury_authority: Option<Pubkey>,
}

#[derive(Accounts)]
pub struct UpdateMegadrop<'info> {
    /// Must equal `crate::ADMIN_AUTHORITY`. The constraint below is what
    /// gates this whole ix — originally absent, leading to the
    /// "any-signer can rewrite the merkle root" CVE.
    #[account(
        constraint = authority.key() == crate::ADMIN_AUTHORITY @ MegadropError::Unauthorized,
    )]
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [MEGADROP_CONFIG_SEED],
        bump = megadrop_config.bump,
    )]
    pub megadrop_config: Account<'info, MegadropConfig>,
}

/// Handler — patch any provided fields in place.
pub fn handler(ctx: Context<UpdateMegadrop>, args: UpdateMegadropArgs) -> Result<()> {
    let cfg = &mut ctx.accounts.megadrop_config;

    if let Some(root) = args.claimable_root {
        cfg.claimable_root = root;
    }
    if let Some(month) = args.genesis_month {
        // Same plausibility check as `init_megadrop` so we can't accidentally
        // brick the unlock schedule by patching to a Unix timestamp.
        let year = month / 100;
        let m = month % 100;
        require!(
            (1970..=9999).contains(&year) && (1..=12).contains(&m),
            MegadropError::BadInitArgs
        );
        cfg.genesis_month = month;
    }
    if let Some(total) = args.total_allocation_lamports {
        require!(total > 0, MegadropError::BadInitArgs);
        cfg.total_allocation_lamports = total;
    }
    if let Some(treasury) = args.treasury_authority {
        cfg.treasury_authority = treasury;
    }

    Ok(())
}

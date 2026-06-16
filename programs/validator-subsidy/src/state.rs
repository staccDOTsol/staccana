//! On-chain state PDAs for the validator-subsidy program.
//!
//! Four top-level account kinds:
//!
//! - [`SubsidyConfig`] — global config: bridge program id, productive-position vault,
//!   bootstrap reserve, federation set, accounting cursors.
//!   PDA seeds: `["subsidy_config"]`.
//! - [`ValidatorRegistry`] — flat list of validator pubkeys; the registry exists so
//!   `distribute_yield` can iterate the population without per-call governance updates.
//!   PDA seeds: `["validator_registry"]`.
//! - [`ValidatorRecord`] — per-validator metrics + lifetime totals.
//!   PDA seeds: `["validator", validator_pubkey]`.
//! - [`EpochAccrual`] — per-epoch yield-observation + distribution mark.
//!   PDA seeds: `["accrual", epoch_le]`.

use anchor_lang::prelude::*;

/// Hard cap on validators in the registry. Sized for v1 (single-digit validators) plus
/// generous headroom; bumping requires a redeploy. The registry is iterated linearly in
/// `distribute_yield`, so the upper bound also caps distribution-ix CU cost.
///
/// Originally cranked down to 8 to dodge SBPF's 4 KB stack frame —
/// `Account<ValidatorRegistry>` deserializes the entire `[Pubkey; MAX]`
/// onto the stack, and at 64 that pushed `init_subsidy` /
/// `register_validator` over the per-frame budget (`Access violation in
/// stack frame 3`). Now safe at 64 because `ValidatorRegistry` is
/// `#[account(zero_copy)]` — the array lives in the account-data buffer,
/// not on the stack.
pub const MAX_VALIDATORS: usize = 256;

/// Hard cap on federation set size. Capped at 16 (down from 32) because
/// `SubsidyConfig` is still a borsh `#[account]` (not zero_copy — its
/// borsh-dense layout differs from `repr(C)` due to u32→u64 padding,
/// and there's already on-chain data we don't want to migrate). 16 is
/// still 2x the production federation size (9-of-9). Keeps the
/// stack-allocated SubsidyConfig under 700 bytes so `register_validator`
/// + similar ixs don't blow the SBPF frame.
pub const MAX_FEDERATION_MEMBERS: usize = 16;

/// SPEC §7.3 constants pinned next to consumers. Values here are normative — if SPEC.md
/// changes, edit both in lockstep.
pub const TREASURY_PRODUCTIVE_BPS: u16 = 8000;
pub const TREASURY_BOOTSTRAP_BPS: u16 = 200;
pub const BOOTSTRAP_EPOCHS: u64 = 60;
pub const SUBSIDY_DISTRIBUTION_EVERY: u64 = 1;

/// Global config for the subsidy machinery. Only one instance per chain — PDA derived
/// from `["subsidy_config"]`.
///
/// Stays as `#[account]` (borsh) — converting to zero_copy would require
/// migrating the existing on-chain bytes since `repr(C)` adds 4 bytes of
/// padding between `productive_asset_id: u32` and `productive_deposit_total:
/// u64` that borsh's dense layout doesn't have. With `MAX_FEDERATION_MEMBERS`
/// shrunk to 16, the stack footprint here is ~655 bytes, fine for all
/// handlers.
#[account]
pub struct SubsidyConfig {
    /// Governance multisig authority. All gated operations
    /// (`stake_to_productive`, `unstake_from_productive`, `register_validator`) must be
    /// signed by this key. v1 expects a Squads vault; the program treats it as an opaque
    /// pubkey.
    pub governance: Pubkey,

    /// Bridge program id. Used to bind CPI calls and verify the registered productive-
    /// position vault was issued by the correct bridge.
    pub bridge_program_id: Pubkey,

    /// PDA address of the productive-position vault (the bridge's `AssetConfig` PDA for
    /// the staked-asset id, e.g. pSYRUP).
    pub productive_vault: Pubkey,

    /// `asset_id` (per the bridge's `AssetConfig`) of the productive position. v1: the
    /// pSYRUP asset id. Stored so CPI seed derivation doesn't need an off-chain hint.
    pub productive_asset_id: u32,

    /// Total amount of underlying SOL deposited into the productive position over the
    /// lifetime of the program. Increased by `stake_to_productive`, decreased by
    /// `unstake_from_productive`. Used as a sanity gate; not used in payout math.
    pub productive_deposit_total: u64,

    /// Initial bootstrap reserve in lamports — `treasury_total * TREASURY_BOOTSTRAP_BPS
    /// / 10_000` at `init_subsidy` time. Immutable after init.
    pub bootstrap_reserve_initial: u64,

    /// Remaining bootstrap reserve in lamports. Decremented on each
    /// `bootstrap_distribute` call until zero.
    pub bootstrap_reserve_remaining: u64,

    /// Most recently distributed epoch (yield or bootstrap, whichever ran last). Used by
    /// off-chain tooling to discover the catch-up cursor; not enforced on-chain
    /// (per-epoch idempotency is enforced via the `EpochAccrual.distributed` flag).
    pub last_distributed_epoch: u64,

    /// Federation threshold (M) for `update_validator_metrics`. Same M-of-N pattern as
    /// the bridge's `update_ratio`.
    pub federation_m: u8,

    /// Federation member count (N).
    pub federation_n: u8,

    /// Federation pubkeys. Slots beyond `federation_n` are zero-filled.
    pub federation_members: [Pubkey; MAX_FEDERATION_MEMBERS],

    /// PDA bump cache.
    pub bump: u8,
}

impl SubsidyConfig {
    /// Anchor discriminator (8) + governance (32) + bridge_program_id (32)
    /// + productive_vault (32) + productive_asset_id (4) + productive_deposit_total (8)
    /// + bootstrap_reserve_initial (8) + bootstrap_reserve_remaining (8)
    /// + last_distributed_epoch (8) + federation_m (1) + federation_n (1)
    /// + federation_members (32 * 32) + bump (1).
    pub const SPACE: usize =
        8 + 32 + 32 + 32 + 4 + 8 + 8 + 8 + 8 + 1 + 1 + (32 * MAX_FEDERATION_MEMBERS) + 1;
}

impl Default for SubsidyConfig {
    fn default() -> Self {
        Self {
            governance: Pubkey::default(),
            bridge_program_id: Pubkey::default(),
            productive_vault: Pubkey::default(),
            productive_asset_id: 0,
            productive_deposit_total: 0,
            bootstrap_reserve_initial: 0,
            bootstrap_reserve_remaining: 0,
            last_distributed_epoch: 0,
            federation_m: 0,
            federation_n: 0,
            federation_members: [Pubkey::default(); MAX_FEDERATION_MEMBERS],
            bump: 0,
        }
    }
}

/// Flat list of registered validator pubkeys. Order is insertion order. `distribute_yield`
/// iterates this list and expects the caller to pass each `ValidatorRecord` (in the same
/// order) via `remaining_accounts`.
///
/// Stored as `#[account(zero_copy)]` so the `[Pubkey; MAX_VALIDATORS]` array
/// lives in the account-data buffer, NOT on SBPF's 4 KB stack frame.
/// Earlier the regular `#[account]` form deserialized the entire registry
/// onto the handler stack — at MAX=64 that pushed `register_validator` past
/// the per-frame budget (`Stack offset of 4104 exceeded max offset of 4096`).
/// zero_copy removes that pressure entirely.
///
/// The PDA bump is NOT cached (was previously a `bump: u8` field) — Anchor
/// re-derives it via `find_program_address` on each call. Cost is ~1500 CU
/// per ix that touches the registry, well under any reasonable budget.
/// Removing the cached bump also keeps the layout migration trivial: the
/// new (larger) struct is bytemuck-compatible with the existing on-chain
/// bytes after a simple `realloc` to the new size.
#[account(zero_copy(unsafe))]
#[repr(C)]
pub struct ValidatorRegistry {
    /// Number of validators currently in the registry (`<= MAX_VALIDATORS`).
    pub count: u32,

    /// Backing storage. Slots beyond `count` are zero-filled.
    /// Pubkey is `[u8; 32]` underneath (1-byte aligned) so no padding is
    /// needed between `count` (u32, 4-aligned) and `validators`.
    pub validators: [Pubkey; MAX_VALIDATORS],
}

impl ValidatorRegistry {
    /// Anchor discriminator (8) + count (4) + validators (32 * MAX_VALIDATORS).
    pub const SPACE: usize = 8 + 4 + (32 * MAX_VALIDATORS);
}

/// Per-validator metrics and lifetime totals. Updated by federation-attested
/// `update_validator_metrics`; consumed by `distribute_yield` and `bootstrap_distribute`.
#[account]
#[derive(Default)]
pub struct ValidatorRecord {
    /// Validator identity address. Sanity field; PDA seeds bind too.
    pub validator: Pubkey,

    /// Most recent uptime metric, in basis points (10_000 == 100%).
    pub uptime_bps: u16,

    /// Most recent delegated stake (lamports). NOT the validator's own balance — this
    /// is the total stake delegated to the validator's vote account.
    pub delegated_stake: u64,

    /// Votes cast in the metrics window (typically the prior epoch).
    pub votes_cast: u64,

    /// Slot at which the metrics were observed by the federation. Equivalent to the
    /// bridge's `last_published_slot`.
    pub last_metrics_slot: u64,

    /// Most recent metrics nonce. Strictly increasing per validator.
    pub last_metrics_nonce: u64,

    /// Last epoch in which this validator received a distribution
    /// (yield or bootstrap). Used by off-chain tooling to detect skipped epochs.
    pub last_distribution_epoch: u64,

    /// Lifetime sum of subsidy lamports paid to this validator.
    pub total_subsidy_received: u64,

    /// PDA bump cache.
    pub bump: u8,
}

impl ValidatorRecord {
    /// Anchor discriminator (8) + validator (32) + uptime_bps (2) + delegated_stake (8)
    /// + votes_cast (8) + last_metrics_slot (8) + last_metrics_nonce (8)
    /// + last_distribution_epoch (8) + total_subsidy_received (8) + bump (1).
    pub const SPACE: usize = 8 + 32 + 2 + 8 + 8 + 8 + 8 + 8 + 8 + 1;

    /// Deserialize a `ValidatorRecord` from raw account info, validating the Anchor
    /// discriminator. Used by `distribute_yield` / `bootstrap_distribute` to read
    /// records passed via `remaining_accounts`, where `Account::try_from` doesn't
    /// satisfy the lifetime bounds (`&'a AccountInfo<'a>` requires both lifetimes to
    /// match, but `ctx.remaining_accounts` produces `&'c [AccountInfo<'info>]` with
    /// `'c != 'info`).
    pub fn read_from(ai: &AccountInfo) -> Result<Self> {
        let data = ai.try_borrow_data()?;
        let mut slice: &[u8] = &data;
        Self::try_deserialize(&mut slice)
    }

    /// Serialize a `ValidatorRecord` back to its raw account info, preserving the
    /// Anchor discriminator at the head. Pairs with [`Self::read_from`].
    pub fn write_to(&self, ai: &AccountInfo) -> Result<()> {
        let mut data = ai.try_borrow_mut_data()?;
        let mut writer: &mut [u8] = &mut data;
        self.try_serialize(&mut writer)
    }
}

/// Per-epoch ledger entry. Created lazily — first call to `populate_epoch_yield`
/// (off-chain attestor task) inits it; `distribute_yield` consumes it.
///
/// Note: this crate does NOT include the attestor-facing `populate_epoch_yield` ix —
/// that's a v1.1 attestor task. The on-chain shape is fixed here so the attestor can
/// land its own ix without renegotiating the layout.
#[account]
#[derive(Default)]
pub struct EpochAccrual {
    /// Sanity field; PDA seeds bind too.
    pub epoch: u64,

    /// Yield observed by the federation/oracle from the productive position over this
    /// epoch, in lamports. Populated by an attestor ix (out of scope for v1) or, for
    /// pre-bootstrap epochs, set to zero.
    pub yield_observed: u64,

    /// Set to `true` when `distribute_yield` (or `bootstrap_distribute`) successfully
    /// pays out this epoch. Second call rejects with `EpochAlreadyDistributed`.
    pub distributed: bool,

    /// Sum of validator weights at distribution time. Stored so off-chain tooling can
    /// reproduce the per-validator share without re-loading every record.
    pub total_weight: u128,

    /// Sum of lamports distributed in this epoch (yield + bootstrap, but for v1 only
    /// one of the two ever runs per epoch). Useful for accounting.
    pub distributed_total: u64,

    /// Optional Merkle root of the per-validator shares — reserved for v1.1 when the
    /// validator set may exceed what fits in `remaining_accounts` and a
    /// claim-against-root flow is more efficient than an explicit transfer to each
    /// validator. `[0u8; 32]` for v1 distributions which always pay inline.
    pub distribution_root: [u8; 32],

    /// PDA bump cache.
    pub bump: u8,
}

impl EpochAccrual {
    /// Anchor discriminator (8) + epoch (8) + yield_observed (8) + distributed (1)
    /// + total_weight (16) + distributed_total (8) + distribution_root (32) + bump (1).
    pub const SPACE: usize = 8 + 8 + 8 + 1 + 16 + 8 + 32 + 1;
}

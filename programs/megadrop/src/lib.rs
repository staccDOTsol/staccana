//! Staccana megadrop program.
//!
//! Implements the holder-claim flow described in `docs/MEGADROP.md`:
//! snapshotted holders of two Solana mainnet collections — `based_stacc_0` (Metaplex NFT
//! collection) and `proofv3` (Token-22 SPL fungible mint) — pull their per-holder
//! allocation **out of the staccana treasury** in 10 equal monthly tranches starting at
//! the chain's launch month.
//!
//! Architectural shape mirrors the lazy-claim program (Merkle inclusion + ed25519
//! precompile + per-pubkey claimed marker), with two extensions:
//!
//! - The "claimed marker" is an upgradable [`state::ClaimedMegadrop`] PDA carrying a
//!   16-bit tranche bitmap rather than a one-shot existence check, so the same holder
//!   can return month after month and consume tranches one at a time.
//! - Each requested tranche is gated on a calendar-month unlock check: tranche `i`
//!   (1..=10) requires `current_month >= genesis_month + (i - 1)`. Calendar math is in
//!   [`calendar`].
//!
//! Module layout:
//!
//! - [`state`]    — `MegadropConfig`, `ClaimedMegadrop`
//! - [`error`]    — typed `MegadropError` codes
//! - [`megadrop`] — pure helpers for tranche math, message construction, bitmap ops
//! - [`calendar`] — Unix timestamp → yyyymm conversion (handles leap years)
//! - [`merkle`]   — Merkle proof verification (mirrors `staccana-genesis::merkle`)
//! - [`ed25519`]  — Instructions-sysvar precompile reader (mirrors bridge / subsidy)
//! - [`instructions`] — handler modules for each ix
//!
//! Instructions:
//!
//! 1. `init_megadrop` — governance one-shot: sets the Merkle root, genesis month, and
//!    treasury authority. See [`instructions::init_megadrop`].
//! 2. `claim_megadrop` — the user-facing instruction. Verifies Merkle proof + ed25519
//!    signature + per-tranche unlock + per-tranche freshness, then debits the treasury
//!    PDA and marks the requested tranche bits. See [`instructions::claim_megadrop`].
//!
//! See `docs/MEGADROP.md` for the normative wire format and verification order, and
//! `docs/SPEC.md` §4 (lazy-claim — the architectural pattern this program mirrors) and
//! §7 (treasury — the source of allocations).

// Anchor 1.0 fires a deprecation warning for raw `AccountInfo` use inside `Accounts`
// derives (preferring `UncheckedAccount`). The semantics are unchanged. The treasury
// drain plumbing here passes account infos through to direct lamport mutation, where
// `AccountInfo` is the clearest expression of intent — suppress crate-wide rather than
// rewriting every account context.
#![allow(deprecated)]

use anchor_lang::prelude::*;

pub mod calendar;
pub mod ed25519;
pub mod error;
pub mod instructions;
pub mod megadrop;
pub mod merkle;
pub mod state;

pub use error::MegadropError;
pub use instructions::*;

// Placeholder program ID. Replace with the real deployed address before mainnet launch.
// 43-character base58 string starting with the human-readable prefix "Megadrop" and
// padded with `1`s; decodes to exactly 32 bytes (verified via
// `base58.b58decode("Megadrop1111...111").length == 32`). The 42-character form in the
// task spec was one byte short, so this string is one `1` longer.
// Real deployed program ID (replaces the placeholder vanity address).
// Anchor 1.x's `#[program]` macro injects a runtime check that the runtime
// `program_id` == `crate::id()`; mismatch returns `DeclaredProgramIdMismatch
// (Anchor 4100)`. The original placeholder went out the door because the
// previous Anchor toolchain didn't insert that check, but the upgrade we
// just landed bumped to a newer expansion that does.
declare_id!("Aicff1zk6b5ifYzFoyhenUD5ehhFYb8GiDbRCrWt9t34");

/// Hardcoded admin pubkey gating privileged ixs (`update_megadrop`).
///
/// Originally `update_megadrop` accepted **any signer** — the comment said
/// "production deployments should gate this off the upgrade authority via
/// `solana program set-upgrade-authority`", but no such constraint was
/// enforced on chain. A friendly auditor demonstrated the obvious
/// consequence: anyone could call `update_megadrop` with their own
/// `claimable_root` and replace the snapshot — siphoning the entire
/// allocation through claims that match THEIR root. devnet, no real funds,
/// but still a hard-coded ROFL.
///
/// This const is the staccana cluster's BPF program upgrade authority
/// (the same key that signs `solana program deploy --buffer-authority …`).
/// Future revisions can graduate to a runtime-stored field on
/// `MegadropConfig` (set during init, rotatable via a separate ix gated by
/// itself), but that needs a state migration on the existing PDA — for now
/// the const lock-down avoids touching the deployed account layout.
///
/// Keypair lives at `/etc/staccana/keys/upgrade-authority.json` on val-1.
// Anchor 1.x doesn't re-export `pubkey!` — use the const-fn path directly.
pub const ADMIN_AUTHORITY: Pubkey =
    Pubkey::from_str_const("HSwe2Y7i6CPuJGb27rBwUumt8HZ8sCpQvG4PBBiC5f4y");

#[program]
pub mod staccana_megadrop {
    use super::*;

    /// Governance-gated one-shot. Initializes the singleton `MegadropConfig` PDA with
    /// the snapshot Merkle root, the genesis month (yyyymm — first tranche unlock), the
    /// total allocation summed across all leaves (sanity check), and the treasury
    /// authority (PDA-derived signer that drains the treasury). See
    /// [`instructions::init_megadrop`].
    pub fn init_megadrop(ctx: Context<InitMegadrop>, args: InitMegadropArgs) -> Result<()> {
        instructions::init_megadrop::handler(ctx, args)
    }

    /// Authority-gated patch of the singleton `MegadropConfig` PDA — lets us
    /// rotate the Merkle root after a re-snapshot, fix the genesis month,
    /// or update the treasury authority without rebuilding genesis. Each
    /// field is optional in the args; only provided fields are written. See
    /// [`instructions::update_megadrop`].
    pub fn update_megadrop(
        ctx: Context<UpdateMegadrop>,
        args: UpdateMegadropArgs,
    ) -> Result<()> {
        instructions::update_megadrop::handler(ctx, args)
    }

    /// Holder-initiated claim. Anyone can submit (the holder, or a relayer on their
    /// behalf — but the holder must have produced a fresh ed25519 signature on the
    /// canonical message), and the lamports always land at the holder's pubkey.
    ///
    /// Verification order:
    /// 1. Merkle proof against `MegadropConfig.claimable_root` for `(holder, total)`.
    /// 2. ed25519 sig from `holder` (via prior precompile + Instructions sysvar).
    /// 3. Each requested tranche is unlocked (current month gate).
    /// 4. Each requested tranche is unclaimed (bitmap check).
    ///
    /// Effects:
    /// 1. Mark the requested tranche bits in `ClaimedMegadrop.tranches_claimed`.
    /// 2. Compute `claim_amount = sum_of_requested_tranches × (total / 10)`.
    /// 3. Debit `claim_amount` lamports from the treasury PDA → holder pubkey.
    /// 4. Update `ClaimedMegadrop.total_claimed_lamports`.
    ///
    /// See [`instructions::claim_megadrop`].
    pub fn claim_megadrop(
        ctx: Context<ClaimMegadrop>,
        args: ClaimMegadropArgs,
    ) -> Result<()> {
        instructions::claim_megadrop::handler(ctx, args)
    }

    /// Allocate a per-(holder, payer) proof-buffer PDA for staging long Merkle
    /// proofs across multiple txs. See `instructions::proof_buffer` for layout.
    pub fn init_megadrop_proof_buffer(
        ctx: Context<InitMegadropProofBuffer>,
        args: InitMegadropProofBufferArgs,
    ) -> Result<()> {
        instructions::proof_buffer::init_proof_buffer_handler(ctx, args)
    }

    /// Append `bytes` into a previously initialized proof buffer at `offset`.
    /// Idempotent on offset; updates the high-water mark.
    pub fn write_megadrop_proof_buffer(
        ctx: Context<WriteMegadropProofBuffer>,
        args: WriteMegadropProofBufferArgs,
    ) -> Result<()> {
        instructions::proof_buffer::write_proof_buffer_handler(ctx, args)
    }

    /// Final claim using a staged proof buffer instead of inline proof bytes.
    /// Same checks as `claim_megadrop`; closes the buffer (rent → relayer) on success.
    pub fn claim_megadrop_from_buffer(
        ctx: Context<ClaimMegadropFromBuffer>,
        args: ClaimMegadropFromBufferArgs,
    ) -> Result<()> {
        instructions::proof_buffer::claim_megadrop_from_buffer_handler(ctx, args)
    }
}

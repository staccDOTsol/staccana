//! On-chain state PDAs for the megadrop program.
//!
//! Two account kinds:
//!
//! - [`MegadropConfig`] — singleton, holds the snapshot Merkle root + genesis month.
//!   PDA seeds: `["megadrop_config"]`. Created by `init_megadrop`; immutable after.
//! - [`ClaimedMegadrop`] — per-holder PDA tracking which tranche bits have been
//!   claimed. PDA seeds: `["megadrop_claimed", holder_pubkey]`. Created lazily on the
//!   holder's first claim; mutated in place on subsequent claims.
//!
//! Both are Anchor-style accounts (8-byte discriminator + Borsh-serialized fields) for
//! consistency with the bridge / validator-subsidy crates.

use anchor_lang::prelude::*;

/// PDA seed for the singleton config.
pub const MEGADROP_CONFIG_SEED: &[u8] = b"megadrop_config";

/// PDA seed prefix for per-holder claim tracking.
pub const CLAIMED_MEGADROP_SEED: &[u8] = b"megadrop_claimed";

/// PDA seed for the treasury authority that signs the claim debit. Concretely the
/// genesis builder MUST set the treasury PDA (the lamport-bearing account) to be owned
/// by THIS program OR install a special-cased treasury-debit rule scoped to this
/// program — see SPEC §7.1 for the authorized-operations list. The handler doesn't
/// CPI; it directly mutates the treasury's lamports field. This works only when the
/// treasury PDA is owned by this program (the usual case for Anchor PDAs).
pub const TREASURY_AUTHORITY_SEED: &[u8] = b"megadrop_treasury";

/// Number of vesting tranches per holder. Hard-coded per `docs/MEGADROP.md` (§§
/// "Vesting", "Allocation parameters"); each tranche is `total_allocation / 10`.
pub const NUM_TRANCHES: u8 = 10;

/// Singleton config holding the snapshot Merkle root and the calendar genesis month.
/// Created exactly once by `init_megadrop`; the field set is immutable after init.
#[account]
pub struct MegadropConfig {
    /// Merkle root over `(holder_pubkey, total_allocation_lamports)` leaves, sorted by
    /// pubkey ascending. Same hashing rules as `staccana_genesis::merkle`
    /// (LEAF_DOMAIN=0x00, NODE_DOMAIN=0x01, SHA-256, sorted-pubkey leaves).
    pub claimable_root: [u8; 32],

    /// First tranche unlock month, expressed as ISO `yyyymm` (e.g. `202605` = May
    /// 2026, the staccana mainnet-sigma launch month). Tranche `i` (1..=10) unlocks at
    /// `genesis_month + (i - 1)`; calendar wraparound is handled by [`crate::calendar`].
    pub genesis_month: u32,

    /// Sum of all leaf allocations. Sanity check the snapshot tool's accounting; not
    /// used in claim math (per-leaf `total_allocation` is what's verified).
    pub total_allocation_lamports: u64,

    /// PDA-derived signer that drains the treasury. Recorded here so the claim handler
    /// can verify the supplied treasury account against the configured authority
    /// without re-derivation on the hot path. The actual lamport-bearing treasury PDA
    /// is `["treasury"]` per SPEC §7.1; this field IS that PDA's address (the program
    /// signs for `["treasury"]` via the singleton authority because the genesis builder
    /// set treasury ownership accordingly — see `state.rs` module doc).
    pub treasury_authority: Pubkey,

    /// PDA bump cache.
    pub bump: u8,
}

impl MegadropConfig {
    /// Anchor discriminator (8) + claimable_root (32) + genesis_month (4)
    /// + total_allocation_lamports (8) + treasury_authority (32) + bump (1).
    pub const SPACE: usize = 8 + 32 + 4 + 8 + 32 + 1;
}

impl Default for MegadropConfig {
    fn default() -> Self {
        Self {
            claimable_root: [0u8; 32],
            genesis_month: 0,
            total_allocation_lamports: 0,
            treasury_authority: Pubkey::default(),
            bump: 0,
        }
    }
}

/// Per-holder claim state. Created lazily on the holder's first `claim_megadrop` call;
/// mutated in place on each subsequent call as more tranches unlock.
#[account]
#[derive(Default)]
pub struct ClaimedMegadrop {
    /// Holder pubkey — sanity field; PDA seeds bind too.
    pub holder: Pubkey,

    /// Mirror of the leaf's `total_allocation_lamports`. Stored at first claim so that
    /// repeat claim ixs can validate-then-skip the (immutable) Merkle proof against
    /// the same leaf, and so off-chain UIs can see "how much do I have allocated"
    /// without re-walking the snapshot.
    pub total_allocation: u64,

    /// 16-bit bitmap of which tranches have been claimed. Bit `i` set ⇒ tranche
    /// `(i + 1)` claimed (so tranche 1 is bit 0, tranche 10 is bit 9). Six high bits
    /// stay unused since `NUM_TRANCHES == 10`. u16 (rather than u8) leaves headroom
    /// for a future revision that wants more than 10 tranches without an account
    /// migration — the field is fixed-width so the schema stays stable.
    pub tranches_claimed: u16,

    /// Lifetime sum of lamports paid out to this holder. Equal to
    /// `popcount(tranches_claimed) × (total_allocation / 10)`; cached so off-chain UIs
    /// don't have to do the math.
    pub total_claimed_lamports: u64,

    /// PDA bump cache.
    pub bump: u8,
}

impl ClaimedMegadrop {
    /// Anchor discriminator (8) + holder (32) + total_allocation (8)
    /// + tranches_claimed (2) + total_claimed_lamports (8) + bump (1).
    pub const SPACE: usize = 8 + 32 + 8 + 2 + 8 + 1;
}

/// Derive the singleton config PDA address and bump seed.
pub fn find_megadrop_config_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MEGADROP_CONFIG_SEED], program_id)
}

/// Derive the per-holder claimed-megadrop PDA address and bump seed.
pub fn find_claimed_megadrop_pda(holder: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[CLAIMED_MEGADROP_SEED, holder.as_ref()], program_id)
}

/// Derive the treasury authority PDA address and bump seed. The treasury account
/// itself is `["treasury"]` per SPEC §7.1; this distinct authority seed is owned by
/// the megadrop program and stored in `MegadropConfig.treasury_authority` so the
/// init-time governance signer can supply / verify the binding.
pub fn find_treasury_authority_pda(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[TREASURY_AUTHORITY_SEED], program_id)
}

/// PDA seed prefix for a per-(holder, payer) megadrop proof-buffer staging account.
///
/// Full seeds: `["megadrop_proof_buffer", holder, payer]`. Multiple users staging at
/// the same time → no PDA collision. The buffer is closed on `ClaimMegadropFromBuffer`.
pub const MEGADROP_PROOF_BUFFER_SEED: &[u8] = b"megadrop_proof_buffer";

/// Header for the megadrop proof-buffer staging account. Identical layout to the
/// lazy-claim version (see `programs/lazy-claim/src/state.rs::ProofBufferHeader`):
///
/// * `[0..1]`   discriminator (constant `0x03`)
/// * `[1..2]`   version (currently `0x01`)
/// * `[2..4]`   reserved
/// * `[4..8]`   total_len (LE u32)
/// * `[8..12]`  bytes_written (LE u32)
/// * `[12..16]` reserved
/// * `[16..]`   raw proof bytes (siblings concatenated, 32-byte each)
pub const PROOF_BUFFER_DISCRIMINATOR: u8 = 0x03;
pub const PROOF_BUFFER_VERSION: u8 = 0x01;
pub const PROOF_BUFFER_HEADER_SIZE: usize = 16;

/// Derive the megadrop proof-buffer PDA for `(holder, payer)`.
pub fn find_megadrop_proof_buffer_pda(
    holder: &Pubkey,
    payer: &Pubkey,
    program_id: &Pubkey,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[MEGADROP_PROOF_BUFFER_SEED, holder.as_ref(), payer.as_ref()],
        program_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn config_pda_is_deterministic() {
        let prog = pk(7);
        let (a1, b1) = find_megadrop_config_pda(&prog);
        let (a2, b2) = find_megadrop_config_pda(&prog);
        assert_eq!(a1, a2);
        assert_eq!(b1, b2);
    }

    #[test]
    fn claimed_pda_changes_with_holder() {
        let prog = pk(1);
        let (addr_a, _) = find_claimed_megadrop_pda(&pk(2), &prog);
        let (addr_b, _) = find_claimed_megadrop_pda(&pk(3), &prog);
        assert_ne!(addr_a, addr_b);
    }

    #[test]
    fn claimed_pda_changes_with_program_id() {
        let holder = pk(99);
        let (addr_a, _) = find_claimed_megadrop_pda(&holder, &pk(1));
        let (addr_b, _) = find_claimed_megadrop_pda(&holder, &pk(2));
        assert_ne!(addr_a, addr_b);
    }

    #[test]
    fn treasury_authority_is_deterministic() {
        let prog = pk(7);
        let (a1, b1) = find_treasury_authority_pda(&prog);
        let (a2, b2) = find_treasury_authority_pda(&prog);
        assert_eq!(a1, a2);
        assert_eq!(b1, b2);
    }

    #[test]
    fn config_size_matches_field_layout() {
        // Sanity check the SPACE calculation against the field byte count.
        // 8 disc + 32 root + 4 month + 8 total + 32 authority + 1 bump = 85.
        assert_eq!(MegadropConfig::SPACE, 85);
    }

    #[test]
    fn claimed_size_matches_field_layout() {
        // 8 disc + 32 holder + 8 total + 2 bitmap + 8 paid + 1 bump = 59.
        assert_eq!(ClaimedMegadrop::SPACE, 59);
    }

    #[test]
    fn num_tranches_is_ten() {
        // Per docs/MEGADROP.md "Vesting" — fixed at 10. Catches a typo refactor.
        assert_eq!(NUM_TRANCHES, 10);
    }
}

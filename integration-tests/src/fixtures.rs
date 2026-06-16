//! Deterministic fixtures for cross-crate integration tests.
//!
//! Two kinds of helpers live here:
//!
//! 1. **Synthetic snapshot accounts** — small, type-erased records that implement
//!    [`staccana_genesis::Account`] so tests can drive `build_genesis` without depending on
//!    the snapshot-fork crate's `AccountRecord` (which itself works fine, but using a local
//!    type keeps test-only fields close to the test layer).
//! 2. **Deterministic keypair derivation** — `solana_sdk::Keypair` is heavyweight to spin
//!    up reproducibly; this module exposes a tiny seeded helper that returns a fresh
//!    `Keypair` for a given byte seed. Used by the claim-flow tests where we need a real
//!    ed25519 signer matched to a known pubkey.
//!
//! Everything here is `pub` and re-exported from [`crate`] so test files can `use
//! staccana_integration_tests::*;`.
//!
//! ## Style
//!
//! Match the terseness of `matcher/src/batch.rs` and `genesis/src/builder.rs`: small
//! helpers, descriptive names, no clever abstraction. The point is to make the test
//! files themselves readable.

use solana_program::pubkey::Pubkey;
use staccana_genesis::partition::{Account, SYSTEM_PROGRAM_ID};

/// A snapshot account that shows up in the synthetic snapshots used by the integration
/// tests. Fields are public so tests can poke them; impls are only what
/// [`staccana_genesis::Account`] actually requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticAccount {
    pub pubkey: Pubkey,
    pub owner: Pubkey,
    pub data_len: usize,
    pub lamports: u64,
}

impl Account for SyntheticAccount {
    fn pubkey(&self) -> &Pubkey {
        &self.pubkey
    }
    fn owner(&self) -> &Pubkey {
        &self.owner
    }
    fn data_len(&self) -> usize {
        self.data_len
    }
    fn lamports(&self) -> u64 {
        self.lamports
    }
}

/// Build a deterministic 32-byte pubkey from a single byte. Same convention as the unit
/// tests in `matcher/src/batch.rs` and `genesis/src/builder.rs`.
pub fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

/// A claimable EOA: system-owned, zero data, lamport balance > 0. Goes into the Merkle
/// tree per SPEC §3.1.
pub fn synthetic_eoa(byte: u8, lamports: u64) -> SyntheticAccount {
    SyntheticAccount {
        pubkey: pk(byte),
        owner: SYSTEM_PROGRAM_ID,
        data_len: 0,
        lamports,
    }
}

/// A token account: owned by the SPL Token program (placeholder pubkey 0x99) with the
/// canonical 165-byte data layout. Always lands in the treasury partition.
pub fn synthetic_token_account(byte: u8, lamports: u64) -> SyntheticAccount {
    SyntheticAccount {
        pubkey: pk(byte),
        owner: pk(0x99),
        data_len: 165,
        lamports,
    }
}

/// A stake account: owned by the Stake program (placeholder pubkey 0x88) with the
/// canonical 200-byte data layout. Always lands in the treasury partition.
pub fn synthetic_stake_account(byte: u8, lamports: u64) -> SyntheticAccount {
    SyntheticAccount {
        pubkey: pk(byte),
        owner: pk(0x88),
        data_len: 200,
        lamports,
    }
}

/// A PDA: system-owned but with non-zero data (e.g. a name service entry) so it lands in
/// the treasury partition under the strict §3.1 rule. Catches the partition's
/// "system-owned-with-data" edge case.
pub fn synthetic_pda(byte: u8, lamports: u64, data_len: usize) -> SyntheticAccount {
    SyntheticAccount {
        pubkey: pk(byte),
        owner: SYSTEM_PROGRAM_ID,
        data_len,
        lamports,
    }
}

/// Build a mixed synthetic snapshot containing all four account flavors above.
///
/// The mix is intentionally hand-chosen to exercise both partition outcomes and to keep
/// the per-test setup short. The returned vector is in insertion order; downstream code
/// that depends on order should sort first (the genesis builder is order-independent
/// internally).
pub fn mixed_snapshot() -> Vec<SyntheticAccount> {
    vec![
        synthetic_eoa(0x01, 1_000_000_000),
        synthetic_eoa(0x02, 2_000_000_000),
        synthetic_token_account(0x03, 2_039_280),
        synthetic_token_account(0x04, 2_039_280),
        synthetic_eoa(0x05, 500_000_000),
        synthetic_stake_account(0x06, 5_000_000_000),
        synthetic_pda(0x07, 100_000, 64),
        synthetic_eoa(0x08, 750_000_000),
    ]
}

/// Derive a deterministic ed25519 keypair from a byte seed.
///
/// Useful for tests that need a real signer whose pubkey is reproducible across runs (the
/// claim-flow tests, in particular). We don't take an `&[u8]` slice because every
/// integration test happens to use a single-byte seed; widening the API can wait.
pub fn deterministic_keypair(seed_byte: u8) -> solana_sdk::signature::Keypair {
    let mut seed = [0u8; 32];
    seed[0] = seed_byte;
    // Pad the remaining bytes with a fixed pattern so different seed_bytes produce
    // genuinely different keypairs (a single non-zero byte is enough — the curve maps any
    // 32-byte input to a unique private scalar — but we vary every byte for clarity).
    for (i, b) in seed.iter_mut().enumerate().skip(1) {
        *b = seed_byte.wrapping_add(i as u8);
    }
    use solana_sdk::signature::SeedDerivable;
    solana_sdk::signature::Keypair::from_seed(&seed).expect("32-byte seed accepted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use staccana_genesis::partition::{partition, Disposition};

    #[test]
    fn synthetic_eoa_partitions_as_claimable() {
        let a = synthetic_eoa(1, 100);
        assert_eq!(partition(&a), Disposition::Claimable);
    }

    #[test]
    fn synthetic_token_account_partitions_as_treasury() {
        let a = synthetic_token_account(2, 100);
        assert_eq!(partition(&a), Disposition::Treasury);
    }

    #[test]
    fn synthetic_stake_account_partitions_as_treasury() {
        let a = synthetic_stake_account(3, 100);
        assert_eq!(partition(&a), Disposition::Treasury);
    }

    #[test]
    fn synthetic_pda_with_data_partitions_as_treasury() {
        let a = synthetic_pda(4, 100, 64);
        assert_eq!(partition(&a), Disposition::Treasury);
    }

    #[test]
    fn mixed_snapshot_yields_expected_partition_split() {
        let accounts = mixed_snapshot();
        let claimable = accounts
            .iter()
            .filter(|a| partition(*a) == Disposition::Claimable)
            .count();
        let treasury = accounts
            .iter()
            .filter(|a| partition(*a) == Disposition::Treasury)
            .count();
        assert_eq!(claimable, 4);
        assert_eq!(treasury, 4);
    }

    #[test]
    fn deterministic_keypair_is_reproducible() {
        let kp_a = deterministic_keypair(7);
        let kp_b = deterministic_keypair(7);
        assert_eq!(kp_a.to_bytes(), kp_b.to_bytes());
    }

    #[test]
    fn deterministic_keypair_varies_by_seed() {
        let kp_a = deterministic_keypair(7);
        let kp_b = deterministic_keypair(8);
        assert_ne!(kp_a.to_bytes(), kp_b.to_bytes());
    }
}

//! Deterministic synthetic snapshot generators.
//!
//! Two flavors of account exist in the e2e tests:
//!
//! 1. **Claimable EOAs** — `(keypair, lamports)` pairs whose pubkey is a real ed25519
//!    public key; the test owns the secret half so it can build the §4.2 signature for the
//!    on-chain claim ix. System-owned, zero data → claimable per SPEC §3.1.
//! 2. **Token-program-owned accounts** — synthetic non-keypair accounts that carry the
//!    canonical 165-byte token-account size. Owner is a placeholder pubkey; we never
//!    interact with them on chain in the claim path, so the lack of a real Token program
//!    doesn't matter. Treasury per SPEC §3.1.
//!
//! Determinism: keypairs derive from a single seed byte via `Keypair::from_seed`, so the
//! same byte yields the same keypair across runs. Useful for reproducing failures.
//!
//! The JSON exporter emits the same shape `staccana_snapshot_fork::mock::MockSnapshot`
//! consumes — base58 pubkey strings, plain-number lamports / data_len fields. The
//! `e2e_genesis_to_claim.rs` test writes one of these to a tempfile and round-trips
//! through `MockSnapshot`.

use serde::{Deserialize, Serialize};
use solana_program::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, SeedDerivable, Signer};
use staccana_genesis::partition::{Account, SYSTEM_PROGRAM_ID};

/// Placeholder owner used for synthetic token-program-owned accounts. Real Token-program
/// id (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`) would work too — we only care that
/// the partition rule treats it as non-system.
const SYNTHETIC_TOKEN_OWNER: Pubkey = Pubkey::new_from_array([0x99; 32]);

/// One synthetic snapshot row. Implements [`staccana_genesis::Account`] so it can feed
/// directly into `build_genesis`. Carries the keypair when present so claim tests can
/// sign on behalf of the account.
///
/// Note: `Keypair` from `solana-sdk` does not implement `Clone` (it wraps an
/// `ed25519_dalek::Keypair` whose secret half intentionally lacks a `Clone` impl to avoid
/// accidental key duplication). We can't derive `Clone` on this struct as a result; tests
/// move or borrow `SyntheticSnapshotAccount`, never clone it. If a caller needs an
/// independent keypair handle, use [`deterministic_keypair`] with the same seed byte.
#[derive(Debug)]
pub struct SyntheticSnapshotAccount {
    pub pubkey: Pubkey,
    pub owner: Pubkey,
    pub data_len: usize,
    pub lamports: u64,
    /// Present iff this account is system-owned + zero-data — i.e. claimable. Treasury
    /// accounts have no keypair (we never need to sign on their behalf).
    pub keypair: Option<Keypair>,
}

impl Account for SyntheticSnapshotAccount {
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

impl Account for &SyntheticSnapshotAccount {
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

/// Derive a deterministic ed25519 keypair from a single seed byte. Matches the convention
/// used by `staccana-integration-tests`'s `deterministic_keypair`.
pub fn deterministic_keypair(seed_byte: u8) -> Keypair {
    let mut seed = [0u8; 32];
    seed[0] = seed_byte;
    for (i, b) in seed.iter_mut().enumerate().skip(1) {
        *b = seed_byte.wrapping_add(i as u8);
    }
    Keypair::from_seed(&seed).expect("32-byte seed accepted")
}

/// Build a claimable EOA with a real keypair. The pubkey is whatever `Keypair::pubkey()`
/// returns for the seeded keypair, so callers don't get to pick it directly — that's the
/// whole point of having a real signer.
pub fn synthetic_eoa_with_keypair(seed_byte: u8, lamports: u64) -> SyntheticSnapshotAccount {
    let keypair = deterministic_keypair(seed_byte);
    SyntheticSnapshotAccount {
        pubkey: keypair.pubkey(),
        owner: SYSTEM_PROGRAM_ID,
        data_len: 0,
        lamports,
        keypair: Some(keypair),
    }
}

/// Build a synthetic token-program-owned account. Always lands in the treasury partition.
/// `seed_byte` only varies the placeholder pubkey; no signing is ever required.
pub fn synthetic_token_account(seed_byte: u8, lamports: u64) -> SyntheticSnapshotAccount {
    SyntheticSnapshotAccount {
        pubkey: Pubkey::new_from_array([seed_byte; 32]),
        owner: SYNTHETIC_TOKEN_OWNER,
        data_len: 165,
        lamports,
        keypair: None,
    }
}

/// Build a deterministic snapshot mixing 3 claimable EOAs and 2 token-program-owned
/// accounts — the canonical e2e shape called out in the task brief.
///
/// Returned accounts are in insertion order; `build_genesis` is order-independent so the
/// caller doesn't need to sort.
pub fn mixed_synthetic_snapshot() -> Vec<SyntheticSnapshotAccount> {
    vec![
        synthetic_eoa_with_keypair(0x10, 1_000_000_000),
        synthetic_eoa_with_keypair(0x20, 2_000_000_000),
        synthetic_token_account(0x33, 2_039_280),
        synthetic_token_account(0x44, 2_039_280),
        synthetic_eoa_with_keypair(0x50, 500_000_000),
    ]
}

/// On-disk JSON shape consumed by `staccana_snapshot_fork::mock::MockSnapshot`. Matches
/// the `JsonAccount` type in that crate's `mock.rs` byte-for-byte.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct JsonAccountRow {
    pubkey: String,
    owner: String,
    data_len: u64,
    lamports: u64,
}

/// Serialize a synthetic snapshot to the JSON format `MockSnapshot` understands. Used by
/// `e2e_genesis_to_claim.rs` to round-trip through the snapshot-fork loader.
pub fn snapshot_to_json(accounts: &[SyntheticSnapshotAccount]) -> String {
    let rows: Vec<JsonAccountRow> = accounts
        .iter()
        .map(|a| JsonAccountRow {
            pubkey: bs58::encode(a.pubkey.to_bytes()).into_string(),
            owner: bs58::encode(a.owner.to_bytes()).into_string(),
            data_len: a.data_len as u64,
            lamports: a.lamports,
        })
        .collect();
    serde_json::to_string_pretty(&rows).expect("serialize synthetic snapshot")
}

#[cfg(test)]
mod tests {
    use super::*;
    use staccana_genesis::partition::{partition, Disposition};

    #[test]
    fn deterministic_keypair_is_reproducible() {
        let a = deterministic_keypair(7);
        let b = deterministic_keypair(7);
        assert_eq!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn deterministic_keypair_varies_by_seed() {
        let a = deterministic_keypair(7);
        let b = deterministic_keypair(8);
        assert_ne!(a.to_bytes(), b.to_bytes());
    }

    #[test]
    fn synthetic_eoa_partitions_as_claimable() {
        let a = synthetic_eoa_with_keypair(1, 100);
        assert_eq!(partition(&a), Disposition::Claimable);
        assert!(a.keypair.is_some());
    }

    #[test]
    fn synthetic_token_account_partitions_as_treasury() {
        let a = synthetic_token_account(2, 100);
        assert_eq!(partition(&a), Disposition::Treasury);
        assert!(a.keypair.is_none());
    }

    #[test]
    fn mixed_snapshot_split_is_three_two() {
        let snap = mixed_synthetic_snapshot();
        let claimable = snap
            .iter()
            .filter(|a| partition(*a) == Disposition::Claimable)
            .count();
        let treasury = snap
            .iter()
            .filter(|a| partition(*a) == Disposition::Treasury)
            .count();
        assert_eq!(claimable, 3);
        assert_eq!(treasury, 2);
    }

    #[test]
    fn snapshot_to_json_round_trips_via_serde() {
        let snap = mixed_synthetic_snapshot();
        let json = snapshot_to_json(&snap);
        let parsed: Vec<JsonAccountRow> = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed.len(), snap.len());
        // First entry is a claimable EOA with the expected lamports balance.
        assert_eq!(parsed[0].lamports, snap[0].lamports);
        assert_eq!(parsed[0].data_len, 0);
    }
}

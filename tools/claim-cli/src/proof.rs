//! Snapshot loading and Merkle inclusion-proof construction.
//!
//! The snapshot file is a JSON array of accounts in the same shape that
//! `tools/snapshot-fork` consumes:
//!
//! ```json
//! [
//!     {
//!         "pubkey":   "<base58 ed25519 pubkey>",
//!         "owner":    "<base58 owner program id>",
//!         "data_len": <u64>,
//!         "lamports": <u64>
//!     },
//!     ...
//! ]
//! ```
//!
//! The CLI partitions accounts into the **claimable** subset using the same rule as the
//! genesis builder (system-program owner, zero data) so the resulting Merkle tree is
//! byte-for-byte identical to the one whose root is embedded in the lazy-claim program.
//!
//! Inclusion proofs follow the same domain bytes (`0x00` for leaves, `0x01` for internal
//! nodes) and the same odd-leaf duplication strategy as `staccana_genesis::merkle::MerkleTree`.
//! The result is a list of sibling hashes plus a packed bitmap (`proof_flags`) where bit `i`
//! is `0` when the sibling is on the **left** at level `i` (i.e. the running hash is on the
//! right) and `1` when the sibling is on the **right**.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use solana_program::hash::{hashv, Hash};
use solana_program::pubkey::Pubkey;
use staccana_genesis::partition::SYSTEM_PROGRAM_ID;
use staccana_genesis::ClaimableLeaf;

/// Domain separation byte for leaf hashes — must match `staccana_genesis::merkle`.
pub const LEAF_DOMAIN: u8 = 0x00;
/// Domain separation byte for internal node hashes — must match `staccana_genesis::merkle`.
pub const NODE_DOMAIN: u8 = 0x01;

/// One row from the snapshot JSON file.
///
/// Pubkeys are base58-encoded strings on disk and decoded into [`Pubkey`] on load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotAccount {
    pub pubkey: String,
    pub owner: String,
    pub data_len: u64,
    pub lamports: u64,
}

/// A snapshot account that has been verified as claimable per the genesis rule.
///
/// The owner is implicitly `SYSTEM_PROGRAM_ID` and `data_len` is implicitly zero — both
/// invariants of the claimable partition — so the leaf only commits to pubkey + lamports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimableAccount {
    pub pubkey: Pubkey,
    pub lamports: u64,
}

impl ClaimableAccount {
    /// Convert to the [`ClaimableLeaf`] type the genesis crate uses for tree building.
    pub fn into_leaf(self) -> ClaimableLeaf {
        ClaimableLeaf {
            pubkey: self.pubkey,
            lamports: self.lamports,
        }
    }

    /// Hash this account into a Merkle leaf.
    pub fn leaf_hash(&self) -> Hash {
        hashv(&[
            &[LEAF_DOMAIN],
            self.pubkey.as_ref(),
            &self.lamports.to_le_bytes(),
        ])
    }
}

/// All errors the proof module can produce.
#[derive(Debug, thiserror::Error)]
pub enum ProofError {
    #[error("failed to read snapshot file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse snapshot JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid base58 pubkey {0:?}: {1}")]
    InvalidPubkey(String, bs58::decode::Error),
    #[error("base58 pubkey {0:?} did not decode to 32 bytes (got {1})")]
    PubkeyWrongLength(String, usize),
    #[error("target pubkey {0} is not present in the claimable partition")]
    TargetNotClaimable(Pubkey),
}

/// Read the snapshot JSON file from disk into the raw, untyped form.
pub fn load_snapshot_accounts(path: impl AsRef<Path>) -> Result<Vec<SnapshotAccount>, ProofError> {
    let raw = fs::read_to_string(path)?;
    let accounts: Vec<SnapshotAccount> = serde_json::from_str(&raw)?;
    Ok(accounts)
}

/// Apply the genesis claimable rule (system-owned + zero data) and decode the surviving
/// accounts' pubkeys.
///
/// Non-claimable rows are dropped silently — they're treasury, not part of the Merkle tree.
/// Rows whose pubkey or owner field can't be base58-decoded into 32 bytes return an error
/// rather than being silently skipped, since that almost always indicates a malformed
/// snapshot rather than an "expected" filter outcome.
pub fn partition_claimable(
    accounts: &[SnapshotAccount],
) -> Result<Vec<ClaimableAccount>, ProofError> {
    let mut out = Vec::new();
    for account in accounts {
        let owner = decode_pubkey(&account.owner)?;
        if owner != SYSTEM_PROGRAM_ID || account.data_len != 0 {
            continue;
        }
        let pubkey = decode_pubkey(&account.pubkey)?;
        out.push(ClaimableAccount {
            pubkey,
            lamports: account.lamports,
        });
    }
    Ok(out)
}

fn decode_pubkey(s: &str) -> Result<Pubkey, ProofError> {
    let bytes = bs58::decode(s)
        .into_vec()
        .map_err(|e| ProofError::InvalidPubkey(s.to_string(), e))?;
    if bytes.len() != 32 {
        return Err(ProofError::PubkeyWrongLength(s.to_string(), bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(Pubkey::new_from_array(arr))
}

/// A Merkle inclusion proof for one leaf in the claimable tree.
///
/// `proof_flags` is the packed bitmap described in `docs/SPEC.md` §4.1: bit `i` (LSB-first
/// within each byte, byte 0 first) controls level `i`. `0` ⇒ sibling on the left (running
/// hash on the right). `1` ⇒ sibling on the right (running hash on the left).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InclusionProof {
    /// The pubkey being proved.
    pub pubkey: Pubkey,
    /// Lamports balance from the snapshot for the pubkey.
    pub lamports: u64,
    /// Sibling hashes from leaf-level upward.
    pub proof: Vec<Hash>,
    /// Packed bit flags describing each sibling's side. Length is
    /// `(proof.len() + 7) / 8` bytes.
    pub proof_flags: Vec<u8>,
    /// The Merkle root reconstructed from this proof — sanity check for callers.
    pub root: Hash,
}

impl InclusionProof {
    /// Recompute the root by walking the proof from the leaf upward. Useful in tests and as
    /// a self-check before submitting a transaction.
    pub fn recomputed_root(&self) -> Hash {
        let mut running = hashv(&[
            &[LEAF_DOMAIN],
            self.pubkey.as_ref(),
            &self.lamports.to_le_bytes(),
        ]);
        for (i, sibling) in self.proof.iter().enumerate() {
            let sibling_on_right = bit_is_set(&self.proof_flags, i);
            running = if sibling_on_right {
                hashv(&[&[NODE_DOMAIN], running.as_ref(), sibling.as_ref()])
            } else {
                hashv(&[&[NODE_DOMAIN], sibling.as_ref(), running.as_ref()])
            };
        }
        running
    }
}

/// Build the inclusion proof for `target` against the claimable partition `accounts`.
///
/// Determinism: leaves are sorted by pubkey ascending — the same rule the genesis builder
/// uses — so a CLI run with the snapshot that produced the embedded root will reconstruct
/// that root exactly.
///
/// Returns [`ProofError::TargetNotClaimable`] if `target` doesn't appear in the claimable
/// partition. Note that the input `accounts` is expected to already be filtered through
/// [`partition_claimable`] — this function does not re-apply the partition rule.
pub fn build_inclusion_proof(
    accounts: &[ClaimableAccount],
    target: &Pubkey,
) -> Result<InclusionProof, ProofError> {
    // Sort ascending by pubkey for determinism (matches genesis MerkleTree::build).
    let mut sorted: Vec<ClaimableAccount> = accounts.to_vec();
    sorted.sort_by(|a, b| a.pubkey.cmp(&b.pubkey));

    let target_index = sorted
        .iter()
        .position(|a| &a.pubkey == target)
        .ok_or(ProofError::TargetNotClaimable(*target))?;
    let target_account = sorted[target_index].clone();

    // Layer 0 is the leaf hashes. Build all layers up to the root, recording the sibling at
    // each level along the path from `target_index` upward.
    let mut layer: Vec<Hash> = sorted.iter().map(ClaimableAccount::leaf_hash).collect();
    let mut idx = target_index;
    let mut proof: Vec<Hash> = Vec::new();
    let mut sibling_on_right_flags: Vec<bool> = Vec::new();

    while layer.len() > 1 {
        // For our `idx` at this level, the sibling is the other element of its pair:
        //   - if `idx` is even ⇒ sibling is `idx + 1` (sibling on the right).
        //   - if `idx` is odd  ⇒ sibling is `idx - 1` (sibling on the left).
        // Odd-leaf case: an even `idx` that has no `idx + 1` pairs with itself — its sibling
        // is its own hash and lives on the right.
        let (sibling_idx, sibling_on_right) = if idx % 2 == 0 {
            if idx + 1 < layer.len() {
                (idx + 1, true)
            } else {
                (idx, true)
            }
        } else {
            (idx - 1, false)
        };
        proof.push(layer[sibling_idx]);
        sibling_on_right_flags.push(sibling_on_right);

        // Promote the layer to the next level up.
        let mut next_layer: Vec<Hash> = Vec::with_capacity(layer.len() / 2 + 1);
        for chunk in layer.chunks(2) {
            let combined = if chunk.len() == 2 {
                hashv(&[&[NODE_DOMAIN], chunk[0].as_ref(), chunk[1].as_ref()])
            } else {
                hashv(&[&[NODE_DOMAIN], chunk[0].as_ref(), chunk[0].as_ref()])
            };
            next_layer.push(combined);
        }
        idx /= 2;
        layer = next_layer;
    }

    let root = layer[0];
    let proof_flags = pack_bits(&sibling_on_right_flags);

    Ok(InclusionProof {
        pubkey: target_account.pubkey,
        lamports: target_account.lamports,
        proof,
        proof_flags,
        root,
    })
}

/// Pack a list of bools into a little-endian-bit byte vector. Bit `i` lives at
/// `byte i/8`, bit position `i % 8` (LSB-first).
fn pack_bits(bits: &[bool]) -> Vec<u8> {
    let n_bytes = (bits.len() + 7) / 8;
    let mut out = vec![0u8; n_bytes];
    for (i, &b) in bits.iter().enumerate() {
        if b {
            out[i / 8] |= 1u8 << (i % 8);
        }
    }
    out
}

/// Read bit `i` from a packed bit vector produced by [`pack_bits`].
fn bit_is_set(bytes: &[u8], i: usize) -> bool {
    (bytes[i / 8] >> (i % 8)) & 1 == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use staccana_genesis::merkle::MerkleTree;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn acct(byte: u8, lamports: u64) -> ClaimableAccount {
        ClaimableAccount {
            pubkey: pk(byte),
            lamports,
        }
    }

    fn snapshot_row(pk_byte: u8, owner: Pubkey, data_len: u64, lamports: u64) -> SnapshotAccount {
        SnapshotAccount {
            pubkey: bs58::encode([pk_byte; 32]).into_string(),
            owner: bs58::encode(owner.to_bytes()).into_string(),
            data_len,
            lamports,
        }
    }

    #[test]
    fn pack_and_read_bits_round_trip() {
        let bits = vec![true, false, true, true, false, false, false, true, true];
        let packed = pack_bits(&bits);
        for (i, &b) in bits.iter().enumerate() {
            assert_eq!(bit_is_set(&packed, i), b, "bit {i}");
        }
        // 9 bits ⇒ 2 bytes.
        assert_eq!(packed.len(), 2);
    }

    #[test]
    fn partition_claimable_filters_by_owner_and_data() {
        let token_program = pk(99);
        let raw = vec![
            // claimable
            snapshot_row(1, SYSTEM_PROGRAM_ID, 0, 1_000),
            // dropped: non-system owner
            snapshot_row(2, token_program, 0, 2_000),
            // dropped: has data
            snapshot_row(3, SYSTEM_PROGRAM_ID, 165, 3_000),
            // claimable
            snapshot_row(4, SYSTEM_PROGRAM_ID, 0, 4_000),
        ];
        let claimable = partition_claimable(&raw).expect("partition");
        assert_eq!(claimable.len(), 2);
        assert_eq!(claimable[0].pubkey, pk(1));
        assert_eq!(claimable[0].lamports, 1_000);
        assert_eq!(claimable[1].pubkey, pk(4));
        assert_eq!(claimable[1].lamports, 4_000);
    }

    #[test]
    fn proof_root_matches_genesis_tree_root_two_leaves() {
        let accounts = vec![acct(1, 100), acct(2, 200)];
        let leaves: Vec<ClaimableLeaf> = accounts
            .iter()
            .cloned()
            .map(ClaimableAccount::into_leaf)
            .collect();
        let tree = MerkleTree::build(leaves);
        let proof = build_inclusion_proof(&accounts, &pk(1)).expect("build");
        assert_eq!(proof.root, tree.root.0);
        assert_eq!(proof.recomputed_root(), tree.root.0);
        assert_eq!(proof.proof.len(), 1);
        assert_eq!(proof.proof_flags, vec![0b0000_0001]);
    }

    #[test]
    fn proof_root_matches_for_each_leaf_in_balanced_tree() {
        // 4 leaves ⇒ tree of depth 2; proofs are 2 siblings each.
        let accounts = vec![acct(1, 100), acct(2, 200), acct(3, 300), acct(4, 400)];
        let leaves: Vec<ClaimableLeaf> = accounts
            .iter()
            .cloned()
            .map(ClaimableAccount::into_leaf)
            .collect();
        let tree = MerkleTree::build(leaves);
        for byte in 1u8..=4 {
            let proof = build_inclusion_proof(&accounts, &pk(byte)).expect("build");
            assert_eq!(
                proof.recomputed_root(),
                tree.root.0,
                "leaf {byte} recompute mismatch"
            );
            assert_eq!(proof.root, tree.root.0);
            assert_eq!(proof.proof.len(), 2);
            assert_eq!(proof.proof_flags.len(), 1);
        }
    }

    #[test]
    fn proof_root_matches_for_each_leaf_in_odd_tree() {
        // 3 leaves ⇒ odd leaf at index 2 pairs with itself.
        let accounts = vec![acct(1, 100), acct(2, 200), acct(3, 300)];
        let leaves: Vec<ClaimableLeaf> = accounts
            .iter()
            .cloned()
            .map(ClaimableAccount::into_leaf)
            .collect();
        let tree = MerkleTree::build(leaves);
        for byte in 1u8..=3 {
            let proof = build_inclusion_proof(&accounts, &pk(byte)).expect("build");
            assert_eq!(
                proof.recomputed_root(),
                tree.root.0,
                "odd-tree leaf {byte} recompute mismatch"
            );
            assert_eq!(proof.root, tree.root.0);
        }
    }

    #[test]
    fn proof_for_seven_leaves_spans_three_levels_with_odd_pairs() {
        // 7 leaves. Layer sizes go 7 → 4 → 2 → 1. Odd-leaf duplication kicks in at the
        // first reduction (the 7th leaf pairs with itself).
        let accounts: Vec<ClaimableAccount> =
            (1u8..=7).map(|b| acct(b, 100u64 * b as u64)).collect();
        let leaves: Vec<ClaimableLeaf> = accounts
            .iter()
            .cloned()
            .map(ClaimableAccount::into_leaf)
            .collect();
        let tree = MerkleTree::build(leaves);
        for byte in 1u8..=7 {
            let proof = build_inclusion_proof(&accounts, &pk(byte)).expect("build");
            assert_eq!(proof.proof.len(), 3, "leaf {byte}");
            assert_eq!(proof.proof_flags.len(), 1);
            assert_eq!(
                proof.recomputed_root(),
                tree.root.0,
                "leaf {byte} recompute mismatch"
            );
        }
    }

    #[test]
    fn proof_input_order_does_not_affect_output() {
        let mut a = vec![acct(1, 100), acct(2, 200), acct(3, 300), acct(4, 400)];
        let b = vec![acct(4, 400), acct(2, 200), acct(1, 100), acct(3, 300)];
        let proof_a = build_inclusion_proof(&a, &pk(2)).expect("a");
        let proof_b = build_inclusion_proof(&b, &pk(2)).expect("b");
        assert_eq!(proof_a, proof_b);

        // And after we sort `a` ourselves we still get the same result.
        a.sort_by(|x, y| x.pubkey.cmp(&y.pubkey));
        let proof_a_sorted = build_inclusion_proof(&a, &pk(2)).expect("a sorted");
        assert_eq!(proof_a_sorted, proof_a);
    }

    #[test]
    fn missing_target_returns_error() {
        let accounts = vec![acct(1, 100), acct(2, 200)];
        let err = build_inclusion_proof(&accounts, &pk(42)).unwrap_err();
        match err {
            ProofError::TargetNotClaimable(p) => assert_eq!(p, pk(42)),
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn proof_for_single_leaf_tree_is_empty() {
        // Single-leaf tree: proof has zero siblings, root is the leaf hash.
        let accounts = vec![acct(7, 500)];
        let leaves: Vec<ClaimableLeaf> = accounts
            .iter()
            .cloned()
            .map(ClaimableAccount::into_leaf)
            .collect();
        let tree = MerkleTree::build(leaves);
        let proof = build_inclusion_proof(&accounts, &pk(7)).expect("build");
        assert!(proof.proof.is_empty());
        assert!(proof.proof_flags.is_empty());
        assert_eq!(proof.root, tree.root.0);
        assert_eq!(proof.recomputed_root(), tree.root.0);
    }
}

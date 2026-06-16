//! On-chain Merkle proof verification against the embedded `claimable_root`.
//!
//! This is the inclusion-check counterpart to `staccana_genesis::merkle::MerkleTree::build`:
//! given a `(pubkey, lamports)` leaf and a sibling-hash path, recompute the root and
//! compare against the embedded value.
//!
//! ## Domain bytes — MUST stay in sync with `staccana_genesis::merkle`
//!
//! Both crates use SHA-256 (via `solana_program::hash::hashv`) with explicit domain bytes
//! to prevent leaf/internal-node second-preimage attacks:
//!
//! * `0x00` prefix → leaf hash
//! * `0x01` prefix → internal node hash
//!
//! Constants are duplicated here (rather than re-exported from staccana-genesis) so the
//! lazy-claim crate has no dependency on the genesis-builder crate. If the genesis side
//! ever changes either constant, this file MUST change to match — there is a unit test in
//! the integration tier that asserts root equality across both crates.
//!
//! ## Proof flag encoding
//!
//! `proof_flags` is a packed bitfield over `proof.len()` bits, LSB-first within each byte:
//!
//! * bit i = 0 → sibling is on the LEFT of the running hash; combine as `node(sibling, running)`
//! * bit i = 1 → sibling is on the RIGHT of the running hash; combine as `node(running, sibling)`
//!
//! Trailing bits in the final byte beyond `proof.len()` are ignored.

use solana_program::hash::{hashv, Hash};

const LEAF_DOMAIN: u8 = 0x00;
const NODE_DOMAIN: u8 = 0x01;

/// Compute the leaf hash for a `(pubkey, lamports)` claim. Identical to
/// `staccana_genesis::merkle::ClaimableLeaf::hash`.
pub fn leaf_hash(pubkey: &[u8; 32], lamports: u64) -> Hash {
    hashv(&[&[LEAF_DOMAIN], pubkey.as_ref(), &lamports.to_le_bytes()])
}

/// Combine two child hashes into a parent node hash. Identical to the internal
/// `node_hash` used in `staccana_genesis::merkle::MerkleTree::build`.
pub fn node_hash(left: &Hash, right: &Hash) -> Hash {
    hashv(&[&[NODE_DOMAIN], left.as_ref(), right.as_ref()])
}

/// Walk a Merkle proof from `leaf` up the tree. Returns the computed root.
///
/// `proof_flags` packs one bit per sibling, LSB-first. Bit `i` controls level `i`:
/// * `0` ⇒ sibling on left, running hash on right
/// * `1` ⇒ sibling on right, running hash on left
pub fn compute_root(leaf: Hash, proof: &[Hash], proof_flags: &[u8]) -> Hash {
    let mut running = leaf;
    for (i, sibling) in proof.iter().enumerate() {
        let byte = proof_flags[i / 8];
        let bit = (byte >> (i % 8)) & 1;
        running = if bit == 0 {
            node_hash(sibling, &running)
        } else {
            node_hash(&running, sibling)
        };
    }
    running
}

/// Verify a Merkle inclusion proof against `expected_root`.
///
/// Returns `true` iff the recomputed root equals `expected_root` byte-for-byte.
/// Caller is responsible for ensuring `proof_flags.len() >= ceil(proof.len() / 8)`.
pub fn verify_inclusion(
    leaf: Hash,
    proof: &[Hash],
    proof_flags: &[u8],
    expected_root: &Hash,
) -> bool {
    let computed = compute_root(leaf, proof, proof_flags);
    computed.as_ref() == expected_root.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    /// Build a tiny tree manually so each test case knows exactly what root to expect.
    /// Mirrors the algorithm in `staccana_genesis::merkle::MerkleTree::build`.
    fn build_root(leaves: &[([u8; 32], u64)]) -> Hash {
        if leaves.is_empty() {
            return Hash::default();
        }
        let mut sorted: Vec<([u8; 32], u64)> = leaves.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut layer: Vec<Hash> = sorted.iter().map(|(p, l)| leaf_hash(p, *l)).collect();
        while layer.len() > 1 {
            let mut next = Vec::with_capacity(layer.len() / 2 + 1);
            for chunk in layer.chunks(2) {
                let parent = if chunk.len() == 2 {
                    node_hash(&chunk[0], &chunk[1])
                } else {
                    node_hash(&chunk[0], &chunk[0])
                };
                next.push(parent);
            }
            layer = next;
        }
        layer[0]
    }

    /// Construct a proof + flag set for `target_idx` against the canonical
    /// `MerkleTree::build` algorithm. Returns `(proof, packed_flags)`.
    fn build_proof(leaves: &[([u8; 32], u64)], target_idx: usize) -> (Vec<Hash>, Vec<u8>) {
        let mut sorted: Vec<([u8; 32], u64)> = leaves.to_vec();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut layer: Vec<Hash> = sorted.iter().map(|(p, l)| leaf_hash(p, *l)).collect();

        let mut proof: Vec<Hash> = Vec::new();
        let mut flag_bits: Vec<u8> = Vec::new(); // 0 or 1 per level
        let mut idx = target_idx;

        while layer.len() > 1 {
            let pair_idx = idx ^ 1;
            let (sibling, sibling_on_right) = if pair_idx < layer.len() {
                // Standard pair.
                (layer[pair_idx], idx % 2 == 0)
            } else {
                // Odd-leaf duplication: our node pairs with itself.
                (layer[idx], true)
            };
            proof.push(sibling);
            flag_bits.push(if sibling_on_right { 1 } else { 0 });

            let mut next = Vec::with_capacity(layer.len() / 2 + 1);
            for chunk in layer.chunks(2) {
                let parent = if chunk.len() == 2 {
                    node_hash(&chunk[0], &chunk[1])
                } else {
                    node_hash(&chunk[0], &chunk[0])
                };
                next.push(parent);
            }
            idx /= 2;
            layer = next;
        }

        // Pack flags LSB-first.
        let mut packed = vec![0u8; (flag_bits.len() + 7) / 8];
        for (i, b) in flag_bits.iter().enumerate() {
            packed[i / 8] |= b << (i % 8);
        }
        (proof, packed)
    }

    #[test]
    fn verify_succeeds_for_valid_proof() {
        let leaves = vec![
            (pk(1), 100u64),
            (pk(2), 200u64),
            (pk(3), 300u64),
            (pk(4), 400u64),
        ];
        let root = build_root(&leaves);

        for (i, (p, l)) in leaves.iter().enumerate() {
            let (proof, flags) = build_proof(&leaves, sorted_index_of(&leaves, p));
            let leaf = leaf_hash(p, *l);
            assert!(
                verify_inclusion(leaf, &proof, &flags, &root),
                "leaf {} (lamports {}) failed verification",
                i,
                l
            );
        }
    }

    #[test]
    fn verify_fails_for_wrong_root() {
        let leaves = vec![(pk(1), 100u64), (pk(2), 200u64)];
        let (proof, flags) = build_proof(&leaves, 0);
        let leaf = leaf_hash(&pk(1), 100);
        let bogus_root = Hash::new_from_array([0xAB; 32]);
        assert!(!verify_inclusion(leaf, &proof, &flags, &bogus_root));
    }

    #[test]
    fn verify_fails_for_tampered_proof() {
        let leaves = vec![
            (pk(1), 100u64),
            (pk(2), 200u64),
            (pk(3), 300u64),
            (pk(4), 400u64),
        ];
        let root = build_root(&leaves);
        let (mut proof, flags) = build_proof(&leaves, 0);

        // Flip a byte in a sibling hash.
        let mut bytes = proof[0].to_bytes();
        bytes[0] ^= 0xFF;
        proof[0] = Hash::new_from_array(bytes);

        let leaf = leaf_hash(&pk(1), 100);
        assert!(!verify_inclusion(leaf, &proof, &flags, &root));
    }

    #[test]
    fn verify_fails_for_wrong_leaf_content() {
        let leaves = vec![(pk(1), 100u64), (pk(2), 200u64)];
        let root = build_root(&leaves);
        let (proof, flags) = build_proof(&leaves, 0);

        // Right pubkey, wrong lamports.
        let bad_leaf = leaf_hash(&pk(1), 999);
        assert!(!verify_inclusion(bad_leaf, &proof, &flags, &root));

        // Wrong pubkey, right lamports.
        let bad_leaf = leaf_hash(&pk(99), 100);
        assert!(!verify_inclusion(bad_leaf, &proof, &flags, &root));
    }

    #[test]
    fn verify_handles_odd_layer_with_self_pairing() {
        // 3 leaves → odd at the bottom. Verify proof for the lone third leaf, which pairs
        // with itself one level up.
        let leaves = vec![(pk(1), 100u64), (pk(2), 200u64), (pk(3), 300u64)];
        let root = build_root(&leaves);
        let (proof, flags) = build_proof(&leaves, 2);
        let leaf = leaf_hash(&pk(3), 300);
        assert!(verify_inclusion(leaf, &proof, &flags, &root));
    }

    #[test]
    fn verify_single_leaf_tree_has_empty_proof() {
        let leaves = vec![(pk(7), 42u64)];
        let root = build_root(&leaves);
        let leaf = leaf_hash(&pk(7), 42);
        assert!(verify_inclusion(leaf, &[], &[], &root));
        // Wrong lamports must fail even with empty proof.
        let bad_leaf = leaf_hash(&pk(7), 43);
        assert!(!verify_inclusion(bad_leaf, &[], &[], &root));
    }

    /// Helper: find the index of `pubkey` in the sorted-by-pubkey ordering of `leaves`.
    fn sorted_index_of(leaves: &[([u8; 32], u64)], pubkey: &[u8; 32]) -> usize {
        let mut sorted: Vec<[u8; 32]> = leaves.iter().map(|(p, _)| *p).collect();
        sorted.sort();
        sorted.iter().position(|p| p == pubkey).unwrap()
    }
}

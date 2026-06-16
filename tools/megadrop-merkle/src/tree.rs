//! Merkle tree builder + per-leaf inclusion proofs.
//!
//! Delegates to `staccana_genesis::merkle::MerkleTree::build` for the root computation
//! so the bytes match what the on-chain verifier in
//! `programs/megadrop/src/merkle.rs` expects (same domain separators, same odd-leaf
//! duplication, same SHA-256). Building the proofs is local — the genesis crate only
//! exposes the root.
//!
//! ## Leaf format
//!
//! `leaf_hash = sha256(0x00 || holder_pubkey || lamports_le)` — matches
//! `programs/megadrop/src/merkle.rs::leaf_hash` byte-for-byte. We re-implement here
//! rather than depending on the megadrop program crate because the program crate is
//! `cdylib` + Anchor and pulling it in would inflate the build graph.

use solana_program::hash::{hashv, Hash};
use solana_sdk::pubkey::Pubkey;
use staccana_genesis::merkle::{ClaimableLeaf, MerkleTree, NODE_DOMAIN};

use crate::allocate::HolderAllocation;

/// Domain-separation byte for leaf hashes — must match
/// `programs/megadrop/src/merkle.rs::LEAF_DOMAIN` and
/// `staccana_genesis::merkle::LEAF_DOMAIN`.
pub const LEAF_DOMAIN: u8 = 0x00;

/// Output of [`build_tree`]: root + leaf hashes (sorted by pubkey ascending) + the
/// per-leaf proof material so the caller can serialize all three output files in one
/// pass.
#[derive(Debug, Clone)]
pub struct BuiltTree {
    pub root: Hash,
    /// Leaves in canonical (pubkey-sorted) order. Each entry: (owner, lamports,
    /// leaf_hash).
    pub leaves: Vec<(Pubkey, u64, Hash)>,
    /// Per-leaf inclusion proofs (sibling hashes leaf-level upward). Indexed
    /// alongside `leaves`.
    pub proofs: Vec<Vec<Hash>>,
    /// Per-leaf packed sibling-side bit flags. Bit `i` controls level `i` of the
    /// corresponding proof: 0 ⇒ sibling on left, 1 ⇒ sibling on right. Matches
    /// `programs/megadrop/src/merkle.rs::compute_root` semantics.
    pub proof_flags: Vec<Vec<u8>>,
}

/// Build a Merkle tree from per-holder allocations. Holders with `lamports == 0` are
/// included so the tree commits to the *full* eligible cohort (an audit reading the
/// tree can confirm "this owner was eligible for zero" rather than wonder if they
/// were dropped accidentally).
pub fn build_tree(allocations: &[HolderAllocation]) -> BuiltTree {
    if allocations.is_empty() {
        return BuiltTree {
            root: Hash::default(),
            leaves: Vec::new(),
            proofs: Vec::new(),
            proof_flags: Vec::new(),
        };
    }

    // Canonicalize: sort by owner pubkey ascending. The genesis crate's
    // `MerkleTree::build` does the same internally so we get a matching root, but
    // we sort here so the proofs we build below see the same indices.
    let mut sorted: Vec<HolderAllocation> = allocations.to_vec();
    sorted.sort_by(|a, b| a.owner.cmp(&b.owner));

    let leaf_data: Vec<(Pubkey, u64)> = sorted.iter().map(|a| (a.owner, a.lamports)).collect();
    let leaf_hashes: Vec<Hash> = leaf_data
        .iter()
        .map(|(p, l)| {
            // Match `programs/megadrop/src/merkle.rs::leaf_hash` byte-for-byte.
            hashv(&[&[LEAF_DOMAIN], p.as_ref(), &l.to_le_bytes()])
        })
        .collect();

    // Use the genesis builder for the root — guarantees byte-equality with what the
    // on-chain verifier produces.
    let claimable_leaves: Vec<ClaimableLeaf> = leaf_data
        .iter()
        .map(|(p, l)| ClaimableLeaf {
            pubkey: *p,
            lamports: *l,
        })
        .collect();
    let tree = MerkleTree::build(claimable_leaves);

    // Build per-leaf proofs locally. Standard binary tree, odd-leaf duplication.
    let mut proofs: Vec<Vec<Hash>> = Vec::with_capacity(leaf_hashes.len());
    let mut flags: Vec<Vec<u8>> = Vec::with_capacity(leaf_hashes.len());
    for target_idx in 0..leaf_hashes.len() {
        let (proof, packed) = build_proof(&leaf_hashes, target_idx);
        proofs.push(proof);
        flags.push(packed);
    }

    let leaves_out: Vec<(Pubkey, u64, Hash)> = sorted
        .into_iter()
        .zip(leaf_hashes)
        .map(|(a, h)| (a.owner, a.lamports, h))
        .collect();

    BuiltTree {
        root: tree.root.0,
        leaves: leaves_out,
        proofs,
        proof_flags: flags,
    }
}

/// Internal: compute one leaf's inclusion proof. Returns `(proof, packed_flags)`
/// matching the on-chain verifier's expected format (see
/// `programs/megadrop/src/merkle.rs::compute_root`).
fn build_proof(leaf_hashes: &[Hash], target_idx: usize) -> (Vec<Hash>, Vec<u8>) {
    if leaf_hashes.len() <= 1 {
        return (Vec::new(), Vec::new());
    }
    let mut layer: Vec<Hash> = leaf_hashes.to_vec();
    let mut idx = target_idx;
    let mut proof: Vec<Hash> = Vec::new();
    let mut flag_bits: Vec<u8> = Vec::new();

    while layer.len() > 1 {
        let pair_idx = idx ^ 1;
        let (sibling, sibling_on_right) = if pair_idx < layer.len() {
            (layer[pair_idx], idx % 2 == 0)
        } else {
            // Odd-leaf duplication.
            (layer[idx], true)
        };
        proof.push(sibling);
        flag_bits.push(if sibling_on_right { 1 } else { 0 });

        let mut next = Vec::with_capacity(layer.len() / 2 + 1);
        for chunk in layer.chunks(2) {
            let parent = if chunk.len() == 2 {
                hashv(&[&[NODE_DOMAIN], chunk[0].as_ref(), chunk[1].as_ref()])
            } else {
                hashv(&[&[NODE_DOMAIN], chunk[0].as_ref(), chunk[0].as_ref()])
            };
            next.push(parent);
        }
        idx /= 2;
        layer = next;
    }

    let mut packed = vec![0u8; flag_bits.len().div_ceil(8)];
    for (i, b) in flag_bits.iter().enumerate() {
        packed[i / 8] |= b << (i % 8);
    }
    (proof, packed)
}

/// Verify a previously-built proof — useful for the integration test that picks a
/// random leaf and confirms its proof recomputes the root.
pub fn verify_proof(leaf: Hash, proof: &[Hash], flags: &[u8], expected_root: &Hash) -> bool {
    let mut running = leaf;
    for (i, sibling) in proof.iter().enumerate() {
        let bit = (flags[i / 8] >> (i % 8)) & 1;
        running = if bit == 0 {
            hashv(&[&[NODE_DOMAIN], sibling.as_ref(), running.as_ref()])
        } else {
            hashv(&[&[NODE_DOMAIN], running.as_ref(), sibling.as_ref()])
        };
    }
    running.as_ref() == expected_root.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocate::{HolderAllocation, HolderContributions};

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn alloc(owner: Pubkey, lamports: u64) -> HolderAllocation {
        HolderAllocation {
            owner,
            lamports,
            contributions: HolderContributions {
                nft_count: 0,
                token_balance: 0,
            },
        }
    }

    #[test]
    fn empty_input_yields_default_root() {
        let t = build_tree(&[]);
        assert_eq!(t.root, Hash::default());
        assert!(t.leaves.is_empty());
        assert!(t.proofs.is_empty());
    }

    #[test]
    fn single_holder_root_is_leaf_hash() {
        let allocs = vec![alloc(pk(1), 1_000)];
        let t = build_tree(&allocs);
        let expected = hashv(&[&[LEAF_DOMAIN], pk(1).as_ref(), &1_000u64.to_le_bytes()]);
        assert_eq!(t.root, expected);
        assert_eq!(t.leaves.len(), 1);
        // Proof for a single-leaf tree is empty; verifier handles that.
        assert!(t.proofs[0].is_empty());
    }

    #[test]
    fn root_is_deterministic_under_input_reordering() {
        let mut a = vec![alloc(pk(1), 100), alloc(pk(2), 200), alloc(pk(3), 300)];
        let mut b = a.clone();
        b.reverse();
        let t1 = build_tree(&a);
        a.reverse();
        let t2 = build_tree(&b);
        assert_eq!(t1.root, t2.root);
    }

    #[test]
    fn proofs_verify_for_every_leaf() {
        let allocs = vec![
            alloc(pk(1), 100),
            alloc(pk(2), 200),
            alloc(pk(3), 300),
            alloc(pk(4), 400),
        ];
        let t = build_tree(&allocs);
        for ((_owner, _lamports, leaf_hash), (proof, flags)) in t
            .leaves
            .iter()
            .zip(t.proofs.iter().zip(t.proof_flags.iter()))
        {
            assert!(verify_proof(*leaf_hash, proof, flags, &t.root));
        }
    }

    #[test]
    fn proofs_verify_for_odd_leaf_count() {
        let allocs = vec![
            alloc(pk(1), 100),
            alloc(pk(2), 200),
            alloc(pk(3), 300),
        ];
        let t = build_tree(&allocs);
        for ((_owner, _lamports, leaf_hash), (proof, flags)) in t
            .leaves
            .iter()
            .zip(t.proofs.iter().zip(t.proof_flags.iter()))
        {
            assert!(verify_proof(*leaf_hash, proof, flags, &t.root));
        }
    }

    /// Integration: build a 100-leaf tree from synthetic allocations, then verify a
    /// randomly-chosen leaf's proof against the root. Mirrors the end-to-end claim
    /// path: snapshot tool → operator → frontend → on-chain verifier.
    #[test]
    fn synthetic_100_holder_random_proof_verifies() {
        // 100 holders, monotonically increasing pubkeys, varied lamports.
        let allocs: Vec<HolderAllocation> = (1u8..=100)
            .map(|i| alloc(pk(i), (i as u64) * 1_000_000_000))
            .collect();
        let t = build_tree(&allocs);
        // "Randomly chosen" via deterministic-but-arbitrary index. Use 47.
        let chosen = 47;
        let (leaf_hash, proof, flags) = (
            t.leaves[chosen].2,
            &t.proofs[chosen],
            &t.proof_flags[chosen],
        );
        assert!(verify_proof(leaf_hash, proof, flags, &t.root));

        // Negative: tamper a single bit of the leaf hash → must fail.
        let mut bytes = leaf_hash.to_bytes();
        bytes[0] ^= 0x01;
        let bad_leaf = Hash::new_from_array(bytes);
        assert!(!verify_proof(bad_leaf, proof, flags, &t.root));
    }
}

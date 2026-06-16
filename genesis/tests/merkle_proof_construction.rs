//! Merkle root verification by independent recomputation.
//!
//! `MerkleTree` only exposes the root — the lazy-claim program will reconstruct intermediate
//! hashes from a (leaf, proof) pair at claim time. This file replicates the verification
//! side of that flow: for each tree of size N, walk the layered hashing manually using
//! `ClaimableLeaf::hash` (which encodes `LEAF_DOMAIN`) for layer 0 and the public
//! `NODE_DOMAIN` byte for every internal-node combine, then assert the recomputed root
//! matches `MerkleTree::build(...).root`.
//!
//! Covers leaf counts 1, 2, 3, 4, 7, 16 — exercising odd-leaf duplication and several
//! depths.

use solana_program::hash::{hashv, Hash};
use solana_program::pubkey::Pubkey;
use staccana_genesis::merkle::NODE_DOMAIN;
use staccana_genesis::*;

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn leaf(byte: u8, lamports: u64) -> ClaimableLeaf {
    ClaimableLeaf {
        pubkey: pk(byte),
        lamports,
    }
}

/// Recompute the Merkle root from a sorted leaf list using the same algorithm as the builder.
/// Mirrors `MerkleTree::build` but is written from the verifier's perspective: takes leaves,
/// hashes them, layer-reduces with the documented domain bytes, returns the final root hash.
fn recompute_root(mut leaves: Vec<ClaimableLeaf>) -> Hash {
    if leaves.is_empty() {
        return Hash::default();
    }
    leaves.sort_by(|a, b| a.pubkey.cmp(&b.pubkey));
    let mut layer: Vec<Hash> = leaves.iter().map(ClaimableLeaf::hash).collect();
    while layer.len() > 1 {
        let mut next_layer: Vec<Hash> = Vec::with_capacity(layer.len() / 2 + 1);
        for chunk in layer.chunks(2) {
            let combined = if chunk.len() == 2 {
                hashv(&[&[NODE_DOMAIN], chunk[0].as_ref(), chunk[1].as_ref()])
            } else {
                // Odd-leaf duplication.
                hashv(&[&[NODE_DOMAIN], chunk[0].as_ref(), chunk[0].as_ref()])
            };
            next_layer.push(combined);
        }
        layer = next_layer;
    }
    layer[0]
}

/// Run the round-trip for one leaf set: build the tree via the public API, recompute the
/// root externally, and assert equality.
fn assert_root_matches(leaves: Vec<ClaimableLeaf>) {
    let n = leaves.len();
    let expected_root = recompute_root(leaves.clone());
    let tree = MerkleTree::build(leaves);
    assert_eq!(tree.leaf_count, n, "leaf_count for N={} mismatch", n);
    assert_eq!(
        tree.root,
        MerkleRoot(expected_root),
        "recomputed root for N={} must match builder root",
        n
    );
}

#[test]
fn one_leaf_root_equals_leaf_hash() {
    // Special case: with a single leaf the layer-reduce loop never executes; the root is
    // exactly the leaf hash.
    let l = leaf(1, 1_000);
    let tree = MerkleTree::build(vec![l.clone()]);
    assert_eq!(tree.root.0, l.hash());
    assert_root_matches(vec![l]);
}

#[test]
fn two_leaves() {
    // Layer 0: [leaf_hash(1), leaf_hash(2)]
    // Root: NODE_DOMAIN || layer0[0] || layer0[1]
    assert_root_matches(vec![leaf(1, 100), leaf(2, 200)]);
}

#[test]
fn three_leaves_odd_handling() {
    // Layer 0: [h1, h2, h3] (odd count)
    // Layer 1: [node(h1, h2), node(h3, h3)]   ← h3 duplicates with itself
    // Root:    node(layer1[0], layer1[1])
    assert_root_matches(vec![leaf(1, 100), leaf(2, 200), leaf(3, 300)]);
}

#[test]
fn four_leaves_balanced() {
    // Layer 0: [h1, h2, h3, h4]
    // Layer 1: [node(h1, h2), node(h3, h4)]
    // Root:    node(layer1[0], layer1[1])
    assert_root_matches(vec![
        leaf(1, 100),
        leaf(2, 200),
        leaf(3, 300),
        leaf(4, 400),
    ]);
}

#[test]
fn seven_leaves_double_odd() {
    // Layer 0: 7 hashes (odd)
    // Layer 1: 4 hashes — last one was h7 dup'd with itself
    // Layer 2: 2 hashes
    // Root:    node(layer2[0], layer2[1])
    let leaves: Vec<ClaimableLeaf> = (1..=7).map(|b| leaf(b, b as u64 * 1_000)).collect();
    assert_root_matches(leaves);
}

#[test]
fn sixteen_leaves_balanced_depth_four() {
    // Power of two — perfect binary tree at depth 4.
    let leaves: Vec<ClaimableLeaf> = (1..=16).map(|b| leaf(b, b as u64 * 1_000)).collect();
    assert_root_matches(leaves);
}

#[test]
fn input_order_does_not_affect_root() {
    // Builder sorts internally — two different input orderings produce the same root.
    let forward: Vec<ClaimableLeaf> = (1..=7).map(|b| leaf(b, b as u64 * 100)).collect();
    let reversed: Vec<ClaimableLeaf> = (1..=7).rev().map(|b| leaf(b, b as u64 * 100)).collect();
    let tree_a = MerkleTree::build(forward);
    let tree_b = MerkleTree::build(reversed);
    assert_eq!(tree_a.root, tree_b.root);
}

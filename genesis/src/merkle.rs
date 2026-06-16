//! Merkle tree construction for the claimable partition.
//!
//! Genesis embeds a single root hash; the lazy-claim program verifies inclusion proofs
//! against it. Leaves are sorted by pubkey ascending for determinism — any genesis builder
//! reading the same snapshot produces the same root byte-for-byte.
//!
//! Hash function: `solana_program::hash::hashv` (SHA-256). Domain separation byte `0x00`
//! for leaves and `0x01` for internal nodes prevents second-preimage attacks where a leaf
//! could be misinterpreted as a node hash or vice versa.
//!
//! Odd-leaf handling: the last hash in a layer of odd length is duplicated to itself when
//! computing the next layer up — standard pattern.

use serde::{Deserialize, Serialize};
use solana_program::hash::{hashv, Hash};
use solana_program::pubkey::Pubkey;

/// Domain-separation byte prepended to leaf preimages before hashing.
/// Public so downstream crates (lazy-claim program, claim-cli) can hash leaves identically
/// without redeclaring the constant.
pub const LEAF_DOMAIN: u8 = 0x00;
/// Domain-separation byte prepended to internal node preimages before hashing.
/// Public for the same reason as [`LEAF_DOMAIN`].
pub const NODE_DOMAIN: u8 = 0x01;

/// A single claimable account from the snapshot.
///
/// Owner is implicitly the System program and `data_len` is implicitly zero — both are
/// invariants of the partition rule, so the leaf only commits to pubkey + lamports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimableLeaf {
    pub pubkey: Pubkey,
    pub lamports: u64,
}

impl ClaimableLeaf {
    pub fn hash(&self) -> Hash {
        hashv(&[
            &[LEAF_DOMAIN],
            self.pubkey.as_ref(),
            &self.lamports.to_le_bytes(),
        ])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerkleRoot(pub Hash);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerkleTree {
    pub root: MerkleRoot,
    pub leaf_count: usize,
}

impl MerkleTree {
    /// Build a Merkle tree from leaves. Sorts by pubkey ascending for determinism.
    pub fn build(leaves: Vec<ClaimableLeaf>) -> Self {
        let with_layers = MerkleTreeWithLayers::build(leaves);
        Self {
            root: with_layers.root,
            leaf_count: with_layers.leaf_count,
        }
    }
}

/// A Merkle tree that retains every internal layer so per-leaf proofs can be
/// generated on demand.
///
/// Memory cost: the full tree holds roughly `2 * leaf_count` 32-byte hashes.
/// For the mainnet snapshot's ~86M claimable leaves that's ~5.5 GB resident,
/// which is fine on the snapshot host (124 GB RAM) but unsuitable for tiny
/// validators. Use [`MerkleTree::build`] when you only need the root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerkleTreeWithLayers {
    pub root: MerkleRoot,
    pub leaf_count: usize,
    /// Sorted leaves (by pubkey ascending), parallel to layer 0.
    pub leaves: Vec<ClaimableLeaf>,
    /// One entry per tree level, bottom-up. `layers[0]` are the leaf hashes,
    /// `layers[layers.len()-1]` is `[root]`.
    pub layers: Vec<Vec<Hash>>,
}

impl MerkleTreeWithLayers {
    /// Build the tree, retaining every layer for proof generation.
    pub fn build(mut leaves: Vec<ClaimableLeaf>) -> Self {
        if leaves.is_empty() {
            return Self {
                root: MerkleRoot(Hash::default()),
                leaf_count: 0,
                leaves: Vec::new(),
                layers: Vec::new(),
            };
        }

        leaves.sort_by(|a, b| a.pubkey.cmp(&b.pubkey));
        let leaf_count = leaves.len();

        let leaf_layer: Vec<Hash> = leaves.iter().map(ClaimableLeaf::hash).collect();
        let mut layers: Vec<Vec<Hash>> = vec![leaf_layer];

        while layers.last().expect("non-empty").len() > 1 {
            let layer = layers.last().expect("non-empty");
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
            layers.push(next_layer);
        }

        let root = MerkleRoot(layers.last().expect("non-empty")[0]);

        Self {
            root,
            leaf_count,
            leaves,
            layers,
        }
    }

    /// Inclusion proof for the leaf at sorted-index `leaf_index`.
    ///
    /// Returns the list of sibling hashes from leaf level up to (but not
    /// including) the root. To verify, hash the leaf with [`LEAF_DOMAIN`],
    /// then iteratively combine with each sibling using [`NODE_DOMAIN`] —
    /// the order is `(left, right)` based on the position bit (low bit of the
    /// running index).
    ///
    /// Odd-leaf nodes self-pair, matching the tree construction: when a node
    /// has no sibling at a given layer, the proof step uses that node itself.
    pub fn proof(&self, leaf_index: usize) -> Vec<Hash> {
        assert!(
            leaf_index < self.leaf_count,
            "leaf_index {leaf_index} out of bounds for {} leaves",
            self.leaf_count
        );
        let mut proof = Vec::with_capacity(self.layers.len().saturating_sub(1));
        let mut idx = leaf_index;
        // Walk every layer except the root.
        for layer in &self.layers[..self.layers.len() - 1] {
            let sibling_idx = if idx % 2 == 0 { idx + 1 } else { idx - 1 };
            let sibling = if sibling_idx < layer.len() {
                layer[sibling_idx]
            } else {
                // Odd node at this layer: self-pair.
                layer[idx]
            };
            proof.push(sibling);
            idx /= 2;
        }
        proof
    }

    /// Verify a previously-generated proof. Used by tests and downstream
    /// crates to sanity-check shard emission.
    pub fn verify(
        leaf: &ClaimableLeaf,
        leaf_index: usize,
        proof: &[Hash],
        root: &MerkleRoot,
    ) -> bool {
        let mut h = leaf.hash();
        let mut idx = leaf_index;
        for sibling in proof {
            h = if idx % 2 == 0 {
                hashv(&[&[NODE_DOMAIN], h.as_ref(), sibling.as_ref()])
            } else {
                hashv(&[&[NODE_DOMAIN], sibling.as_ref(), h.as_ref()])
            };
            idx /= 2;
        }
        h == root.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn leaf(byte: u8, lamports: u64) -> ClaimableLeaf {
        ClaimableLeaf {
            pubkey: pk(byte),
            lamports,
        }
    }

    #[test]
    fn empty_tree_has_default_root() {
        let tree = MerkleTree::build(vec![]);
        assert_eq!(tree.root, MerkleRoot(Hash::default()));
        assert_eq!(tree.leaf_count, 0);
    }

    #[test]
    fn single_leaf_root_is_leaf_hash() {
        let l = leaf(1, 1_000);
        let tree = MerkleTree::build(vec![l.clone()]);
        assert_eq!(tree.root.0, l.hash());
        assert_eq!(tree.leaf_count, 1);
    }

    #[test]
    fn determinism_under_input_reordering() {
        let leaves_a = vec![leaf(1, 100), leaf(2, 200), leaf(3, 300), leaf(4, 400)];
        let leaves_b = vec![leaf(4, 400), leaf(2, 200), leaf(1, 100), leaf(3, 300)];

        let tree_a = MerkleTree::build(leaves_a);
        let tree_b = MerkleTree::build(leaves_b);

        assert_eq!(tree_a.root, tree_b.root);
        assert_eq!(tree_a.leaf_count, tree_b.leaf_count);
    }

    #[test]
    fn odd_leaf_count_is_handled() {
        // 3 leaves: layer 1 = [h1, h2, h3], layer 2 = [hash(h1,h2), hash(h3,h3)],
        // root = hash(layer2[0], layer2[1]).
        let leaves = vec![leaf(1, 100), leaf(2, 200), leaf(3, 300)];
        let tree = MerkleTree::build(leaves);
        assert_eq!(tree.leaf_count, 3);
        assert_ne!(tree.root, MerkleRoot(Hash::default()));
    }

    #[test]
    fn different_lamports_change_root() {
        let tree_a = MerkleTree::build(vec![leaf(1, 100), leaf(2, 200)]);
        let tree_b = MerkleTree::build(vec![leaf(1, 100), leaf(2, 201)]);
        assert_ne!(tree_a.root, tree_b.root);
    }

    #[test]
    fn different_pubkeys_change_root() {
        let tree_a = MerkleTree::build(vec![leaf(1, 100), leaf(2, 200)]);
        let tree_b = MerkleTree::build(vec![leaf(1, 100), leaf(3, 200)]);
        assert_ne!(tree_a.root, tree_b.root);
    }

    #[test]
    fn with_layers_root_matches_compact_build() {
        let leaves = vec![leaf(1, 10), leaf(2, 20), leaf(3, 30), leaf(4, 40)];
        let compact = MerkleTree::build(leaves.clone());
        let with_layers = MerkleTreeWithLayers::build(leaves);
        assert_eq!(compact.root, with_layers.root);
        assert_eq!(compact.leaf_count, with_layers.leaf_count);
    }

    #[test]
    fn proofs_verify_for_every_leaf_even_count() {
        let leaves = vec![leaf(1, 10), leaf(2, 20), leaf(3, 30), leaf(4, 40)];
        let tree = MerkleTreeWithLayers::build(leaves.clone());
        // Use the sorted leaves from inside the tree so indices line up.
        for (i, l) in tree.leaves.iter().enumerate() {
            let proof = tree.proof(i);
            assert!(
                MerkleTreeWithLayers::verify(l, i, &proof, &tree.root),
                "leaf {i} failed to verify"
            );
        }
    }

    #[test]
    fn proofs_verify_for_every_leaf_odd_count() {
        let leaves = vec![leaf(1, 10), leaf(2, 20), leaf(3, 30)];
        let tree = MerkleTreeWithLayers::build(leaves);
        for (i, l) in tree.leaves.iter().enumerate() {
            let proof = tree.proof(i);
            assert!(
                MerkleTreeWithLayers::verify(l, i, &proof, &tree.root),
                "leaf {i} failed to verify"
            );
        }
    }

    #[test]
    fn proofs_verify_for_single_leaf() {
        let leaves = vec![leaf(7, 777)];
        let tree = MerkleTreeWithLayers::build(leaves);
        let proof = tree.proof(0);
        assert!(proof.is_empty(), "single-leaf proof should be empty");
        assert!(MerkleTreeWithLayers::verify(
            &tree.leaves[0],
            0,
            &proof,
            &tree.root
        ));
    }

    #[test]
    fn proof_for_larger_tree_verifies() {
        // 17 leaves: forces multiple layers with odd-leaf duplication on more
        // than one level (17 -> 9 -> 5 -> 3 -> 2 -> 1).
        let leaves: Vec<_> = (1u8..=17).map(|b| leaf(b, b as u64 * 100)).collect();
        let tree = MerkleTreeWithLayers::build(leaves);
        for (i, l) in tree.leaves.iter().enumerate() {
            let proof = tree.proof(i);
            assert!(
                MerkleTreeWithLayers::verify(l, i, &proof, &tree.root),
                "leaf {i} failed to verify (proof len = {})",
                proof.len()
            );
        }
    }

    #[test]
    fn proof_does_not_verify_against_wrong_root() {
        let leaves = vec![leaf(1, 10), leaf(2, 20), leaf(3, 30)];
        let tree = MerkleTreeWithLayers::build(leaves);
        let proof = tree.proof(0);
        let wrong = MerkleRoot(Hash::default());
        assert!(!MerkleTreeWithLayers::verify(
            &tree.leaves[0],
            0,
            &proof,
            &wrong
        ));
    }
}

//! Merkle tree consistency across crates.
//!
//! Three independent implementations of the Merkle algorithm currently exist:
//!
//! * `staccana_genesis::MerkleTree` — used by the genesis builder to produce the embedded
//!   `claimable_root` (SPEC §3.4).
//! * `staccana_lazy_claim::merkle::{leaf_hash, node_hash, verify_inclusion}` — used by
//!   the on-chain program to verify proofs (SPEC §4.3).
//! * `staccana_claim_cli::build_inclusion_proof` — used by the off-chain CLI to construct
//!   the proof a claimant submits to the program.
//!
//! All three MUST agree on:
//! 1. Domain bytes (`LEAF_DOMAIN = 0x00`, `NODE_DOMAIN = 0x01`).
//! 2. Leaf sort order (ascending by pubkey).
//! 3. Odd-leaf duplication (the trailing odd leaf pairs with itself).
//! 4. Proof-flag encoding (LSB-first, bit i controls level i).
//!
//! The tests below cross-check these directly. Disagreement here is a consensus break —
//! a claimant could construct a "valid" proof that the on-chain program rejects, or
//! worse, vice-versa.

use solana_program::hash::{hashv, Hash};
use solana_program::pubkey::Pubkey;
use staccana_claim_cli::{build_inclusion_proof, ClaimableAccount};
use staccana_genesis::{ClaimableLeaf, MerkleTree, LEAF_DOMAIN, NODE_DOMAIN};
use staccana_integration_tests::pk;
use staccana_lazy_claim::merkle::{leaf_hash, node_hash, verify_inclusion};

fn leaves_for_test(count: usize) -> Vec<ClaimableLeaf> {
    (1..=count)
        .map(|i| ClaimableLeaf {
            pubkey: pk(i as u8),
            lamports: 100u64 * i as u64,
        })
        .collect()
}

fn cli_accounts_for_test(count: usize) -> Vec<ClaimableAccount> {
    (1..=count)
        .map(|i| ClaimableAccount {
            pubkey: pk(i as u8),
            lamports: 100u64 * i as u64,
        })
        .collect()
}

#[test]
fn leaf_hash_agrees_across_genesis_and_lazy_claim() {
    // Both crates must produce the same 32 bytes for the same (pubkey, lamports).
    let pubkey_bytes = [0xABu8; 32];
    let lamports = 1_234_567_890u64;

    let genesis_leaf_hash = ClaimableLeaf {
        pubkey: Pubkey::new_from_array(pubkey_bytes),
        lamports,
    }
    .hash();
    let lazy_claim_leaf_hash = leaf_hash(&pubkey_bytes, lamports);
    assert_eq!(genesis_leaf_hash, lazy_claim_leaf_hash);

    // And both must equal the SPEC §3.2 literal: SHA-256(0x00 || pubkey || lamports_le).
    let expected = hashv(&[&[LEAF_DOMAIN], &pubkey_bytes, &lamports.to_le_bytes()]);
    assert_eq!(genesis_leaf_hash, expected);
}

#[test]
fn node_hash_constants_match_spec_3_3() {
    // Pin both domain bytes and confirm they're both used as expected.
    assert_eq!(LEAF_DOMAIN, 0x00);
    assert_eq!(NODE_DOMAIN, 0x01);

    let l = Hash::new_from_array([0x11u8; 32]);
    let r = Hash::new_from_array([0x22u8; 32]);
    let combined = node_hash(&l, &r);
    let expected = hashv(&[&[NODE_DOMAIN], l.as_ref(), r.as_ref()]);
    assert_eq!(combined, expected);
}

#[test]
fn cli_proof_round_trips_for_every_leaf_in_two_through_eight_leaf_trees() {
    // Walks the tree-size space that hits every odd-leaf branch:
    //   2 → balanced.
    //   3 → odd at base.
    //   4 → balanced.
    //   5 → odd at base.
    //   6 → odd at level 1 (3 → 2 reduction is even, 6 → 3 is odd).
    //   7 → odd at base AND level 1.
    //   8 → fully balanced.
    for n in 2..=8 {
        let leaves = leaves_for_test(n);
        let cli_accounts = cli_accounts_for_test(n);
        let tree = MerkleTree::build(leaves.clone());

        for byte in 1u8..=(n as u8) {
            let target = pk(byte);
            let proof = build_inclusion_proof(&cli_accounts, &target).expect("proof");
            assert_eq!(
                proof.recomputed_root(),
                tree.root.0,
                "tree size {n}, target 0x{byte:02x}: recompute mismatch",
            );
            // And lazy-claim accepts the same proof against the same root.
            let leaf = leaf_hash(&target.to_bytes(), proof.lamports);
            assert!(
                verify_inclusion(leaf, &proof.proof, &proof.proof_flags, &tree.root.0),
                "lazy-claim rejected valid proof at tree size {n}, target 0x{byte:02x}",
            );
        }
    }
}

#[test]
fn empty_tree_root_matches_default_hash_per_spec_3_4() {
    // SPEC §3.4: empty input → claimable_root = [0; 32] (default Hash).
    let tree = MerkleTree::build(vec![]);
    assert_eq!(tree.root.0, Hash::default());
    assert_eq!(tree.leaf_count, 0);
}

#[test]
fn single_leaf_tree_proof_is_empty_and_root_is_leaf_hash() {
    let tree = MerkleTree::build(leaves_for_test(1));
    assert_eq!(tree.leaf_count, 1);
    assert_eq!(tree.root.0, leaf_hash(&pk(1).to_bytes(), 100));

    let cli_proof = build_inclusion_proof(&cli_accounts_for_test(1), &pk(1)).expect("proof");
    assert!(cli_proof.proof.is_empty());
    assert!(cli_proof.proof_flags.is_empty());
    // lazy-claim accepts the empty proof.
    let leaf = leaf_hash(&pk(1).to_bytes(), 100);
    assert!(verify_inclusion(leaf, &[], &[], &tree.root.0));
}

#[test]
fn input_order_independence_holds_for_all_three_crates() {
    // The genesis crate sorts leaves internally, the claim-cli sorts CLI accounts
    // internally, and the lazy-claim verifier doesn't care about order at all because
    // it consumes a single leaf + sibling path. Reordering the inputs to either the
    // genesis or the cli must not change the root or the proof.
    let order_a = leaves_for_test(7);
    let order_b: Vec<ClaimableLeaf> = order_a.iter().rev().cloned().collect();
    assert_eq!(
        MerkleTree::build(order_a).root,
        MerkleTree::build(order_b).root
    );

    let cli_a = cli_accounts_for_test(7);
    let cli_b: Vec<ClaimableAccount> = cli_a.iter().rev().cloned().collect();
    let proof_a = build_inclusion_proof(&cli_a, &pk(4)).expect("a");
    let proof_b = build_inclusion_proof(&cli_b, &pk(4)).expect("b");
    assert_eq!(proof_a, proof_b);
}

#[test]
fn tampered_sibling_hash_breaks_lazy_claim_verification() {
    // The cli builds a valid proof; flipping one bit in any sibling MUST cause
    // lazy-claim's verifier to reject. This is the basic Merkle security property and
    // we want it covered at the cross-crate boundary.
    let leaves = leaves_for_test(8);
    let cli_accounts = cli_accounts_for_test(8);
    let tree = MerkleTree::build(leaves);

    let target = pk(3);
    let proof = build_inclusion_proof(&cli_accounts, &target).expect("proof");
    let leaf = leaf_hash(&target.to_bytes(), proof.lamports);

    // Sanity: pristine proof verifies.
    assert!(verify_inclusion(
        leaf,
        &proof.proof,
        &proof.proof_flags,
        &tree.root.0
    ));

    // Now tamper.
    let mut tampered = proof.proof.clone();
    let mut bytes = tampered[0].to_bytes();
    bytes[0] ^= 0xFF;
    tampered[0] = Hash::new_from_array(bytes);
    assert!(!verify_inclusion(
        leaf,
        &tampered,
        &proof.proof_flags,
        &tree.root.0
    ));
}

#[test]
fn proof_flag_bit_swap_breaks_verification() {
    // Same setup, but instead of tampering with hash bytes, flip a single proof_flags
    // bit (which controls left/right combination). Must break verification.
    let leaves = leaves_for_test(8);
    let cli_accounts = cli_accounts_for_test(8);
    let tree = MerkleTree::build(leaves);

    let target = pk(3);
    let proof = build_inclusion_proof(&cli_accounts, &target).expect("proof");
    let leaf = leaf_hash(&target.to_bytes(), proof.lamports);

    let mut flipped_flags = proof.proof_flags.clone();
    flipped_flags[0] ^= 0b0000_0001;
    assert!(
        !verify_inclusion(leaf, &proof.proof, &flipped_flags, &tree.root.0),
        "flipped flag bit should break verification"
    );
}

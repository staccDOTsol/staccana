//! End-to-end claim flow.
//!
//! The flow under test:
//!
//! 1. Build a synthetic snapshot via [`mixed_snapshot`] (mix of EOAs, token accounts,
//!    PDAs, stake accounts).
//! 2. Run `staccana_genesis::build_genesis` to produce the canonical genesis output
//!    (Merkle root + treasury accumulator + classic defaults).
//! 3. Have the claim-cli partition the same snapshot rows and build an inclusion proof
//!    for one of the claimable EOAs.
//! 4. Walk that proof against the genesis Merkle root using `staccana_lazy_claim`'s
//!    `verify_inclusion` (the same code the on-chain program runs).
//!
//! The claim-cli's recomputed root MUST equal the genesis root byte-for-byte. Any drift
//! between the two — different sort order, different leaf hashing, different odd-leaf
//! handling — fails this test loudly.

use staccana_claim_cli::{
    build_inclusion_proof, partition_claimable, ClaimableAccount, SnapshotAccount,
};
use staccana_genesis::build_genesis;
use staccana_genesis::partition::SYSTEM_PROGRAM_ID;
use staccana_integration_tests::*;
use staccana_lazy_claim::merkle::{leaf_hash, verify_inclusion};

/// Convert a [`SyntheticAccount`] into the [`SnapshotAccount`] shape that the claim-cli
/// consumes (base58-encoded pubkey + owner strings).
fn to_snapshot_row(a: &SyntheticAccount) -> SnapshotAccount {
    SnapshotAccount {
        pubkey: bs58_inner(&a.pubkey.to_bytes()),
        owner: bs58_inner(&a.owner.to_bytes()),
        data_len: a.data_len as u64,
        lamports: a.lamports,
    }
}

/// Tiny base58 (Bitcoin alphabet) encoder so this crate doesn't take a `bs58` dep just
/// for the synthetic snapshot conversion. Output is exactly what `bs58::encode(...)` from
/// the rest of the workspace would produce.
fn bs58_inner(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let leading_zeros = bytes.iter().take_while(|&&b| b == 0).count();
    let mut digits: Vec<u8> = Vec::new();

    for &b in &bytes[leading_zeros..] {
        let mut carry = b as u32;
        for d in digits.iter_mut() {
            carry += (*d as u32) * 256;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }

    let mut out = String::new();
    for _ in 0..leading_zeros {
        out.push('1');
    }
    for &d in digits.iter().rev() {
        out.push(ALPHABET[d as usize] as char);
    }
    out
}

#[test]
fn synthetic_snapshot_to_genesis_root_via_build_genesis() {
    let snapshot = mixed_snapshot();
    let claimable_count = snapshot
        .iter()
        .filter(|a| a.owner == SYSTEM_PROGRAM_ID && a.data_len == 0)
        .count();

    let genesis = build_genesis(snapshot.clone());
    assert_eq!(genesis.claimable_count, claimable_count);

    // Treasury sum = lamports of every non-claimable account.
    let expected_treasury: u128 = snapshot
        .iter()
        .filter(|a| !(a.owner == SYSTEM_PROGRAM_ID && a.data_len == 0))
        .map(|a| a.lamports as u128)
        .sum();
    assert_eq!(genesis.treasury.total_lamports(), expected_treasury);
}

#[test]
fn claim_cli_proof_reconstructs_genesis_root() {
    let snapshot = mixed_snapshot();
    let genesis = build_genesis(snapshot.clone());

    // Mirror the genesis pipeline through the claim-cli to produce its own claimable
    // partition, then build an inclusion proof for one known EOA (pk(0x01)).
    let snapshot_rows: Vec<SnapshotAccount> = snapshot.iter().map(to_snapshot_row).collect();
    let claimable: Vec<ClaimableAccount> =
        partition_claimable(&snapshot_rows).expect("claim-cli partition");

    let target = pk(0x01);
    let proof = build_inclusion_proof(&claimable, &target).expect("inclusion proof");

    // The claim-cli's reported root MUST match the genesis crate's root byte-for-byte.
    assert_eq!(
        proof.root, genesis.claimable_root.0,
        "claim-cli root must match genesis root"
    );
    // And the recomputed root from the proof itself must too.
    assert_eq!(
        proof.recomputed_root(),
        genesis.claimable_root.0,
        "proof recompute must match genesis root"
    );
}

#[test]
fn lazy_claim_verify_accepts_claim_cli_proof_against_genesis_root() {
    // The strongest cross-crate assertion: the on-chain verifier (lazy-claim) accepts the
    // off-chain proof (claim-cli) against the canonical genesis root. If any of the three
    // crates drift on hashing, sort order, or proof flags, this test fails.
    let snapshot = mixed_snapshot();
    let genesis = build_genesis(snapshot.clone());
    let snapshot_rows: Vec<SnapshotAccount> = snapshot.iter().map(to_snapshot_row).collect();
    let claimable = partition_claimable(&snapshot_rows).expect("claim-cli partition");

    for byte in [0x01u8, 0x02, 0x05, 0x08] {
        let target = pk(byte);
        let proof = build_inclusion_proof(&claimable, &target).expect("proof");

        let leaf = leaf_hash(&target.to_bytes(), proof.lamports);
        assert!(
            verify_inclusion(
                leaf,
                &proof.proof,
                &proof.proof_flags,
                &genesis.claimable_root.0
            ),
            "lazy-claim must accept claim-cli proof for pubkey 0x{byte:02x}"
        );
    }
}

#[test]
fn lazy_claim_verify_rejects_proof_under_wrong_lamports() {
    // The claim-cli builds the proof for a known (pubkey, lamports) pair. Tampering with
    // the lamports MUST cause lazy-claim to reject — otherwise an attacker could grant
    // themselves a larger balance with a valid Merkle path.
    let snapshot = mixed_snapshot();
    let genesis = build_genesis(snapshot.clone());
    let snapshot_rows: Vec<SnapshotAccount> = snapshot.iter().map(to_snapshot_row).collect();
    let claimable = partition_claimable(&snapshot_rows).expect("claim-cli partition");

    let target = pk(0x01);
    let proof = build_inclusion_proof(&claimable, &target).expect("proof");
    let tampered = leaf_hash(&target.to_bytes(), proof.lamports.wrapping_add(1));
    assert!(
        !verify_inclusion(
            tampered,
            &proof.proof,
            &proof.proof_flags,
            &genesis.claimable_root.0
        ),
        "tampered lamports must not verify"
    );
}

#[test]
fn missing_target_in_claim_cli_returns_explicit_error() {
    // Sanity-check that the claim-cli's "not in claimable partition" path is wired up so
    // an integrator who points the CLI at the wrong snapshot gets a clear failure rather
    // than a confusing root-mismatch error downstream.
    let snapshot = mixed_snapshot();
    let snapshot_rows: Vec<SnapshotAccount> = snapshot.iter().map(to_snapshot_row).collect();
    let claimable = partition_claimable(&snapshot_rows).expect("claim-cli partition");
    let nowhere = pk(0xEE);
    let err = build_inclusion_proof(&claimable, &nowhere).expect_err("must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("not present in the claimable partition"),
        "got: {msg}"
    );
}

#[test]
fn genesis_treasury_total_equals_sum_of_non_claimable_lamports() {
    // Restated I1 (genesis SOL conservation), specifically for the mixed_snapshot fixture.
    // The proptest version in `property_invariants.rs` covers this for arbitrary inputs.
    let snapshot = mixed_snapshot();
    let total_input: u128 = snapshot.iter().map(|a| a.lamports as u128).sum();
    let genesis = build_genesis(snapshot.clone());
    let claimable_total: u128 = snapshot
        .iter()
        .filter(|a| a.owner == SYSTEM_PROGRAM_ID && a.data_len == 0)
        .map(|a| a.lamports as u128)
        .sum();
    assert_eq!(
        claimable_total + genesis.treasury.total_lamports(),
        total_input
    );
}

#[test]
fn deterministic_keypair_signs_canonical_claim_message() {
    // The claim flow's signed message (SPEC §4.2) carries the pubkey of the keypair that
    // signs it. Confirm that the deterministic_keypair helper hands back a `Keypair`
    // whose pubkey equals what the claim message embeds, and that the keypair can
    // actually sign / verify the canonical message.
    use solana_sdk::signer::Signer;

    let kp = deterministic_keypair(0x42);
    let pubkey = kp.pubkey();
    let lamports = 1_234u64;
    let msg = staccana_claim_cli::tx::build_claim_message(&pubkey, lamports);
    // First 17 bytes are the domain; bytes 17..49 are the pubkey.
    assert_eq!(&msg[17..49], pubkey.as_ref());
    // The signature should round-trip through verification.
    let sig = kp.sign_message(&msg);
    assert!(sig.verify(pubkey.as_ref(), &msg));
}

#[test]
fn genesis_emit_compose_carries_root_through_to_lazy_claim_account() {
    // Cross-crate sanity: the genesis-emit composer wraps the same Merkle root that
    // build_genesis produced. The lazy-claim program embeds this root into its config
    // account at genesis, so any drift here breaks the on-chain verifier.
    let snapshot = mixed_snapshot();
    let genesis = build_genesis(snapshot);
    let composed = staccana_genesis_emit::compose(&genesis);
    assert_eq!(
        composed.lazy_claim_account.claimable_root,
        genesis.claimable_root.0.to_bytes(),
        "ComposedGenesis root must equal GenesisOutput root"
    );
    assert_eq!(composed.claimable_count, genesis.claimable_count as u64);
    assert_eq!(
        composed.treasury_pda_lamports,
        genesis.treasury.lamports_for_pda()
    );
}

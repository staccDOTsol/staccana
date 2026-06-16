//! SPEC §2 invariants for the classic v1 economic defaults inherited at staccana genesis.
//!
//! These constants are normative: they're committed to in the genesis configuration
//! (SPEC §3.5) and must not drift. This file pins each value to the SPEC and verifies the
//! `ClassicDefaults` helpers expose them consistently.

use staccana_genesis::*;

#[test]
fn fixed_transaction_fee_lamports_matches_spec() {
    // SPEC §2.2: FIXED_TRANSACTION_FEE_LAMPORTS = 27_000_000 (0.027 SOL).
    assert_eq!(FIXED_TRANSACTION_FEE_LAMPORTS, 27_000_000);
}

#[test]
fn vote_transaction_fee_lamports_matches_spec() {
    // SPEC §2.2: VOTE_TRANSACTION_FEE_LAMPORTS = 5_000.
    assert_eq!(VOTE_TRANSACTION_FEE_LAMPORTS, 5_000);
}

#[test]
fn burn_percent_matches_spec() {
    // SPEC §2.2: BURN_PERCENT = 50 (50% of fees burned).
    assert_eq!(BURN_PERCENT, 50);
}

#[test]
fn inflation_disabled_matches_spec() {
    // SPEC §2.2: INFLATION = disabled.
    assert!(ClassicDefaults::inflation_disabled());
}

#[test]
fn fee_governor_emits_pinned_fixed_fee() {
    // The min/max bracket is collapsed to the fixed fee — no dynamic adjustment ever.
    let g = ClassicDefaults::fee_rate_governor();
    assert_eq!(g.target_lamports_per_signature, FIXED_TRANSACTION_FEE_LAMPORTS);
    assert_eq!(g.min_lamports_per_signature, FIXED_TRANSACTION_FEE_LAMPORTS);
    assert_eq!(g.max_lamports_per_signature, FIXED_TRANSACTION_FEE_LAMPORTS);
    assert_eq!(g.target_signatures_per_slot, 0);
    assert_eq!(g.burn_percent, BURN_PERCENT);
}

#[test]
fn cte_feature_gates_count_matches_spec() {
    // SPEC §2.4: 4 ZK / CTE gates + 5 Token-22 v8 syscall prerequisites + 1 SBPFv3
    // deployment gate = 10 gates ship ON at slot 0. The constant name keeps its `CTE_`
    // prefix for backwards-compat (see `genesis/src/classic_defaults.rs`).
    assert_eq!(CTE_FEATURE_GATES_AT_GENESIS.len(), 10);
}

#[test]
fn cte_feature_gates_pubkeys_match_spec() {
    // SPEC §2.4 lists these ten pubkeys exactly. The order in the const is normative —
    // any reordering changes the genesis fingerprint downstream. Positions 0..4 are the
    // ZK / confidential-transfer gates; 4..9 the Token-22 v8 syscall prerequisites;
    // position 9 the SBPFv3 deployment gate.
    let expected_pubkeys = [
        "zk1snxsc6Fh3wsGNbbHAJNHiJoYgF29mMnTSusGx5EJ",
        "zkesAyFB19sTkX8i9ReoKaMNDA4YNTPYJpZKPDt7FMW",
        "zkNLP7EQALfC1TYeB3biDU7akDckj8iPkvh9y2Mt2K3",
        "zkiTNuzBKxrCLMKehzuQeKZyLtX2yvFcEKMML8nExU8",
        "7rcw5UtqgDTBBv2EcynNfYckgdAaH1MAsCjKgXMkN7Ri",
        "A16q37opZdQMCbe5qJ6xpBB9usykfv8jZaMkxvZQi4GJ",
        "EJJewYSddEEtSZHiqugnvhQHiWyZKjkFDQASd7oKSagn",
        "EeyoXa3AyQuHkhRmT9mhKtTPrLNPBuNQbLEvyt5VrYxv",
        "EaQpmC6GtRssaZ3PCUM5YksGqUdMLeZ46BQXYtHYakDS",
        "BUwGLeF3Lxyfv1J1wY8biFHBB2hrk2QhbNftQf3VV3cC",
    ];
    for (i, expected) in expected_pubkeys.iter().enumerate() {
        assert_eq!(
            CTE_FEATURE_GATES_AT_GENESIS[i].0, *expected,
            "CTE gate {} pubkey diverged from SPEC §2.4",
            i
        );
    }
}

#[test]
fn cte_feature_gates_have_descriptions() {
    // Every gate carries a short description (the second tuple field). Empty descriptions
    // would be a regression — the human-readable label is what makes the gate set
    // self-documenting in the genesis config.
    for (pubkey, desc) in CTE_FEATURE_GATES_AT_GENESIS {
        assert!(!pubkey.is_empty(), "pubkey must not be empty");
        assert!(
            !desc.is_empty(),
            "description for {} must not be empty",
            pubkey
        );
    }
}

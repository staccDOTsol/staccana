//! End-to-end chain-simulation tests for Staccana.
//!
//! Distinct from the cross-crate unit-and-property suite under `integration-tests/`. This
//! crate stands up a real `solana-program-test` BanksClient, registers the lazy-claim
//! program in-process via `processor!()`, and drives full transactions through it. The
//! point is to exercise everything below the validator boundary: ed25519 precompile,
//! Instructions sysvar inspection, treasury PDA debit, claimed-marker PDA initialization,
//! and the per-pubkey idempotency guarantee.
//!
//! Layout:
//!
//! - [`harness`]   — `ProgramTest` builder helpers (genesis composition, lazy-claim
//!                   config and treasury PDA pre-state, marker PDA address derivation).
//! - [`synthetic`] — deterministic keypair / snapshot generators that match the genesis
//!                   partition rule (mix of system-owned EOAs and token-program-owned
//!                   accounts so both branches of the partition fire).
//!
//! Tests under `tests/`:
//!
//! - `e2e_claim.rs`             — full lazy-claim happy path + idempotency + bad-proof
//!                                negative path.
//! - `e2e_genesis_to_claim.rs`  — full pipeline: synthetic JSON snapshot → MockSnapshot →
//!                                build_genesis → compose → claim every claimable account
//!                                → assert SOL conservation invariant I1.
//! - `e2e_matcher.rs`           — pure-Rust FBA round across three base mints.

pub mod harness;
pub mod synthetic;

pub use harness::{
    build_lazy_claim_program_test, install_claim_pre_state, install_lazy_claim_config,
    pre_credit_treasury, ClaimPreState, LazyClaimSetup, LAZY_CLAIM_TEST_PROGRAM_ID,
    TREASURY_PDA_SEED,
};
pub use synthetic::{
    deterministic_keypair, mixed_synthetic_snapshot, snapshot_to_json, synthetic_eoa_with_keypair,
    synthetic_token_account, SyntheticSnapshotAccount,
};

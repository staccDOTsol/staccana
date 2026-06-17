//! Cross-crate integration tests for Staccana.
//!
//! This crate contains shared utilities (test fixtures, mock AMM, deterministic keypair
//! generators) used by the integration test files under `tests/`. Each test file under
//! `tests/` corresponds to one cross-crate flow or one cluster of related invariants:
//!
//! * `claim_flow.rs`         — synthetic snapshot → `build_genesis` → claim-cli proof
//!                             reconstruction → genesis root verification.
//! * `matcher_scenarios.rs`  — matcher against a real-feeling constant-product AMM,
//!                             multi-mint batches, residual flow, varied buyer/seller
//!                             distributions.
//! * `spec_conformance.rs`   — byte-exact wire-format assertions against `docs/SPEC.md`
//!                             for the claim message (§4.2), `ClaimArgs` (§4.1), and the
//!                             SwapIntent canonical encoding (§6.1).
//! * `merkle_consistency.rs` — Merkle tree built via `staccana-genesis` ↔ proof generated
//!                             via `staccana-claim-cli` ↔ inclusion check via
//!                             `staccana-lazy-claim` agree byte-for-byte.
//! * `property_invariants.rs` — proptest-driven SPEC §8 invariants: I1 (genesis SOL
//!                             conservation), I5 (matcher replay invariance), Merkle
//!                             determinism, and treasury commutativity.

pub mod fixtures;
pub mod mocks;

pub use fixtures::{
    deterministic_keypair, mixed_snapshot, pk, synthetic_eoa, synthetic_pda,
    synthetic_stake_account, synthetic_token_account, SyntheticAccount,
};
pub use mocks::{ConstantProductAmm, ConstantProductPool};

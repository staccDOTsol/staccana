//! Megadrop snapshot → Merkle root + per-holder allocations.
//!
//! Reads two operator-held snapshot files (`based_stacc_0_holders.json`,
//! `proofv3_holders.json`), applies a linear allocation policy, normalizes the result
//! so the lamport sum exactly equals the configured total, and emits a Merkle tree
//! whose root can be passed to the on-chain `init_megadrop` ix.
//!
//! ## Module layout
//!
//! - [`input`] — JSONL parsing for the operator's snapshot files.
//! - [`allocate`] — linear allocation formula + lamport normalization.
//! - [`tree`] — Merkle tree wrapper that delegates to `staccana_genesis::merkle`,
//!   which is the same builder genesis-bake uses; this guarantees the root we emit
//!   matches the on-chain verifier byte-for-byte.
//! - [`output`] — file writers for `allocations.json`, `merkle-leaves.json`,
//!   `merkle-tree.json`, `merkle-root.hex`, `summary.json`.
//!
//! All allocation arithmetic is in u128 to absorb the production scale (30M SOL =
//! 3e16 lamports) without overflow risk.

pub mod allocate;
pub mod input;
pub mod output;
pub mod tree;

pub use allocate::{
    compute_allocations, AllocationParams, HolderAllocation, HolderContributions,
};
pub use input::{
    load_based_stacc_holders, load_proofv3_holders, BasedStaccHolderRecord, ProofV3HolderRecord,
};
pub use output::{
    write_outputs, AllocationRow, MerkleLeafRow, ProofRow, RunOutputs, Summary,
};
pub use tree::{build_tree, BuiltTree};

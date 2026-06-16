//! Library entry for the staccana lazy-claim CLI.
//!
//! Splits the runtime work — proof construction, transaction encoding, RPC submit — into
//! testable modules so that `main.rs` can stay a thin command-line wrapper.
//!
//! The CLI flow is:
//!
//! 1. Load the user's mainnet keypair from disk.
//! 2. Load the staccana snapshot file (the same JSON shape that
//!    `tools/snapshot-fork` consumes — a flat array of accounts).
//! 3. Partition the snapshot into the **claimable** set per the genesis rule
//!    (system-owned, zero data) and build the inclusion proof for the user's pubkey.
//! 4. Construct the `ed25519` precompile instruction signing the message defined in
//!    `docs/SPEC.md` §4.2.
//! 5. Construct the `claim` instruction per `docs/SPEC.md` §4.1.
//! 6. Submit the transaction to a staccana RPC endpoint.
//!
//! Everything except step 6 is testable without an RPC connection — see the unit tests in
//! each module.

pub mod proof;
pub mod submit;
pub mod tx;

pub use proof::{
    build_inclusion_proof, load_snapshot_accounts, partition_claimable, ClaimableAccount,
    InclusionProof, ProofError, SnapshotAccount,
};
pub use submit::{submit_claim_transaction, SubmitError};
pub use tx::{
    build_claim_instruction, build_claim_message, build_ed25519_precompile_instruction,
    claimed_marker_pda, ClaimArgs, LAZY_CLAIM_PROGRAM_ID, STACCANA_CLAIM_DOMAIN,
};

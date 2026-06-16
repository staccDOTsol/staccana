//! Genesis builder for Staccana.
//!
//! Consumes a Solana mainnet snapshot (abstracted via the [`Account`] trait so the heavy
//! `solana-runtime` / `solana-accounts-db` dependencies stay out of this crate) and produces:
//!
//! 1. A Merkle root over the **claimable** partition — every account that is system-program
//!    owned with zero data. The root goes into the lazy-claim program at genesis.
//! 2. A **treasury** total — the sum of lamports from every other account. Credited to a
//!    treasury PDA at slot 0; funds project ops since inflation is disabled.
//! 3. The **classic v1 defaults** — fixed fee governor, disabled inflation, 50% burn.
//!    Inherited unchanged from `solana-classic` v1.
//! 4. The **CTE feature gate set** — the four ZK ElGamal Proof / confidential transfer
//!    gates that ship ON at slot 0.
//!
//! See `docs/ARCHITECTURE.md` for the surrounding design.

pub mod builder;
pub mod classic_defaults;
pub mod merkle;
pub mod partition;
pub mod treasury;

pub use builder::{build_genesis, build_genesis_with_tree, GenesisOutput};
pub use classic_defaults::{
    ClassicDefaults, FeeRateGovernor, BURN_PERCENT, CTE_FEATURE_GATES_AT_GENESIS,
    FIXED_TRANSACTION_FEE_LAMPORTS, VOTE_TRANSACTION_FEE_LAMPORTS,
};
pub use merkle::{
    ClaimableLeaf, MerkleRoot, MerkleTree, MerkleTreeWithLayers, LEAF_DOMAIN, NODE_DOMAIN,
};
pub use partition::{partition, Account, Disposition, SYSTEM_PROGRAM_ID};
pub use treasury::Treasury;

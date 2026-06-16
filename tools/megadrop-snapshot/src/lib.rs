//! `staccana-megadrop-snapshot` — walk holder snapshots of two Solana mainnet
//! collections and produce the megadrop Merkle root + per-holder allocations.
//!
//! Two cohorts (per `docs/MEGADROP.md` §"Snapshot inputs"):
//!
//! - `based_stacc_0` — Metaplex NFT collection
//!   `Ej1jbbw7QKgC9XMmWPxKFipMLJY5oVNd3rdbE1TzjNdz`. Per-holder count = number of NFTs.
//! - `proofv3` — Token-22 SPL fungible mint
//!   `CLWeikxiw8pC9JEtZt14fqDzYfXF7uVwLuvnJPkrE7av`. Per-holder amount = sum of
//!   balances across all the holder's token accounts (a holder may own multiple ATAs).
//!
//! The walk is implemented behind the [`das::DasClient`] trait so unit tests can mock
//! it without touching mainnet RPC.
//!
//! ## Pipeline
//!
//! ```text
//! das::DasClient ──┐
//!                  ├─► snapshot::collect_holders()
//! ─────────────────┘            │
//!                                ▼
//!                       allocate::compute_allocations()
//!                                │
//!                                ▼
//!                       output::write_outputs()
//! ```

pub mod allocate;
pub mod cli;
pub mod das;
pub mod output;
pub mod snapshot;

pub use allocate::{compute_allocations, AllocationModel, HolderAllocation};
pub use cli::{run, Args};
pub use snapshot::{collect_holders, HolderEntry};

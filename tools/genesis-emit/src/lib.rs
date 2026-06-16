//! Genesis-emit pipeline for Staccana.
//!
//! Compose a [`staccana_genesis::GenesisOutput`] (produced by the snapshot-fork
//! tool) into a [`composed::ComposedGenesis`] — the typed, serde-friendly
//! handoff struct that the (eventual) agave-side genesis bootstrap consumes.
//!
//! # Pipeline
//!
//! ```text
//!   tools/snapshot-fork/  --[GenesisOutput JSON]-->  tools/genesis-emit/
//!                                                       |
//!                                                       v
//!                                              compose::compose()
//!                                                       |
//!                                                       v
//!                                              ComposedGenesis (JSON)
//!                                                       |
//!                                                       v
//!                                          [TODO: agave fork bootstrap]
//!                                                       |
//!                                                       v
//!                                                  genesis.bin
//! ```
//!
//! # v0 scope
//!
//! This crate stops at the JSON-encoded `ComposedGenesis`. Producing the actual
//! `genesis.bin` requires `solana-genesis` / `solana-runtime` / `solana-sdk`
//! (heavy crates we deliberately keep out of the pure pipeline). See
//! [`emit`] module-level docs for the integration TODO.

pub mod compose;
pub mod composed;
pub mod emit;

pub use compose::{compose, DEFAULT_BANK_HASH_SEED};
pub use composed::{ActiveFeatureGate, ComposedGenesis, LazyClaimGenesisAccount};
pub use emit::{load_genesis_output, write_composed_genesis, write_genesis_output};

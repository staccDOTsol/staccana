//! Instruction handlers for the bridge program.
//!
//! Each submodule defines one instruction, its `Accounts` context, and its handler.
//! Pure helpers (R math, attestation message construction, ed25519 precompile reading)
//! live in `crate::attestation` and `crate::ed25519` and are unit-tested there.
//!
//! Instruction set (SPEC.md §5):
//! - [`register_asset`] — governance one-shot to register an `(AssetConfig, RatioState)`
//!   pair for a new asset. Also initializes the outbound nonce counter.
//! - [`update_ratio`]  — federation publishes a new R for an asset.
//! - [`mint`]          — relay an inbound (mainnet → staccana) attestation, mint Token-22
//!   wrapper to the recipient ATA.
//! - [`burn`]          — user burns wrapper tokens, emits an event for the federation
//!   to relay back to the mainnet vault.

pub mod burn;
pub mod convert_native_to_wsol;
pub mod convert_wsol_to_native;
pub mod mint;
pub mod register_asset;
pub mod update_ratio;

pub use burn::*;
pub use convert_native_to_wsol::*;
pub use convert_wsol_to_native::*;
pub use mint::*;
pub use register_asset::*;
pub use update_ratio::*;

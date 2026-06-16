//! Instruction handlers for the mainnet vault program.
//!
//! Each submodule defines one instruction, its `Accounts` context, and its handler.
//! Pure helpers (release-message construction, ed25519 precompile reading, fee math)
//! live in `crate::attestation` and `crate::ed25519` and are unit-tested there.
//!
//! Instructions:
//! - [`init_vault`] — governance one-shot to register a `VaultConfig` for an asset and
//!   bootstrap the federation set on first call.
//! - [`deposit`]    — user locks underlying (or wraps native SOL) into the vault and
//!   declares a staccana destination; emits a `DepositEvent`.
//! - [`release_with_attestation`] — verify M-of-N federation sigs over a release
//!   attestation, transfer underlying to the attested recipient, mark the staccana-side
//!   outbound nonce consumed.

pub mod deposit;
pub mod init_vault;
pub mod release_with_attestation;

pub use deposit::*;
pub use init_vault::*;
pub use release_with_attestation::*;

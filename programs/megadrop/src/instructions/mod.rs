//! Instruction handlers for the megadrop program.
//!
//! Each submodule defines one instruction, its `Accounts` context, and its handler.
//! Pure helpers (tranche math, message construction, calendar arithmetic, ed25519
//! precompile reading) live in `crate::megadrop`, `crate::calendar`, and
//! `crate::ed25519` and are unit-tested there.
//!
//! Instructions (per `docs/MEGADROP.md`):
//!
//! - [`init_megadrop`]  — governance one-shot to create the singleton config.
//! - [`claim_megadrop`] — user-facing claim with Merkle proof + ed25519 sig +
//!   per-tranche unlock check.

pub mod claim_megadrop;
pub mod init_megadrop;
pub mod proof_buffer;
pub mod update_megadrop;

pub use claim_megadrop::*;
pub use init_megadrop::*;
pub use proof_buffer::*;
pub use update_megadrop::*;

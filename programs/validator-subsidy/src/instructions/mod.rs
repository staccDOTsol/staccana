//! Instruction handlers for the validator-subsidy program.
//!
//! Each submodule defines one instruction, its `Accounts` context, and its handler.
//! Pure helpers (weight + share math, attestation message construction, ed25519
//! precompile reading) live in `crate::subsidy` and `crate::ed25519` and are unit-tested
//! there.
//!
//! Instruction set (SPEC.md §7.2 / §7.3):
//!
//! - [`init_subsidy`]               — governance one-shot bootstrap.
//! - [`stake_to_productive`]        — governance CPI into bridge `mint`.
//! - [`unstake_from_productive`]    — governance CPI into bridge `burn`.
//! - [`register_validator`]         — governance adds a validator to the registry.
//! - [`update_validator_metrics`]   — federation-attested metrics update.
//! - [`distribute_yield`]           — permissionless: pays validators their pro-rata
//!   share of the epoch's observed yield.
//! - [`bootstrap_distribute`]       — permissionless: replaces yield distribution for
//!   the first 60 epochs while the productive position has not yet earned anything.

pub mod admin_set_metrics;
pub mod bootstrap_distribute;
pub mod delegate_treasury_stake;
pub mod distribute_yield;
pub mod init_subsidy;
pub mod migrate_treasury_owner;
pub mod register_validator;
pub mod set_bootstrap_per_epoch;
pub mod stake_to_productive;
pub mod unregister_validator;
pub mod unstake_from_productive;
pub mod update_validator_metrics;

pub use admin_set_metrics::*;
pub use bootstrap_distribute::*;
pub use delegate_treasury_stake::*;
pub use distribute_yield::*;
pub use init_subsidy::*;
pub use migrate_treasury_owner::*;
pub use register_validator::*;
pub use set_bootstrap_per_epoch::*;
pub use stake_to_productive::*;
pub use unregister_validator::*;
pub use unstake_from_productive::*;
pub use update_validator_metrics::*;

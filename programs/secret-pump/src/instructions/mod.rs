//! Instruction handlers for secret-pump.
//!
//! Each submodule defines an Anchor `#[derive(Accounts)]` struct and a `handler` function
//! invoked from the program module in [`crate::lib`]. The handlers do account validation
//! and CPI plumbing; all curve math is delegated to [`crate::curve`] so the swap logic
//! itself is exercisable without an on-chain runtime.

pub mod buy;
pub mod create;
pub mod sell;

pub use buy::*;
pub use create::*;
pub use sell::*;

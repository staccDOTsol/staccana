//! Per-mint frequent batch auction matcher.
//!
//! This crate is the deterministic core of the Staccana consensus rule. It takes a set of
//! [`SwapIntent`]s observed within a slot, groups them by their longtail (non-quote) mint,
//! and clears each group at a single AMM-anchored uniform price.
//!
//! Determinism matters: any validator replaying the same input set must produce the same
//! [`ClearingResult`]s byte-for-byte. The matcher achieves this via:
//!
//! * Sorted iteration over the (base, quote) groups (`BTreeMap`).
//! * Sorting buys and sells by `(in_amount desc, signer asc)` — deterministic tiebreak on
//!   the signer pubkey when amounts are equal.
//! * No floating-point math; clearing price is Q64.64 fixed-point throughout.
//!
//! See `docs/ARCHITECTURE.md` for the surrounding design.

pub mod amm;
pub mod batch;
pub mod intent;
pub mod quote_registry;

pub use amm::AmmAdapter;
pub use batch::{batch_match, BatchConfig, ClearingResult, Match};
pub use intent::{Side, SwapIntent, SWAP_INTENT_CANONICAL_LEN};
pub use quote_registry::QuoteRegistry;

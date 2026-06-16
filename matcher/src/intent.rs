//! Swap intent type — the unit of work the matcher consumes.

use serde::{Deserialize, Serialize};
use solana_program::pubkey::Pubkey;

/// A user's signed intent to swap `in_amount` of `in_mint` for at least `min_out` of `out_mint`.
///
/// Intents replace direct AMM calls in the staccana fork. Every swap that flows through the
/// chain surfaces as an intent, which is what makes per-mint batch matching possible without
/// breaking determinism.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapIntent {
    pub signer: Pubkey,
    pub in_mint: Pubkey,
    pub in_amount: u64,
    pub out_mint: Pubkey,
    pub min_out: u64,
    pub nonce: u64,
}

/// Length of the canonical encoding of a single [`SwapIntent`].
///
/// `signer (32) + in_mint (32) + in_amount (8) + out_mint (32) + min_out (8) + nonce (8)`.
pub const SWAP_INTENT_CANONICAL_LEN: usize = 32 + 32 + 8 + 32 + 8 + 8;

impl SwapIntent {
    /// Canonical encoding per `docs/SPEC.md` §6.1: fields concatenated in declaration order,
    /// little-endian for `u64`, raw 32-byte pubkey bytes.
    ///
    /// Used by the block-level commitment (SPEC §6.5) where `intent_set_hash` =
    /// SHA-256 over the canonical encoding of every intent in the slot, sorted by
    /// `(signer, nonce)`.
    pub fn to_canonical_bytes(&self) -> [u8; SWAP_INTENT_CANONICAL_LEN] {
        let mut out = [0u8; SWAP_INTENT_CANONICAL_LEN];
        out[0..32].copy_from_slice(self.signer.as_ref());
        out[32..64].copy_from_slice(self.in_mint.as_ref());
        out[64..72].copy_from_slice(&self.in_amount.to_le_bytes());
        out[72..104].copy_from_slice(self.out_mint.as_ref());
        out[104..112].copy_from_slice(&self.min_out.to_le_bytes());
        out[112..120].copy_from_slice(&self.nonce.to_le_bytes());
        out
    }
}

/// Side of a batch from the perspective of the longtail (base) mint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Intent acquires the base mint (out_mint == base).
    Buy,
    /// Intent disposes of the base mint (in_mint == base).
    Sell,
}

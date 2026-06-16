//! The [`ComposedGenesis`] struct — the typed handoff between the pure genesis
//! library and the (eventual) agave-side bootstrap that turns it into an actual
//! Solana `genesis.bin`.
//!
//! Everything the validator needs to boot from slot 0 lives here in serde-friendly
//! form. No heavy `solana-runtime` / `solana-genesis` types appear in this struct so
//! it can be loaded, inspected, and round-tripped through JSON without dragging the
//! entire validator dependency graph along.
//!
//! # Layout
//!
//! - [`ComposedGenesis::fee_governor`] — the fixed-fee classic v1 governor (see
//!   `staccana_genesis::ClassicDefaults::fee_rate_governor`).
//! - [`ComposedGenesis::inflation_disabled`] — always `true`; staccana validators
//!   earn from fees only.
//! - [`ComposedGenesis::active_feature_gates`] — the four CTE / ZK ElGamal Proof
//!   gates that ship ON at slot 0 (§2.4 of `docs/SPEC.md`). Upstream-active gates
//!   are appended in the agave wiring step (TODO).
//! - [`ComposedGenesis::treasury_pda_lamports`] — pre-credited balance for the
//!   treasury PDA derived from `["treasury", TREASURY_PROGRAM_ID]` (the address
//!   itself isn't computed here; see TODO).
//! - [`ComposedGenesis::lazy_claim_account`] — embeds the Merkle root the
//!   lazy-claim program verifies inclusion proofs against.
//! - [`ComposedGenesis::claimable_count`] — purely informational; the number of
//!   leaves that contributed to the root.
//! - [`ComposedGenesis::bank_hash_seed`] — distinct-bank-hash discriminator (§3.5).

use serde::{Deserialize, Serialize};
use staccana_genesis::{FeeRateGovernor, MerkleRoot};

/// Identifier + activation reason for a feature gate that should be marked active
/// at slot 0.
///
/// Mirrors the `(&str, &str)` tuples in
/// `staccana_genesis::CTE_FEATURE_GATES_AT_GENESIS` but as an owned, serde-friendly
/// struct so the composed genesis is fully self-contained on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveFeatureGate {
    /// Base58-encoded feature gate program ID (e.g. `zk1snxsc6Fh3wsGNbbHAJNHiJoYgF29mMnTSusGx5EJ`).
    pub pubkey_b58: String,
    /// Human-readable description of what activating this gate enables.
    pub description: String,
}

/// Genesis-time account data for the lazy-claim program.
///
/// In the eventual agave wiring this will be serialized into the program-data
/// account at `LAZY_CLAIM_PROGRAM_ID` so the on-chain program can read its
/// embedded `claimable_root` via account data load. For v0 we only need the root
/// itself; the on-chain serialization layout is a TODO that depends on the final
/// `programs/lazy-claim/` state account schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LazyClaimGenesisAccount {
    /// 32-byte Merkle root over the claimable partition (§3.4).
    pub claimable_root: [u8; 32],
}

impl LazyClaimGenesisAccount {
    pub fn from_root(root: MerkleRoot) -> Self {
        Self {
            claimable_root: root.0.to_bytes(),
        }
    }
}

/// The complete, typed handoff struct from the pure genesis library to the
/// agave-side genesis bootstrap.
///
/// Producing this struct is the responsibility of [`crate::compose::compose`];
/// writing it to disk is the responsibility of [`crate::emit`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposedGenesis {
    /// Fixed-fee classic v1 governor (0.027 SOL non-vote, 5_000 lamports vote,
    /// 50% burn). See `staccana_genesis::ClassicDefaults::fee_rate_governor`.
    pub fee_governor: FeeRateGovernor,

    /// Always `true` for staccana v2.
    pub inflation_disabled: bool,

    /// Feature gates flipped ON at slot 0. Currently the four CTE / ZK ElGamal
    /// Proof gates from §2.4. The agave wiring will additionally union this with
    /// the set of upstream-active gates at fork time (TODO).
    pub active_feature_gates: Vec<ActiveFeatureGate>,

    /// Pre-credited lamport balance for the treasury PDA at
    /// `find_program_address(["treasury"], TREASURY_PROGRAM_ID)`. Sourced from
    /// `Treasury::lamports_for_pda()`. The PDA address itself isn't computed in
    /// this crate — see TODO in `compose.rs`.
    pub treasury_pda_lamports: u64,

    /// Number of treasury accounts that contributed to
    /// `treasury_pda_lamports`. Purely informational; not used at boot.
    pub treasury_account_count: u64,

    /// Genesis-time account data for the lazy-claim program (carries the Merkle
    /// root). Will be serialized into the program's state account.
    pub lazy_claim_account: LazyClaimGenesisAccount,

    /// Number of claimable leaves that contributed to the Merkle root. Purely
    /// informational; not used at boot.
    pub claimable_count: u64,

    /// Discriminator that ensures staccana's bank hash at slot 0 is distinct from
    /// mainnet's, so cross-chain double-signing slashing does not apply (§3.5).
    /// The agave bootstrap mixes this into the slot-0 bank hash computation.
    pub bank_hash_seed: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::hash::Hash;

    #[test]
    fn lazy_claim_account_round_trips_root() {
        let bytes = [7u8; 32];
        let root = MerkleRoot(Hash::new_from_array(bytes));
        let acct = LazyClaimGenesisAccount::from_root(root);
        assert_eq!(acct.claimable_root, bytes);
    }
}

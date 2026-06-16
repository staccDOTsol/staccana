//! On-disk emission of the [`ComposedGenesis`].
//!
//! v0 scope: pretty-printed JSON. The struct is fully serde-friendly so this is a
//! one-liner. JSON gives us trivial inspectability and round-trip testability
//! without committing to a binary format prematurely.
//!
//! # Future: real `genesis.bin` emission
//!
//! TODO: produce an actual `solana-genesis::Builder`-compatible `genesis.bin` so
//! the (forked) agave validator boots directly from the output of this tool.
//! That requires:
//!
//! - `solana-genesis-config` (for `GenesisConfig`, `Inflation`)
//! - `solana-fee-calculator` (for the real `FeeRateGovernor` we currently mirror)
//! - `solana-feature-set` (for the `FEATURE_NAMES` map and `Feature` account
//!   layout to mark each `active_feature_gates` entry as active at slot 0)
//! - `solana-program::system_program::create_account` (for the genesis treasury
//!   PDA pre-credit and the lazy-claim program account)
//! - `solana-program::pubkey::Pubkey::find_program_address` (to derive the
//!   treasury PDA from the still-TBD `TREASURY_PROGRAM_ID`)
//! - `solana-sdk::account::AccountSharedData` (genesis account encoding)
//!
//! These are heavy dependencies (~hundreds of crates transitively) and live in
//! the `solana-program-library` / agave workspace. Pulling them into a
//! workspace-internal tool would balloon build times by 10x+, so we defer to the
//! agave-side wiring step that already pays that cost.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::composed::ComposedGenesis;
use staccana_genesis::{FeeRateGovernor, GenesisOutput, MerkleRoot, Treasury};

/// Serde-friendly mirror of `staccana_genesis::GenesisOutput`.
///
/// `GenesisOutput` itself does not derive `Serialize`/`Deserialize` (its inner
/// fields are all serde-friendly, but the outer struct isn't). Rather than touch
/// the upstream crate, we mirror the layout here with the same field names so the
/// JSON encoding is the natural one a `#[derive(Serialize, Deserialize)]` on
/// `GenesisOutput` would have produced.
///
/// TODO: when `staccana-genesis` adds `Serialize`/`Deserialize` derives to
/// `GenesisOutput`, delete this DTO and use the upstream type directly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GenesisOutputDto {
    claimable_root: MerkleRoot,
    claimable_count: usize,
    treasury: Treasury,
    fee_governor: FeeRateGovernor,
    inflation_disabled: bool,
}

impl From<&GenesisOutput> for GenesisOutputDto {
    fn from(o: &GenesisOutput) -> Self {
        Self {
            claimable_root: o.claimable_root,
            claimable_count: o.claimable_count,
            treasury: o.treasury.clone(),
            fee_governor: o.fee_governor.clone(),
            inflation_disabled: o.inflation_disabled,
        }
    }
}

impl From<GenesisOutputDto> for GenesisOutput {
    fn from(d: GenesisOutputDto) -> Self {
        Self {
            claimable_root: d.claimable_root,
            claimable_count: d.claimable_count,
            treasury: d.treasury,
            fee_governor: d.fee_governor,
            inflation_disabled: d.inflation_disabled,
        }
    }
}

/// Read a [`GenesisOutput`] from JSON on disk.
///
/// The on-disk format is whatever `tools/snapshot-fork/` writes; for v0 we assume
/// pretty-printed JSON via `serde_json`. Bincode is a future compatible option:
/// every inner type is `Serialize`/`Deserialize` so adding a bincode loader is a
/// small follow-up.
pub fn load_genesis_output(path: impl AsRef<Path>) -> Result<GenesisOutput> {
    let path = path.as_ref();
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading genesis output from {}", path.display()))?;
    let dto: GenesisOutputDto = serde_json::from_str(&raw)
        .with_context(|| format!("parsing genesis output JSON from {}", path.display()))?;
    Ok(dto.into())
}

/// Write a [`GenesisOutput`] to JSON on disk. Symmetric counterpart to
/// [`load_genesis_output`]; primarily used in tests.
pub fn write_genesis_output(output: &GenesisOutput, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let dto: GenesisOutputDto = output.into();
    let json = serde_json::to_string_pretty(&dto)
        .context("serializing genesis output to JSON")?;
    std::fs::write(path, json)
        .with_context(|| format!("writing genesis output to {}", path.display()))?;
    Ok(())
}

/// Write a [`ComposedGenesis`] to disk as pretty-printed JSON.
///
/// TODO: replace (or augment) with real `genesis.bin` emission once the agave
/// fork's bootstrap surface is available — see module-level TODO above.
pub fn write_composed_genesis(composed: &ComposedGenesis, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let json = serde_json::to_string_pretty(composed)
        .context("serializing composed genesis to JSON")?;
    std::fs::write(path, json)
        .with_context(|| format!("writing composed genesis to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::compose;
    use staccana_genesis::ClassicDefaults;
    use solana_program::hash::Hash;

    fn synthetic_output() -> GenesisOutput {
        let mut treasury = Treasury::new();
        treasury.credit(123_456_789);
        GenesisOutput {
            claimable_root: MerkleRoot(Hash::new_from_array([0x42; 32])),
            claimable_count: 9,
            treasury,
            fee_governor: ClassicDefaults::fee_rate_governor(),
            inflation_disabled: ClassicDefaults::inflation_disabled(),
        }
    }

    #[test]
    fn composed_genesis_round_trips_through_json() {
        let composed = compose(&synthetic_output());
        let json = serde_json::to_string(&composed).expect("serialize");
        let back: ComposedGenesis = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(composed, back);
    }

    #[test]
    fn genesis_output_dto_round_trips_through_json() {
        let out = synthetic_output();
        let dto: GenesisOutputDto = (&out).into();
        let json = serde_json::to_string(&dto).expect("serialize");
        let back: GenesisOutputDto = serde_json::from_str(&json).expect("deserialize");
        let back_out: GenesisOutput = back.into();
        assert_eq!(out, back_out);
    }

    /// Build a unique temp directory for a test. `std::process::id()` alone is
    /// not enough because cargo runs tests within a single process and a fixed
    /// directory name would collide between parallel test threads.
    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "staccana-genesis-emit-{}-{}-{}",
            label,
            std::process::id(),
            n,
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_then_load_via_disk() {
        let composed = compose(&synthetic_output());

        let dir = unique_temp_dir("write-load");
        let path = dir.join("composed.json");

        write_composed_genesis(&composed, &path).expect("write");
        let raw = std::fs::read_to_string(&path).expect("read");
        let back: ComposedGenesis = serde_json::from_str(&raw).expect("deserialize");

        assert_eq!(composed, back);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn load_genesis_output_round_trips_through_disk() {
        let out = synthetic_output();
        let dir = unique_temp_dir("load-roundtrip");
        let path = dir.join("genesis-output.json");

        write_genesis_output(&out, &path).expect("write");
        let back = load_genesis_output(&path).expect("load");
        assert_eq!(out, back);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}

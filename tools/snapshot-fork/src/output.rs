//! Serialize and write [`staccana_genesis::GenesisOutput`] to disk.
//!
//! `GenesisOutput` itself doesn't impl `Serialize` (the genesis crate has no
//! `serde_json` / `bincode` dep), so we project it into a local
//! [`SerializableGenesis`] DTO that mirrors the public field set 1:1. This
//! keeps the genesis crate lean and makes the on-disk schema something this
//! tool owns and can evolve.
//!
//! ## Output formats
//!
//! * **JSON** (`--format json`): pretty-printed, human-inspectable. Use for
//!   debugging, golden fixtures, and small snapshots. The Merkle root is
//!   serialized as a base58 string for convenience; treasury fields are
//!   plain numbers.
//! * **Bincode** (`--format bincode`): compact, deterministic, fast to
//!   re-read. Use for handoff to the validator-bootstrap tool that produces
//!   the actual genesis.bin.
//!
//! ## Determinism
//!
//! Both formats are byte-for-byte deterministic given the same input. JSON
//! key order is fixed by struct field order via `serde`. Bincode is
//! length-prefixed and field-ordered so determinism is mechanical.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use staccana_genesis::{
    FeeRateGovernor, GenesisOutput, MerkleRoot, Treasury, BURN_PERCENT,
    FIXED_TRANSACTION_FEE_LAMPORTS, VOTE_TRANSACTION_FEE_LAMPORTS,
};

/// Output format flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Bincode,
}

/// On-disk DTO for [`GenesisOutput`]. Mirrors its public fields plus a small
/// header for forward-compat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializableGenesis {
    /// Schema version. Bump on any breaking change to this struct's layout.
    pub schema_version: u32,
    /// Merkle root over the claimable partition.
    pub claimable_root: MerkleRoot,
    /// Number of accounts in the claimable partition.
    pub claimable_count: u64,
    /// Treasury accumulator.
    pub treasury: Treasury,
    /// Classic v1 fee governor (fixed-fee model).
    pub fee_governor: FeeRateGovernor,
    /// Whether inflation is disabled.
    pub inflation_disabled: bool,
    /// Echo of the static economic constants used to build `fee_governor`.
    /// Redundant with `fee_governor` content but useful as a sanity check
    /// when reading the file by hand.
    pub constants: GenesisConstants,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisConstants {
    pub fixed_transaction_fee_lamports: u64,
    pub vote_transaction_fee_lamports: u64,
    pub burn_percent: u8,
}

impl GenesisConstants {
    fn from_classic_defaults() -> Self {
        Self {
            fixed_transaction_fee_lamports: FIXED_TRANSACTION_FEE_LAMPORTS,
            vote_transaction_fee_lamports: VOTE_TRANSACTION_FEE_LAMPORTS,
            burn_percent: BURN_PERCENT,
        }
    }
}

const SCHEMA_VERSION: u32 = 1;

impl SerializableGenesis {
    /// Project a [`GenesisOutput`] into the on-disk DTO.
    pub fn from_output(out: &GenesisOutput) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            claimable_root: out.claimable_root,
            claimable_count: out.claimable_count as u64,
            treasury: out.treasury.clone(),
            fee_governor: out.fee_governor.clone(),
            inflation_disabled: out.inflation_disabled,
            constants: GenesisConstants::from_classic_defaults(),
        }
    }

    /// Reconstruct a `GenesisOutput` from the DTO. The two are isomorphic
    /// for the fields the genesis crate exposes; `constants` is dropped on
    /// the way back since it's redundant with `fee_governor`.
    ///
    /// Used by tests; production tooling reads `SerializableGenesis`
    /// directly.
    pub fn into_output(self) -> GenesisOutput {
        // If a file was written with a fee_governor that disagrees with the
        // recorded `constants`, trust the fee_governor field — it's what the
        // builder actually used.
        GenesisOutput {
            claimable_root: self.claimable_root,
            claimable_count: self.claimable_count as usize,
            treasury: self.treasury,
            fee_governor: self.fee_governor,
            inflation_disabled: self.inflation_disabled,
        }
    }
}

/// Encode `GenesisOutput` in the requested format and write it to `path`.
pub fn write_to_path(out: &GenesisOutput, path: &Path, format: OutputFormat) -> Result<()> {
    let bytes = encode(out, format)?;
    std::fs::write(path, bytes)
        .with_context(|| format!("writing genesis output to {}", path.display()))?;
    Ok(())
}

/// Encode without writing — useful for tests and for piping to stdout.
pub fn encode(out: &GenesisOutput, format: OutputFormat) -> Result<Vec<u8>> {
    let dto = SerializableGenesis::from_output(out);
    match format {
        OutputFormat::Json => serde_json::to_vec_pretty(&dto)
            .context("serializing genesis output as JSON"),
        OutputFormat::Bincode => bincode::serialize(&dto)
            .context("serializing genesis output as bincode"),
    }
}

/// Decode previously-written bytes back into the DTO.
pub fn decode(bytes: &[u8], format: OutputFormat) -> Result<SerializableGenesis> {
    match format {
        OutputFormat::Json => serde_json::from_slice(bytes)
            .context("parsing genesis output as JSON"),
        OutputFormat::Bincode => bincode::deserialize(bytes)
            .context("parsing genesis output as bincode"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::pubkey::Pubkey;
    use staccana_genesis::{build_genesis, Account, SYSTEM_PROGRAM_ID};

    struct TestAccount {
        pubkey: Pubkey,
        owner: Pubkey,
        data_len: usize,
        lamports: u64,
    }

    impl Account for TestAccount {
        fn pubkey(&self) -> &Pubkey {
            &self.pubkey
        }
        fn owner(&self) -> &Pubkey {
            &self.owner
        }
        fn data_len(&self) -> usize {
            self.data_len
        }
        fn lamports(&self) -> u64 {
            self.lamports
        }
    }

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn sample_output() -> GenesisOutput {
        let accts = vec![
            TestAccount {
                pubkey: pk(1),
                owner: SYSTEM_PROGRAM_ID,
                data_len: 0,
                lamports: 1_000_000_000,
            },
            TestAccount {
                pubkey: pk(2),
                owner: SYSTEM_PROGRAM_ID,
                data_len: 0,
                lamports: 2_000_000_000,
            },
            TestAccount {
                pubkey: pk(3),
                owner: pk(99),
                data_len: 165,
                lamports: 2_039_280,
            },
        ];
        build_genesis(accts)
    }

    #[test]
    fn json_roundtrip_preserves_fields() {
        let out = sample_output();
        let bytes = encode(&out, OutputFormat::Json).expect("encode");
        let dto = decode(&bytes, OutputFormat::Json).expect("decode");
        let restored = dto.into_output();
        assert_eq!(restored, out);
    }

    #[test]
    fn bincode_roundtrip_preserves_fields() {
        let out = sample_output();
        let bytes = encode(&out, OutputFormat::Bincode).expect("encode");
        let dto = decode(&bytes, OutputFormat::Bincode).expect("decode");
        let restored = dto.into_output();
        assert_eq!(restored, out);
    }

    #[test]
    fn json_is_human_readable_pretty() {
        let out = sample_output();
        let bytes = encode(&out, OutputFormat::Json).expect("encode");
        let s = std::str::from_utf8(&bytes).expect("utf8");
        // Pretty-printed → multi-line.
        assert!(s.contains('\n'));
        assert!(s.contains("\"schema_version\""));
        assert!(s.contains("\"claimable_count\""));
        assert!(s.contains("\"treasury\""));
        assert!(s.contains("\"fee_governor\""));
    }

    #[test]
    fn bincode_is_compact() {
        let out = sample_output();
        let json = encode(&out, OutputFormat::Json).expect("encode json");
        let bincode = encode(&out, OutputFormat::Bincode).expect("encode bincode");
        // Bincode of this fixed structure is well under the JSON pretty output.
        assert!(bincode.len() < json.len());
    }

    #[test]
    fn write_to_path_round_trips_via_disk() {
        let out = sample_output();
        let f = tempfile::Builder::new()
            .suffix(".bincode")
            .tempfile()
            .expect("tempfile");
        write_to_path(&out, f.path(), OutputFormat::Bincode).expect("write");
        let bytes = std::fs::read(f.path()).expect("read back");
        let dto = decode(&bytes, OutputFormat::Bincode).expect("decode");
        assert_eq!(dto.into_output(), out);
    }

    #[test]
    fn dto_records_classic_constants() {
        let out = sample_output();
        let dto = SerializableGenesis::from_output(&out);
        assert_eq!(dto.constants.fixed_transaction_fee_lamports, 27_000_000);
        assert_eq!(dto.constants.vote_transaction_fee_lamports, 5_000);
        assert_eq!(dto.constants.burn_percent, 50);
    }
}

//! CLI argument parsing and top-level [`run`] entrypoint.
//!
//! Kept in the library (rather than `main.rs`) so integration tests can drive
//! the whole pipeline through the same code path the binary uses.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use staccana_genesis::{build_genesis, build_genesis_with_tree, GenesisOutput};

use crate::mock::MockSnapshot;
use crate::output::{write_to_path, OutputFormat};
use crate::shards::emit_shards;
use crate::solana::SolanaSnapshot;
use crate::source::SnapshotSource;

/// `staccana-snapshot-fork` CLI args.
#[derive(Parser, Clone, Debug)]
#[command(
    name = "staccana-snapshot-fork",
    about = "Partition a Solana mainnet snapshot into the Staccana genesis output (Merkle root + treasury + classic defaults).",
    version
)]
pub struct Args {
    /// Path to the snapshot input. For `--source mock`, a JSON fixture (see
    /// `crate::mock` docs). For `--source solana`, a `.tar.zst` snapshot
    /// archive — see `crate::solana` for the resource cost on mainnet.
    #[arg(long)]
    pub snapshot: PathBuf,

    /// Path to write the resulting `GenesisOutput` to.
    #[arg(long)]
    pub output: PathBuf,

    /// Output encoding.
    #[arg(long, value_enum, default_value_t = Format::Json)]
    pub format: Format,

    /// Snapshot source implementation.
    #[arg(long, value_enum, default_value_t = SourceKind::Mock)]
    pub source: SourceKind,

    /// If set, write 4096 sharded `.jsonl` files (one line per claimable
    /// leaf, with its Merkle inclusion proof) into this directory.
    ///
    /// Shard filename: the first 3 hex chars of the leaf's pubkey bytes,
    /// e.g. `7af.jsonl`. Empty shards are still written (zero-byte) so the
    /// edge function can issue deterministic GETs for any shard id.
    ///
    /// This adds significant memory cost — see [`MerkleTreeWithLayers`] in
    /// `staccana-genesis` — and only matters for the production index build.
    #[arg(long)]
    pub emit_shards_dir: Option<PathBuf>,
}

/// Output format flag, exposed as a clap-friendly enum so the help text
/// renders the variants automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum Format {
    Json,
    Bincode,
}

impl From<Format> for OutputFormat {
    fn from(f: Format) -> Self {
        match f {
            Format::Json => OutputFormat::Json,
            Format::Bincode => OutputFormat::Bincode,
        }
    }
}

/// Snapshot source flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum SourceKind {
    /// JSON fixture (see [`crate::mock`]).
    Mock,
    /// Real `.tar.zst` Solana snapshot (see [`crate::solana`]).
    Solana,
}

/// Construct the appropriate [`SnapshotSource`] for the chosen flag.
pub fn build_source(kind: SourceKind, snapshot: PathBuf) -> Box<dyn SnapshotSource> {
    match kind {
        SourceKind::Mock => Box::new(MockSnapshot::new(snapshot)),
        SourceKind::Solana => Box::new(SolanaSnapshot::new(snapshot)),
    }
}

/// End-to-end pipeline. Loads accounts, partitions them, writes the result.
///
/// When `args.emit_shards_dir` is set, also retains the full Merkle tree and
/// emits one `.jsonl` shard file per first-3-hex-char bucket of the leaf
/// pubkey, each line carrying `{pubkey, lamports, leafIndex, proof}` so the
/// edge function can serve a single-leaf inclusion proof from blob storage.
pub fn run(args: Args) -> Result<RunReport> {
    let Args {
        snapshot,
        output: output_path,
        format,
        source,
        emit_shards_dir,
    } = args;

    let source = build_source(source, snapshot);
    let accounts = source.accounts()?;
    let format: OutputFormat = format.into();

    let (output, shards_emitted) = match emit_shards_dir {
        Some(dir) => {
            let (output, tree) = build_genesis_with_tree(accounts);
            let count = emit_shards(&dir, &tree)?;
            (output, Some(count))
        }
        None => (build_genesis(accounts), None),
    };

    write_to_path(&output, &output_path, format)?;
    Ok(RunReport {
        output,
        output_path,
        format,
        shards_emitted,
    })
}

/// Summary of a successful [`run`] — useful for the binary's stdout log and
/// for integration tests that want to assert on what was produced.
#[derive(Debug)]
pub struct RunReport {
    pub output: GenesisOutput,
    pub output_path: PathBuf,
    pub format: OutputFormat,
    /// Number of leaves written across all shards, when `--emit-shards-dir`
    /// was set. `None` when shard emission was skipped.
    pub shards_emitted: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::pubkey::Pubkey;
    use staccana_genesis::SYSTEM_PROGRAM_ID;
    use std::io::Write;

    fn b58(bytes: [u8; 32]) -> String {
        bs58::encode(bytes).into_string()
    }

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn write_json_fixture(records: &[(Pubkey, Pubkey, u64, u64)]) -> tempfile::NamedTempFile {
        let mut s = String::from("[\n");
        for (i, (p, owner, data_len, lamports)) in records.iter().enumerate() {
            if i > 0 {
                s.push_str(",\n");
            }
            s.push_str(&format!(
                "  {{\"pubkey\":\"{}\",\"owner\":\"{}\",\"data_len\":{},\"lamports\":{}}}",
                b58(p.to_bytes()),
                b58(owner.to_bytes()),
                data_len,
                lamports
            ));
        }
        s.push_str("\n]\n");
        let mut f = tempfile::Builder::new()
            .suffix(".json")
            .tempfile()
            .expect("tempfile");
        f.write_all(s.as_bytes()).expect("write fixture");
        f
    }

    fn out_tempfile(suffix: &str) -> tempfile::NamedTempFile {
        tempfile::Builder::new()
            .suffix(suffix)
            .tempfile()
            .expect("tempfile")
    }

    #[test]
    fn end_to_end_via_mock_json_json_format() {
        // Three claimable EOAs (system-owned, zero data) and two treasury
        // accounts (token program-owned, has data).
        let token_program = pk(99);
        let fixture = write_json_fixture(&[
            (pk(1), SYSTEM_PROGRAM_ID, 0, 1_000_000_000),
            (pk(2), SYSTEM_PROGRAM_ID, 0, 2_000_000_000),
            (pk(3), token_program, 165, 2_039_280),
            (pk(4), token_program, 165, 2_039_280),
            (pk(5), SYSTEM_PROGRAM_ID, 0, 500_000_000),
        ]);
        let out_file = out_tempfile(".json");
        let args = Args {
            snapshot: fixture.path().to_path_buf(),
            output: out_file.path().to_path_buf(),
            format: Format::Json,
            source: SourceKind::Mock,
            emit_shards_dir: None,
        };

        let report = run(args).expect("pipeline succeeds");

        // Partition routing.
        assert_eq!(report.output.claimable_count, 3);
        assert_eq!(report.output.treasury.account_count(), 2);
        assert_eq!(
            report.output.treasury.total_lamports(),
            2 * 2_039_280
        );
        // Classic defaults survived.
        assert!(report.output.inflation_disabled);
        assert_eq!(report.output.fee_governor.burn_percent, 50);
        assert_eq!(
            report.output.fee_governor.min_lamports_per_signature,
            27_000_000
        );

        // The output file was written and is parseable.
        let bytes = std::fs::read(out_file.path()).expect("read output");
        let dto = crate::output::decode(&bytes, OutputFormat::Json).expect("decode");
        assert_eq!(dto.claimable_count, 3);
        assert_eq!(dto.treasury.total_lamports(), 2 * 2_039_280);
    }

    #[test]
    fn end_to_end_via_mock_json_bincode_format() {
        let fixture = write_json_fixture(&[
            (pk(10), SYSTEM_PROGRAM_ID, 0, 100),
            (pk(11), SYSTEM_PROGRAM_ID, 0, 200),
            (pk(12), pk(99), 1, 50),
        ]);
        let out_file = out_tempfile(".bincode");
        let args = Args {
            snapshot: fixture.path().to_path_buf(),
            output: out_file.path().to_path_buf(),
            format: Format::Bincode,
            source: SourceKind::Mock,
            emit_shards_dir: None,
        };

        let report = run(args).expect("pipeline succeeds");
        assert_eq!(report.output.claimable_count, 2);
        assert_eq!(report.output.treasury.account_count(), 1);
        assert_eq!(report.output.treasury.total_lamports(), 50);

        let bytes = std::fs::read(out_file.path()).expect("read");
        let dto = crate::output::decode(&bytes, OutputFormat::Bincode).expect("decode");
        assert_eq!(dto.claimable_count, 2);
    }

    #[test]
    fn solana_source_errors_clearly_when_archive_missing() {
        // We don't have a real snapshot fixture to test against in unit tests,
        // but we can confirm the source surfaces a clear error for a missing
        // archive — which is the most common operator misconfiguration.
        let out_file = out_tempfile(".bincode");
        let args = Args {
            snapshot: PathBuf::from("/nonexistent/snapshot-fork-test/snapshot.tar.zst"),
            output: out_file.path().to_path_buf(),
            format: Format::Bincode,
            source: SourceKind::Solana,
            emit_shards_dir: None,
        };
        let err = run(args).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not found"), "got: {msg}");
    }

    #[test]
    fn args_parse_with_defaults() {
        let parsed = Args::try_parse_from([
            "staccana-snapshot-fork",
            "--snapshot",
            "/tmp/s.json",
            "--output",
            "/tmp/o.json",
        ])
        .expect("parse");
        assert_eq!(parsed.snapshot, PathBuf::from("/tmp/s.json"));
        assert_eq!(parsed.output, PathBuf::from("/tmp/o.json"));
        assert_eq!(parsed.format, Format::Json);
        assert_eq!(parsed.source, SourceKind::Mock);
        assert_eq!(parsed.emit_shards_dir, None);
    }

    #[test]
    fn args_parse_emit_shards_dir() {
        let parsed = Args::try_parse_from([
            "staccana-snapshot-fork",
            "--snapshot",
            "/tmp/s.json",
            "--output",
            "/tmp/o.json",
            "--emit-shards-dir",
            "/var/cache/lazy-claim-shards",
        ])
        .expect("parse");
        assert_eq!(
            parsed.emit_shards_dir,
            Some(PathBuf::from("/var/cache/lazy-claim-shards"))
        );
    }

    #[test]
    fn end_to_end_emits_shards_when_requested() {
        let token_program = pk(99);
        let fixture = write_json_fixture(&[
            (pk(1), SYSTEM_PROGRAM_ID, 0, 1_000),
            (pk(2), SYSTEM_PROGRAM_ID, 0, 2_000),
            (pk(3), token_program, 165, 2_039_280), // treasury
            (pk(4), SYSTEM_PROGRAM_ID, 0, 4_000),
        ]);
        let out_file = out_tempfile(".json");
        let shard_dir = tempfile::tempdir().expect("tempdir");
        let args = Args {
            snapshot: fixture.path().to_path_buf(),
            output: out_file.path().to_path_buf(),
            format: Format::Json,
            source: SourceKind::Mock,
            emit_shards_dir: Some(shard_dir.path().to_path_buf()),
        };

        let report = run(args).expect("pipeline succeeds");
        assert_eq!(report.output.claimable_count, 3);
        assert_eq!(report.shards_emitted, Some(3));

        // 4096 shard files exist, but most are empty.
        let entries: Vec<_> = std::fs::read_dir(shard_dir.path())
            .expect("readdir")
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 4096);
        let total_size: u64 = entries
            .iter()
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        assert!(total_size > 0, "expected at least one non-empty shard");
    }

    #[test]
    fn args_parse_with_explicit_flags() {
        let parsed = Args::try_parse_from([
            "staccana-snapshot-fork",
            "--snapshot",
            "/tmp/s.tar.zst",
            "--output",
            "/tmp/o.bin",
            "--format",
            "bincode",
            "--source",
            "solana",
        ])
        .expect("parse");
        assert_eq!(parsed.format, Format::Bincode);
        assert_eq!(parsed.source, SourceKind::Solana);
    }
}

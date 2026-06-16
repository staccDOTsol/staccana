//! `staccana-megadrop-snapshot` CLI binary.
//!
//! Tiny shim — all of the heavy lifting (CLI argument parsing, the snapshot →
//! allocation → Merkle pipeline, the four output files) lives in
//! [`staccana_megadrop_snapshot::cli`] so integration tests can drive the same code
//! path the binary uses.

use anyhow::Result;
use clap::Parser;

use staccana_megadrop_snapshot::cli::{run, Args};

fn main() -> Result<()> {
    let args = Args::parse();
    let report = run(args)?;
    println!(
        "wrote {} allocations ({} holders) to {}, root = {}",
        report.allocation_count,
        report.holder_count,
        report.output_dir.display(),
        report.summary.root_hex,
    );
    Ok(())
}

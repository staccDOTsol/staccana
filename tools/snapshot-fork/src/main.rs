//! Binary entrypoint for `staccana-snapshot-fork`.
//!
//! All real logic lives in [`staccana_snapshot_fork::cli::run`] so the
//! pipeline can be exercised end-to-end from integration tests.

use anyhow::Result;
use clap::Parser;
use staccana_snapshot_fork::{run, Args};

fn main() -> Result<()> {
    let args = Args::parse();
    let report = run(args)?;

    // Concise stdout summary. Anyone wanting structured output should read
    // back the file directly.
    println!(
        "wrote {format:?} to {path}",
        format = report.format,
        path = report.output_path.display()
    );
    println!("  claimable_count : {}", report.output.claimable_count);
    println!(
        "  treasury_lamports: {} ({} accounts)",
        report.output.treasury.total_lamports(),
        report.output.treasury.account_count()
    );
    println!(
        "  inflation_disabled: {}",
        report.output.inflation_disabled
    );
    if let Some(n) = report.shards_emitted {
        println!("  shards_emitted   : {n} leaves");
    }
    Ok(())
}

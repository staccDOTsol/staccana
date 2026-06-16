//! `staccana-genesis-emit` CLI.
//!
//! Reads a `GenesisOutput` JSON file (produced by `tools/snapshot-fork/`) and
//! writes a `ComposedGenesis` JSON file ready for hand-off to the (eventual)
//! agave fork's genesis bootstrap.
//!
//! Usage:
//!
//! ```text
//! staccana-genesis-emit \
//!     --input  /path/to/genesis-output.json \
//!     --output /path/to/composed-genesis.json
//! ```

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use staccana_genesis_emit::{compose, load_genesis_output, write_composed_genesis};

#[derive(Parser, Debug)]
#[command(
    name = "staccana-genesis-emit",
    about = "Compose a GenesisOutput into a ComposedGenesis for the agave fork bootstrap.",
    version
)]
struct Cli {
    /// Path to the input `GenesisOutput` JSON file (produced by
    /// `tools/snapshot-fork/`).
    #[arg(short, long)]
    input: PathBuf,

    /// Path where the composed genesis JSON should be written. Parent directory
    /// must already exist.
    #[arg(short, long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let genesis_output = load_genesis_output(&cli.input)?;
    let composed = compose(&genesis_output);
    write_composed_genesis(&composed, &cli.output)?;

    eprintln!(
        "wrote composed genesis: {} claimable leaves, {} treasury accounts ({} lamports), {} active feature gates",
        composed.claimable_count,
        composed.treasury_account_count,
        composed.treasury_pda_lamports,
        composed.active_feature_gates.len(),
    );

    Ok(())
}

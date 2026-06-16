//! CLI argument parsing and top-level [`run`] entrypoint.
//!
//! Kept in the library (rather than `main.rs`) so integration tests can drive the
//! whole pipeline through the same code path the binary uses.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use solana_sdk::pubkey::Pubkey;

use crate::allocate::{compute_allocations, AllocationModel};
use crate::das::HttpDasClient;
use crate::output::{write_outputs, MerkleSummary};
use crate::snapshot::collect_holders;

/// `staccana-megadrop-snapshot` CLI args.
#[derive(Parser, Clone, Debug)]
#[command(
    name = "staccana-megadrop-snapshot",
    about = "Snapshot based_stacc_0 + proofv3 holders, compute per-holder allocations, build the megadrop Merkle tree.",
    version
)]
pub struct Args {
    /// `based_stacc_0` Metaplex NFT collection address (verified collection key).
    /// Default: per docs/MEGADROP.md.
    #[arg(
        long,
        default_value = "Ej1jbbw7QKgC9XMmWPxKFipMLJY5oVNd3rdbE1TzjNdz"
    )]
    pub based_stacc_0_collection: Pubkey,

    /// `proofv3` Token-22 mint address. Default: per docs/MEGADROP.md.
    #[arg(long, default_value = "CLWeikxiw8pC9JEtZt14fqDzYfXF7uVwLuvnJPkrE7av")]
    pub proofv3_mint: Pubkey,

    /// Total lamports to distribute across all holders. Default: 300M SOL.
    #[arg(long, default_value_t = 300_000_000u64.saturating_mul(1_000_000_000))]
    pub total_megadrop_lamports: u64,

    /// Cohort weight for `based_stacc_0`. Default: 60.
    #[arg(long, default_value_t = 60)]
    pub weight_based_stacc_0: u32,

    /// Cohort weight for `proofv3`. Default: 40.
    #[arg(long, default_value_t = 40)]
    pub weight_proofv3: u32,

    /// Per-holder allocation model.
    #[arg(long, value_enum, default_value_t = AllocationModel::Sqrt)]
    pub allocation_model: AllocationModel,

    /// First tranche unlock month (yyyymm). Default: May 2026 (mainnet-sigma launch).
    #[arg(long, default_value_t = 202605)]
    pub genesis_month: u32,

    /// Mainnet RPC URL. DAS-aware endpoint preferred (e.g. Helius); the public
    /// endpoint will reject `getAssetsByGroup`.
    #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
    pub mainnet_rpc: String,

    /// Output directory for the four files (`allocations.json`, `merkle-root.hex`,
    /// `init-megadrop-args.json`, `proofs.json`).
    #[arg(long)]
    pub output_dir: PathBuf,
}

/// Run the full snapshot → allocation → Merkle pipeline.
pub fn run(args: Args) -> Result<RunReport> {
    let client = HttpDasClient::new(args.mainnet_rpc.clone());
    let holders = collect_holders(
        &client,
        &args.based_stacc_0_collection,
        &args.proofv3_mint,
    )?;

    let allocations = compute_allocations(
        &holders,
        args.allocation_model,
        args.weight_based_stacc_0,
        args.weight_proofv3,
        args.total_megadrop_lamports,
    );

    let summary = write_outputs(
        &allocations,
        &args.output_dir,
        args.genesis_month,
        args.total_megadrop_lamports,
    )?;

    Ok(RunReport {
        holder_count: holders.len(),
        allocation_count: allocations.len(),
        summary,
        output_dir: args.output_dir,
    })
}

/// Summary of a successful [`run`].
#[derive(Debug)]
pub struct RunReport {
    /// Number of distinct holders found across both cohorts.
    pub holder_count: usize,
    /// Number of allocation rows emitted (= holder_count).
    pub allocation_count: usize,
    /// Merkle summary.
    pub summary: MerkleSummary,
    pub output_dir: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn args_parse_with_defaults() {
        let parsed = Args::try_parse_from([
            "staccana-megadrop-snapshot",
            "--output-dir",
            "/tmp/out",
        ])
        .expect("parse");
        // Defaults from docs/MEGADROP.md.
        assert_eq!(
            parsed.based_stacc_0_collection.to_string(),
            "Ej1jbbw7QKgC9XMmWPxKFipMLJY5oVNd3rdbE1TzjNdz"
        );
        assert_eq!(
            parsed.proofv3_mint.to_string(),
            "CLWeikxiw8pC9JEtZt14fqDzYfXF7uVwLuvnJPkrE7av"
        );
        assert_eq!(parsed.weight_based_stacc_0, 60);
        assert_eq!(parsed.weight_proofv3, 40);
        assert_eq!(parsed.genesis_month, 202605);
        assert_eq!(parsed.allocation_model, AllocationModel::Sqrt);
    }

    #[test]
    fn args_parse_with_explicit_overrides() {
        let parsed = Args::try_parse_from([
            "staccana-megadrop-snapshot",
            "--output-dir",
            "/tmp/out",
            "--allocation-model",
            "linear",
            "--weight-based-stacc-0",
            "70",
            "--weight-proofv3",
            "30",
            "--total-megadrop-lamports",
            "1000000",
            "--genesis-month",
            "202607",
        ])
        .expect("parse");
        assert_eq!(parsed.allocation_model, AllocationModel::Linear);
        assert_eq!(parsed.weight_based_stacc_0, 70);
        assert_eq!(parsed.weight_proofv3, 30);
        assert_eq!(parsed.total_megadrop_lamports, 1_000_000);
        assert_eq!(parsed.genesis_month, 202607);
    }

    #[test]
    fn cli_help_renders() {
        // Smoke test: clap doesn't panic constructing the help text. Catches a bad
        // `default_value` annotation.
        let _ = Args::command().render_help();
    }
}

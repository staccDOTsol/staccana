//! `staccana-megadrop-merkle` — build the megadrop Merkle root + per-holder
//! allocation set from operator-held snapshot files.
//!
//! ## Example
//!
//! ```bash
//! staccana-megadrop-merkle \
//!   --based-stacc-holders ~/megadrop-snapshot-2026-05-02T1900Z/based_stacc_0_holders.json \
//!   --proofv3-holders     ~/megadrop-snapshot-2026-05-02T1900Z/proofv3_holders.json \
//!   --total-allocation-sol 30000000 \
//!   --base-allocation-sol 10 \
//!   --per-nft-bonus-sol 100 \
//!   --per-token-bonus-sol-per-million 5 \
//!   --output-dir /var/lib/staccana/megadrop-snapshot/round-1
//! ```
//!
//! Outputs five files into `--output-dir`. See `output::write_outputs` for the per-file
//! shape; the `merkle-root.hex` file is the byte-for-byte input to
//! `tools/megadrop-init --root <hex>`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use staccana_megadrop_merkle::{
    allocate::AllocationParams, build_tree, compute_allocations, load_based_stacc_holders,
    load_proofv3_holders, write_outputs,
};

#[derive(Parser, Debug)]
#[command(
    name = "staccana-megadrop-merkle",
    about = "Build the megadrop Merkle root + per-holder allocations from snapshot files.",
    version
)]
struct Cli {
    /// JSONL file from `tools/megadrop-snapshot` (or the operator's quick-snapshot
    /// script) listing one record per based_stacc_0 NFT (`{owner, mint}`).
    #[arg(long)]
    based_stacc_holders: PathBuf,

    /// JSONL file listing one record per proofv3 token account
    /// (`{owner, balance, mint, ata}`).
    #[arg(long)]
    proofv3_holders: PathBuf,

    /// Total round pool in SOL. Multiplied by 1e9 to get the exact target lamport
    /// total — the per-leaf lamport sum will equal this exactly after residue
    /// distribution.
    #[arg(long)]
    total_allocation_sol: u64,

    /// Floor allocation, in SOL. Every eligible holder (present in either cohort)
    /// gets this much before bonuses.
    #[arg(long)]
    base_allocation_sol: u64,

    /// Per-NFT bonus, in SOL. Multiplied by the holder's NFT count from
    /// `based_stacc_0`.
    #[arg(long)]
    per_nft_bonus_sol: u64,

    /// Per-million-token bonus, in SOL. A holder with 1M proofv3 tokens gets exactly
    /// this much extra; sub-million holders contribute zero (floor division on
    /// `balance / 1_000_000`).
    #[arg(long)]
    per_token_bonus_sol_per_million: u64,

    /// Directory where the five output files are written. Created if missing.
    #[arg(long)]
    output_dir: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    eprintln!(
        "[megadrop-merkle] inputs: based={} proofv3={}",
        cli.based_stacc_holders.display(),
        cli.proofv3_holders.display()
    );
    let nft_counts = load_based_stacc_holders(&cli.based_stacc_holders)
        .with_context(|| format!("load {}", cli.based_stacc_holders.display()))?;
    let token_balances = load_proofv3_holders(&cli.proofv3_holders)
        .with_context(|| format!("load {}", cli.proofv3_holders.display()))?;

    eprintln!(
        "[megadrop-merkle] cohort sizes: based_stacc holders={} proofv3 holders={}",
        nft_counts.len(),
        token_balances.len()
    );

    let params = AllocationParams {
        total_allocation_sol: cli.total_allocation_sol,
        base_allocation_sol: cli.base_allocation_sol,
        per_nft_bonus_sol: cli.per_nft_bonus_sol,
        per_token_bonus_sol_per_million: cli.per_token_bonus_sol_per_million,
    };
    let allocations = compute_allocations(&nft_counts, &token_balances, params);
    eprintln!(
        "[megadrop-merkle] computed {} per-holder allocations",
        allocations.len()
    );

    let tree = build_tree(&allocations);
    let target_total: u128 = (params.total_allocation_sol as u128) * 1_000_000_000;
    let outputs = write_outputs(&allocations, &tree, target_total, &cli.output_dir)?;

    eprintln!(
        "[megadrop-merkle] wrote outputs to {}: root={} leaf_count={} total_lamports={} gini={:.4}",
        cli.output_dir.display(),
        outputs.root_hex,
        outputs.leaf_count,
        outputs.total_allocation_lamports,
        outputs.gini,
    );

    // Bail if the actual total drifted from the target — should never happen given
    // the residue-distribution loop, but a CI assertion is cheap insurance.
    if outputs.total_allocation_lamports != target_total {
        anyhow::bail!(
            "internal error: per-leaf sum {} != target {} (off by {})",
            outputs.total_allocation_lamports,
            target_total,
            (outputs.total_allocation_lamports as i128 - target_total as i128)
        );
    }

    Ok(())
}

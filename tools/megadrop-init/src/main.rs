//! `staccana-megadrop-init` — one-shot CLI to invoke the megadrop program's
//! `init_megadrop` instruction, creating the singleton `MegadropConfig` PDA.
//!
//! After step 40 deploys the megadrop .so, the on-chain config still has to be
//! initialized once before any user can claim. This binary does that — pass the
//! deployed program ID + the genesis month + the snapshot Merkle root + the
//! treasury authority, and it sends a single tx as the `--keypair` signer.
//!
//! For tonight's devnet shake-out the snapshot Merkle root can be all-zero
//! (`--placeholder-root`); the on-chain validator only checks plausibility of
//! `genesis_month` and that `total_allocation_lamports > 0`. Real allocation
//! data lands in a follow-up `init_megadrop` re-run with the real root once
//! `tools/megadrop-snapshot` has produced it (the program's `init_megadrop`
//! instruction creates the PDA — it's a one-shot, so for re-init you'd close
//! and re-create, or land a `update_megadrop` ix in v1.1).
//!
//! Usage:
//!   staccana-megadrop-init \
//!     --keypair /etc/staccana/keys/identity.json \
//!     --rpc http://localhost:8899 \
//!     --program-id Aicff1zk6b5ifYzFoyhenUD5ehhFYb8GiDbRCrWt9t34 \
//!     --treasury-authority <pubkey> \
//!     --genesis-month 202605 \
//!     --total-allocation-sol 30000000 \
//!     --placeholder-root

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use borsh::BorshSerialize;
use clap::Parser;
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Signer};
use solana_sdk::system_program;
use solana_sdk::transaction::Transaction;

/// Anchor instruction discriminator: first 8 bytes of sha256("global:<name>").
fn anchor_discriminator(name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{}", name).as_bytes());
    let out = h.finalize();
    let mut d = [0u8; 8];
    d.copy_from_slice(&out[..8]);
    d
}

fn init_megadrop_discriminator() -> [u8; 8] {
    anchor_discriminator("init_megadrop")
}

fn update_megadrop_discriminator() -> [u8; 8] {
    anchor_discriminator("update_megadrop")
}

/// Borsh layout for `update_megadrop`'s args. Each field is `Option<...>`
/// — Borsh encodes Option as `[tag:u8, value]` where tag=0 means None and
/// tag=1 means Some(value). We always emit the full 4-tuple so the wire
/// payload is deterministic regardless of which fields the operator wants
/// to patch.
#[derive(BorshSerialize)]
struct UpdateMegadropArgs {
    claimable_root: Option<[u8; 32]>,
    genesis_month: Option<u32>,
    total_allocation_lamports: Option<u64>,
    treasury_authority: Option<[u8; 32]>,
}

/// Borsh-equivalent layout of `InitMegadropArgs` per
/// `programs/megadrop/src/instructions/init_megadrop.rs`.
#[derive(BorshSerialize)]
struct InitMegadropArgs {
    claimable_root: [u8; 32],
    genesis_month: u32,
    total_allocation_lamports: u64,
    treasury_authority: [u8; 32],
}

/// `["megadrop_config"]` PDA seed, matches `programs/megadrop/src/state.rs::MEGADROP_CONFIG_SEED`.
const MEGADROP_CONFIG_SEED: &[u8] = b"megadrop_config";

#[derive(Parser, Debug)]
#[command(name = "staccana-megadrop-init", about = "init MegadropConfig PDA on-chain")]
struct Cli {
    /// Fee payer + authority for the init ix.
    #[arg(long)]
    keypair: PathBuf,

    /// RPC URL (e.g. http://localhost:8899 or https://rpc.mp.fun).
    #[arg(long, default_value = "http://localhost:8899")]
    rpc: String,

    /// Deployed megadrop program ID (from /etc/staccana/program-ids.json).
    #[arg(long)]
    program_id: String,

    /// Treasury authority PDA (signer for treasury debits during claim). Typically the
    /// validator-subsidy program's treasury PDA, or a governance multisig.
    #[arg(long)]
    treasury_authority: String,

    /// Genesis month, ISO yyyymm (e.g. 202605 = May 2026).
    #[arg(long, default_value_t = 202605)]
    genesis_month: u32,

    /// Total megadrop allocation in SOL (gets multiplied by 1_000_000_000 for lamports).
    /// docs/MEGADROP.md locks this at 30M for the v1 design.
    #[arg(long, default_value_t = 30_000_000)]
    total_allocation_sol: u64,

    /// Use an all-zero placeholder Merkle root. For tonight's devnet only — replace
    /// with `--root <hex>` once `tools/megadrop-snapshot` has produced the real root.
    #[arg(long, conflicts_with = "root")]
    placeholder_root: bool,

    /// Hex-encoded 32-byte Merkle root from `tools/megadrop-snapshot`.
    #[arg(long, conflicts_with = "placeholder_root")]
    root: Option<String>,

    /// Path to `allocations.json` (the file emitted by `tools/megadrop-merkle`,
    /// or in production: `frontend/public/megadrop/allocations.json`). When
    /// provided, the tool computes the Merkle root locally via the same
    /// `staccana_megadrop_merkle::build_tree` the page uses, instead of
    /// requiring the operator to pass `--root` from a precomputed
    /// `merkle-root.hex`. Conflicts with `--root` and `--placeholder-root`.
    #[arg(long, conflicts_with_all = ["placeholder_root", "root"])]
    allocations_json: Option<PathBuf>,

    /// Send `update_megadrop` instead of `init_megadrop`. Used to patch the
    /// claimable_root post-deploy when the PDA was originally seeded with
    /// `--placeholder-root` and a real snapshot has since landed. The PDA
    /// must already exist (init must have run before) — the program rejects
    /// update_megadrop on an uninitialized PDA via the seed-check.
    #[arg(long)]
    update: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let payer = read_keypair_file(&cli.keypair)
        .map_err(|e| anyhow!("reading keypair {}: {}", cli.keypair.display(), e))?;
    let program_id = Pubkey::from_str(&cli.program_id).context("parsing --program-id")?;
    let treasury_authority =
        Pubkey::from_str(&cli.treasury_authority).context("parsing --treasury-authority")?;

    let claimable_root: [u8; 32] = if cli.placeholder_root {
        eprintln!("[init] WARNING: using all-zero placeholder Merkle root. Re-init with the real root before mainnet.");
        [0u8; 32]
    } else if let Some(path) = cli.allocations_json.as_ref() {
        // Read allocations.json. The on-disk shape encodes `owner` as a
        // base58 STRING (it's emitted by `tools/megadrop-merkle::output::write_outputs`
        // via solana_sdk::pubkey::Pubkey's Display impl + serde's default
        // string serializer), but solana_sdk::pubkey::Pubkey's Deserialize
        // impl expects a 32-byte array. Use a local wrapper struct that
        // parses the string and converts to HolderAllocation.
        //
        // Extra top-level fields like `nft_count`/`token_balance` (the snapshot
        // tool puts them both at the top AND nested for audit reporting) are
        // ignored by serde via the default `#[serde(default)]` behavior.
        #[derive(serde::Deserialize)]
        struct WireContrib {
            #[serde(default)]
            nft_count: u64,
            #[serde(default)]
            token_balance: u64,
        }
        #[derive(serde::Deserialize)]
        struct WireRow {
            owner: String,
            lamports: u64,
            #[serde(default)]
            contributions: Option<WireContrib>,
            // Top-level fallback when `contributions` is absent.
            #[serde(default)]
            nft_count: u64,
            #[serde(default)]
            token_balance: u64,
        }
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading allocations.json {}", path.display()))?;
        let wire: Vec<WireRow> = serde_json::from_slice(&bytes)
            .context("parsing allocations.json")?;
        if wire.is_empty() {
            return Err(anyhow!("allocations.json is empty"));
        }
        let mut allocations: Vec<staccana_megadrop_merkle::HolderAllocation> =
            Vec::with_capacity(wire.len());
        for row in &wire {
            let owner = Pubkey::from_str(&row.owner)
                .with_context(|| format!("invalid base58 pubkey {}", row.owner))?;
            let contrib = row.contributions.as_ref();
            allocations.push(staccana_megadrop_merkle::HolderAllocation {
                owner,
                lamports: row.lamports,
                contributions: staccana_megadrop_merkle::HolderContributions {
                    nft_count: contrib.map(|c| c.nft_count).unwrap_or(row.nft_count),
                    token_balance: contrib
                        .map(|c| c.token_balance)
                        .unwrap_or(row.token_balance),
                },
            });
        }
        let tree = staccana_megadrop_merkle::tree::build_tree(&allocations);
        let root = tree.root.to_bytes();
        eprintln!(
            "[init] computed root from {} allocations: 0x{}",
            allocations.len(),
            hex::encode(root),
        );
        root
    } else {
        let hex = cli
            .root
            .ok_or_else(|| anyhow!("must pass --root <hex>, --allocations-json <path>, or --placeholder-root"))?;
        let bytes = hex::decode(hex.trim_start_matches("0x"))
            .context("decoding --root hex")?;
        if bytes.len() != 32 {
            return Err(anyhow!("--root must be exactly 32 bytes (got {})", bytes.len()));
        }
        let mut r = [0u8; 32];
        r.copy_from_slice(&bytes);
        r
    };

    let total_allocation_lamports = cli
        .total_allocation_sol
        .checked_mul(1_000_000_000)
        .ok_or_else(|| anyhow!("--total-allocation-sol overflow"))?;

    let (megadrop_config, _bump) =
        Pubkey::find_program_address(&[MEGADROP_CONFIG_SEED], &program_id);

    eprintln!("[init] megadrop program:    {}", program_id);
    eprintln!("[init] MegadropConfig PDA:  {}", megadrop_config);
    eprintln!("[init] payer / authority:   {}", payer.pubkey());
    eprintln!("[init] treasury authority:  {}", treasury_authority);
    eprintln!("[init] genesis_month:       {}", cli.genesis_month);
    eprintln!(
        "[init] total allocation:    {} lamports ({} SOL)",
        total_allocation_lamports, cli.total_allocation_sol
    );
    eprintln!("[init] claimable_root:      0x{}", hex::encode(claimable_root));

    // Build instruction data + accounts list.
    let (data, accounts) = if cli.update {
        // update_megadrop accepts Option<...> per field. We patch the same four
        // fields init writes — caller can choose to overwrite all of them.
        // Accounts: [authority(signer, NOT writable), megadrop_config(writable)].
        let args = UpdateMegadropArgs {
            claimable_root: Some(claimable_root),
            genesis_month: Some(cli.genesis_month),
            total_allocation_lamports: Some(total_allocation_lamports),
            treasury_authority: Some(treasury_authority.to_bytes()),
        };
        let mut data = Vec::with_capacity(8 + (1 + 32) + (1 + 4) + (1 + 8) + (1 + 32));
        data.extend_from_slice(&update_megadrop_discriminator());
        args.serialize(&mut data).context("borsh-serialize update args")?;
        let accounts = vec![
            AccountMeta::new_readonly(payer.pubkey(), true), // authority (signer, NOT writable)
            AccountMeta::new(megadrop_config, false),        // megadrop_config (writable)
        ];
        eprintln!("[update] sending update_megadrop tx — patching all four fields.");
        (data, accounts)
    } else {
        // init_megadrop: full payload + system_program for the PDA allocation.
        let args = InitMegadropArgs {
            claimable_root,
            genesis_month: cli.genesis_month,
            total_allocation_lamports,
            treasury_authority: treasury_authority.to_bytes(),
        };
        let mut data = Vec::with_capacity(8 + 32 + 4 + 8 + 32);
        data.extend_from_slice(&init_megadrop_discriminator());
        args.serialize(&mut data).context("borsh-serialize init args")?;
        let accounts = vec![
            AccountMeta::new(payer.pubkey(), true),                 // authority
            AccountMeta::new(megadrop_config, false),               // megadrop_config
            AccountMeta::new_readonly(system_program::id(), false), // system_program
        ];
        eprintln!("[init] sending init_megadrop tx...");
        (data, accounts)
    };

    let ix = Instruction { program_id, accounts, data };

    let rpc = RpcClient::new_with_commitment(cli.rpc.clone(), CommitmentConfig::confirmed());
    let blockhash = rpc.get_latest_blockhash().context("get_latest_blockhash")?;
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);

    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("send_and_confirm_transaction")?;
    eprintln!("[done] confirmed: {}", sig);
    println!("MegadropConfig PDA: {}", megadrop_config);
    Ok(())
}

//! `staccana-bridge-init` — one-shot CLI that invokes the staccana bridge
//! program's `register_asset` instruction for a single asset (stSOL, ssUSDC,
//! or wSOL).
//!
//! Each invocation initializes:
//!   - `AssetConfig`     PDA at `["asset",     asset_id_le]`
//!   - `RatioState`      PDA at `["ratio",     asset_id_le]`  (R = 1.0 Q64.64)
//!   - `NonceOutCounter` PDA at `["nonce_out", asset_id_le]`  (next_nonce = 0)
//!   - `FederationSet`   PDA at `["federation"]`              (only on first call)
//!
//! Re-running for the same asset will fail cleanly with "already in use" — the
//! `init` constraint on the AssetConfig PDA enforces single-shot semantics.
//!
//! Usage:
//!   staccana-bridge-init \
//!     --keypair /etc/staccana/keys/identity.json \
//!     --rpc http://localhost:8899 \
//!     --program-id LA7h3hjvD62MeTtdeE4h2vq3EGxbU1oqzHtewp4xb9b \
//!     --federation-pubkeys /etc/staccana/federation-pubkeys.json \
//!     --mainnet-vault-program F2AypZ8FDWnR5bdyLHzo4idof9YrBpdBmbgLwLBjLfVU \
//!     --staccana-mint <token-22 mint pubkey> \
//!     --asset wSOL

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use borsh::BorshSerialize;
use clap::Parser;
use serde::Deserialize;
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Signer};
use solana_sdk::system_program;
use solana_sdk::transaction::Transaction;

use staccana_bridge_init::{
    anchor_discriminator, lookup_asset, RegisterAssetArgs, ASSET_SEED, FEDERATION_SEED,
    MAX_FEDERATION_MEMBERS, NONCE_OUT_SEED, RATIO_SEED,
};

#[derive(Deserialize, Debug)]
struct FederationFile {
    threshold: u8,
    pubkeys: Vec<String>,
}

#[derive(Parser, Debug)]
#[command(
    name = "staccana-bridge-init",
    about = "register one asset on the staccana bridge program (creates AssetConfig + RatioState + NonceOutCounter; bootstraps FederationSet on first call)"
)]
struct Cli {
    /// Fee payer + authority for the register_asset ix.
    #[arg(long)]
    keypair: PathBuf,

    /// RPC URL. Defaults to local validator.
    #[arg(long, default_value = "http://localhost:8899")]
    rpc: String,

    /// Deployed staccana bridge program ID.
    #[arg(long)]
    program_id: String,

    /// JSON file describing the federation: `{ "threshold": M, "pubkeys": [..N base58..] }`.
    /// Only used on first-call bootstrap; subsequent calls reuse the on-chain set.
    #[arg(long)]
    federation_pubkeys: PathBuf,

    /// Mainnet bridge-vault program ID. Stored in `AssetConfig.mainnet_vault_program` so
    /// relayers + UI know where to deposit. Bridge program itself doesn't CPI into it.
    #[arg(long)]
    mainnet_vault_program: String,

    /// Token-22 staccana mint pubkey for this asset. Must already exist with mint
    /// authority set to the bridge program's per-asset PDA before this ix can mint.
    #[arg(long)]
    staccana_mint: String,

    /// Asset to register: stSOL, ssUSDC, or wSOL.
    #[arg(long)]
    asset: String,
}

fn load_federation(path: &std::path::Path) -> Result<(u8, u8, [[u8; 32]; MAX_FEDERATION_MEMBERS])> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading federation file {}", path.display()))?;
    let f: FederationFile =
        serde_json::from_str(&raw).context("parsing federation JSON (expected {threshold, pubkeys})")?;
    if f.pubkeys.is_empty() {
        return Err(anyhow!("federation pubkeys list is empty"));
    }
    if f.pubkeys.len() > MAX_FEDERATION_MEMBERS {
        return Err(anyhow!(
            "federation pubkeys ({}) exceeds MAX_FEDERATION_MEMBERS ({})",
            f.pubkeys.len(),
            MAX_FEDERATION_MEMBERS
        ));
    }
    if f.threshold == 0 || f.threshold as usize > f.pubkeys.len() {
        return Err(anyhow!(
            "threshold {} invalid for {} pubkeys",
            f.threshold,
            f.pubkeys.len()
        ));
    }
    let n = f.pubkeys.len() as u8;
    let mut members = [[0u8; 32]; MAX_FEDERATION_MEMBERS];
    for (i, p) in f.pubkeys.iter().enumerate() {
        let pk = Pubkey::from_str(p)
            .with_context(|| format!("parsing federation pubkey #{i}: {p}"))?;
        members[i] = pk.to_bytes();
    }
    Ok((f.threshold, n, members))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let payer = read_keypair_file(&cli.keypair)
        .map_err(|e| anyhow!("reading keypair {}: {}", cli.keypair.display(), e))?;
    let program_id = Pubkey::from_str(&cli.program_id).context("parsing --program-id")?;
    let mainnet_vault_program = Pubkey::from_str(&cli.mainnet_vault_program)
        .context("parsing --mainnet-vault-program")?;
    let staccana_mint =
        Pubkey::from_str(&cli.staccana_mint).context("parsing --staccana-mint")?;
    let asset_cfg = lookup_asset(&cli.asset)?;

    let (m, n, members) = load_federation(&cli.federation_pubkeys)?;

    // Derive PDAs (same scheme as the on-chain `register_asset` accounts block).
    let asset_id_le = asset_cfg.asset_id.to_le_bytes();
    let (asset_config_pda, _) =
        Pubkey::find_program_address(&[ASSET_SEED, &asset_id_le], &program_id);
    let (ratio_state_pda, _) =
        Pubkey::find_program_address(&[RATIO_SEED, &asset_id_le], &program_id);
    let (nonce_out_pda, _) =
        Pubkey::find_program_address(&[NONCE_OUT_SEED, &asset_id_le], &program_id);
    let (federation_set_pda, _) =
        Pubkey::find_program_address(&[FEDERATION_SEED], &program_id);

    eprintln!("[bridge-init] asset:                 {} (id={})", asset_cfg.label, asset_cfg.asset_id);
    eprintln!("[bridge-init] bridge program:        {program_id}");
    eprintln!("[bridge-init] payer / authority:     {}", payer.pubkey());
    eprintln!("[bridge-init] AssetConfig PDA:       {asset_config_pda}");
    eprintln!("[bridge-init] RatioState PDA:        {ratio_state_pda}");
    eprintln!("[bridge-init] NonceOutCounter PDA:   {nonce_out_pda}");
    eprintln!("[bridge-init] FederationSet PDA:     {federation_set_pda}");
    eprintln!("[bridge-init] mainnet vault program: {mainnet_vault_program}");
    eprintln!("[bridge-init] staccana mint:         {staccana_mint}");
    eprintln!(
        "[bridge-init] decimals={} mint_fee_bps={} burn_fee_bps={} flags=0b{:08b}",
        asset_cfg.decimals, asset_cfg.mint_fee_bps, asset_cfg.burn_fee_bps, asset_cfg.flags
    );
    eprintln!("[bridge-init] federation: {m}-of-{n}");

    let args = RegisterAssetArgs {
        asset_id: asset_cfg.asset_id,
        underlying_label: asset_cfg.underlying_label,
        mainnet_vault_program: mainnet_vault_program.to_bytes(),
        staccana_mint: staccana_mint.to_bytes(),
        decimals: asset_cfg.decimals,
        mint_fee_bps: asset_cfg.mint_fee_bps,
        burn_fee_bps: asset_cfg.burn_fee_bps,
        federation_m: m,
        federation_n: n,
        federation_members: members,
        flags: asset_cfg.flags,
    };

    let mut data = Vec::with_capacity(8 + 1024);
    data.extend_from_slice(&anchor_discriminator("register_asset"));
    args.serialize(&mut data).context("borsh-serialize args")?;

    // Account order matches `RegisterAsset<'info>` in
    // programs/bridge/src/instructions/register_asset.rs.
    let accounts = vec![
        AccountMeta::new(payer.pubkey(), true),                 // authority (signer, writable)
        AccountMeta::new(asset_config_pda, false),              // asset_config (PDA, writable, init)
        AccountMeta::new(ratio_state_pda, false),               // ratio_state (PDA, writable, init)
        AccountMeta::new(nonce_out_pda, false),                 // nonce_out (PDA, writable, init)
        AccountMeta::new(federation_set_pda, false),            // federation_set (PDA, writable, init_if_needed)
        AccountMeta::new_readonly(system_program::id(), false), // system_program
    ];

    let ix = Instruction { program_id, accounts, data };

    let rpc = RpcClient::new_with_commitment(cli.rpc.clone(), CommitmentConfig::confirmed());
    let blockhash = rpc.get_latest_blockhash().context("get_latest_blockhash")?;
    let tx =
        Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);

    eprintln!("[bridge-init] sending tx...");
    let sig = rpc.send_and_confirm_transaction(&tx).context(
        "send_and_confirm_transaction (re-running for the same asset fails with 'already in use' — that's expected idempotency)",
    )?;
    eprintln!("[bridge-init] confirmed: {sig}");
    println!("AssetConfig PDA initialized: {asset_config_pda}");
    Ok(())
}

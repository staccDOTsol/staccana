//! `staccana-bridge-vault-init` — one-shot CLI that invokes the mainnet
//! `bridge-vault` program's `init_vault` instruction for a single asset.
//!
//! Each invocation initializes:
//!   - `VaultConfig`    PDA at `["vault",     asset_id_le]`
//!   - `NonceInCounter` PDA at `["nonce_in",  asset_id_le]`  (next_nonce = 0)
//!   - `FederationSet`  PDA at `["federation"]`              (only on first call)
//!
//! Re-running for the same asset will fail cleanly with "already in use".
//!
//! For wSOL, no `--underlying-mint` / `--vault-token-account` is required;
//! the program enforces both must be `Pubkey::default()` when NATIVE_SOL is set.
//!
//! For stSOL / ssUSDC the underlying SPL mint and the PDA-owned ATA must be
//! supplied. If they're omitted, the binary will refuse to send the tx (since
//! the program would reject `Pubkey::default()` with `AssetKindMismatch`).
//!
//! Usage:
//!   staccana-bridge-vault-init \
//!     --keypair $HOME/.config/solana/id.json \
//!     --rpc https://api.devnet.solana.com \
//!     --program-id F2AypZ8FDWnR5bdyLHzo4idof9YrBpdBmbgLwLBjLfVU \
//!     --federation-pubkeys /etc/staccana/federation-pubkeys.json \
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

use staccana_bridge_vault_init::{
    anchor_discriminator, lookup_asset, InitVaultArgs, FEDERATION_SEED,
    MAX_FEDERATION_MEMBERS, NONCE_IN_SEED, VAULT_SEED,
};

#[derive(Deserialize, Debug)]
struct FederationFile {
    threshold: u8,
    pubkeys: Vec<String>,
}

#[derive(Parser, Debug)]
#[command(
    name = "staccana-bridge-vault-init",
    about = "init one asset's vault on the mainnet bridge-vault program"
)]
struct Cli {
    #[arg(long)]
    keypair: PathBuf,

    #[arg(long, default_value = "https://api.devnet.solana.com")]
    rpc: String,

    #[arg(long)]
    program_id: String,

    /// JSON file: `{ "threshold": M, "pubkeys": [..N base58..] }`.
    #[arg(long)]
    federation_pubkeys: PathBuf,

    /// Asset to register: stSOL, ssUSDC, or wSOL.
    #[arg(long)]
    asset: String,

    /// SPL mint of the underlying. REQUIRED for stSOL / ssUSDC; must be
    /// omitted for wSOL (the program enforces NATIVE_SOL = no underlying mint).
    #[arg(long)]
    underlying_mint: Option<String>,

    /// Vault token account (PDA-owned ATA holding the underlying). REQUIRED for
    /// stSOL / ssUSDC; must be omitted for wSOL.
    #[arg(long)]
    vault_token_account: Option<String>,
}

fn load_federation(path: &std::path::Path) -> Result<(u8, u8, Vec<[u8; 32]>)> {
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
    // Variable-length on the wire — size prefixed by Borsh as `[u32_le_len,
    // n × 32 bytes]`. For 5-of-9 that's 4 + 288 = 292 bytes vs the old
    // fixed-size 1024 bytes that blew the 1232-byte legacy tx ceiling.
    let mut members: Vec<[u8; 32]> = Vec::with_capacity(f.pubkeys.len());
    for (i, p) in f.pubkeys.iter().enumerate() {
        let pk = Pubkey::from_str(p)
            .with_context(|| format!("parsing federation pubkey #{i}: {p}"))?;
        members.push(pk.to_bytes());
    }
    Ok((f.threshold, n, members))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let payer = read_keypair_file(&cli.keypair)
        .map_err(|e| anyhow!("reading keypair {}: {}", cli.keypair.display(), e))?;
    let program_id = Pubkey::from_str(&cli.program_id).context("parsing --program-id")?;
    let asset_cfg = lookup_asset(&cli.asset)?;

    // Validate asset-kind invariants client-side so we don't waste a tx on a
    // certain-to-fail send. Mirrors the on-chain checks in init_vault.handler.
    let (underlying_mint, vault_token_account) = if asset_cfg.requires_spl_backing {
        let m = cli.underlying_mint.as_ref().ok_or_else(|| {
            anyhow!(
                "--underlying-mint is REQUIRED for {} (SPL-backed). \
                 For tonight's devnet stSOL/ssUSDC bring-up: skip this binary or pass a real mint. \
                 Passing all-zero would be rejected on-chain with AssetKindMismatch.",
                asset_cfg.label
            )
        })?;
        let v = cli.vault_token_account.as_ref().ok_or_else(|| {
            anyhow!(
                "--vault-token-account is REQUIRED for {} (PDA-owned ATA holding the underlying)",
                asset_cfg.label
            )
        })?;
        (
            Pubkey::from_str(m).context("parsing --underlying-mint")?,
            Pubkey::from_str(v).context("parsing --vault-token-account")?,
        )
    } else {
        if cli.underlying_mint.is_some() || cli.vault_token_account.is_some() {
            return Err(anyhow!(
                "{} is NATIVE_SOL — must NOT pass --underlying-mint or --vault-token-account",
                asset_cfg.label
            ));
        }
        (Pubkey::default(), Pubkey::default())
    };

    let (m, n, members) = load_federation(&cli.federation_pubkeys)?;

    let asset_id_le = asset_cfg.asset_id.to_le_bytes();
    let (vault_config_pda, _) =
        Pubkey::find_program_address(&[VAULT_SEED, &asset_id_le], &program_id);
    let (nonce_in_pda, _) =
        Pubkey::find_program_address(&[NONCE_IN_SEED, &asset_id_le], &program_id);
    let (federation_set_pda, _) =
        Pubkey::find_program_address(&[FEDERATION_SEED], &program_id);

    eprintln!("[vault-init] asset:                {} (id={})", asset_cfg.label, asset_cfg.asset_id);
    eprintln!("[vault-init] vault program:        {program_id}");
    eprintln!("[vault-init] payer / authority:    {}", payer.pubkey());
    eprintln!("[vault-init] VaultConfig PDA:      {vault_config_pda}");
    eprintln!("[vault-init] NonceInCounter PDA:   {nonce_in_pda}");
    eprintln!("[vault-init] FederationSet PDA:    {federation_set_pda}");
    eprintln!("[vault-init] underlying_mint:      {underlying_mint}");
    eprintln!("[vault-init] vault_token_account:  {vault_token_account}");
    eprintln!(
        "[vault-init] decimals={} deposit_fee_bps={} release_fee_bps={} flags=0b{:08b}",
        asset_cfg.decimals, asset_cfg.deposit_fee_bps, asset_cfg.release_fee_bps, asset_cfg.flags
    );
    eprintln!("[vault-init] federation: {m}-of-{n}");

    let args = InitVaultArgs {
        asset_id: asset_cfg.asset_id,
        underlying_label: asset_cfg.underlying_label,
        underlying_mint: underlying_mint.to_bytes(),
        vault_token_account: vault_token_account.to_bytes(),
        decimals: asset_cfg.decimals,
        deposit_fee_bps: asset_cfg.deposit_fee_bps,
        release_fee_bps: asset_cfg.release_fee_bps,
        federation_m: m,
        federation_n: n,
        federation_members: members,
        flags: asset_cfg.flags,
    };

    let mut data = Vec::with_capacity(8 + 1024);
    data.extend_from_slice(&anchor_discriminator("init_vault"));
    args.serialize(&mut data).context("borsh-serialize args")?;

    // Account order matches `InitVault<'info>` in
    // programs/bridge-vault/src/instructions/init_vault.rs.
    let accounts = vec![
        AccountMeta::new(payer.pubkey(), true),                 // authority
        AccountMeta::new(vault_config_pda, false),              // vault_config
        AccountMeta::new(nonce_in_pda, false),                  // nonce_in
        AccountMeta::new(federation_set_pda, false),            // federation_set
        AccountMeta::new_readonly(system_program::id(), false), // system_program
    ];

    let ix = Instruction { program_id, accounts, data };

    let rpc = RpcClient::new_with_commitment(cli.rpc.clone(), CommitmentConfig::confirmed());
    let blockhash = rpc.get_latest_blockhash().context("get_latest_blockhash")?;
    let tx =
        Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);

    eprintln!("[vault-init] sending tx...");
    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("send_and_confirm_transaction (re-run for same asset returns 'already in use')")?;
    eprintln!("[vault-init] confirmed: {sig}");
    println!("VaultConfig PDA initialized: {vault_config_pda}");
    Ok(())
}

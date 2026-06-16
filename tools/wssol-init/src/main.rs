//! `staccana-wssol-init` — create the wssol Token-22 mint with the Confidential
//! Transfer Extension (CTE) on staccana, against an *arbitrary* Token-22 program ID.
//!
//! Why this tool exists: the upstream `spl-token` CLI hardcodes the canonical
//! Token-22 program ID (`TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`). Staccana
//! deploys Token-22 v8 at a fresh address (the canonical one would require Anza's
//! upgrade-authority keypair, which we don't have), so the CLI rejects every
//! `--program-id <ours>` call with "Unrecognized token program id". We work around
//! that by building the create-mint tx ourselves with the
//! [`spl_token_2022::instruction`] builders + [`spl_token_2022::extension`] CTE
//! initializer, against whatever Token-22 program pubkey we pass in.
//!
//! What it does, atomically in one tx:
//!   1. `system_program::create_account` — allocates the mint account at
//!      `--mint <keypair>`, sized for `Mint + ConfidentialTransferMint` extension,
//!      assigns owner = `--token-program <pubkey>`.
//!   2. `confidential_transfer::initialize_mint` — CTE config: authority,
//!      auto-approve flag, optional ElGamal auditor pubkey.
//!   3. `initialize_mint_2` — base mint init: decimals, mint authority,
//!      freeze authority.
//!
//! Usage (typical):
//!   staccana-wssol-init \
//!     --keypair /etc/staccana/keys/identity.json \
//!     --rpc http://localhost:8899 \
//!     --token-program 7bFHH22ASoMF1MGPvKPSWVfKXku8UJQUh355rmdrwAjU \
//!     --mint /etc/staccana/keys/wssol-mint.json \
//!     --decimals 9
//!
//! After this lands, the mint pubkey can be passed to `staccana-bridge-init
//! --staccana-mint <wssol-pk> --asset wSOL` to register wssol as a bridge asset.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Keypair, Signer};
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;
use spl_token_2022::extension::confidential_transfer::instruction as cte_ix;
use spl_token_2022::extension::ExtensionType;
use spl_token_2022::instruction as token_ix;
use spl_token_2022::state::Mint;

#[derive(Parser, Debug)]
#[command(
    name = "staccana-wssol-init",
    about = "create a Token-22 mint with the Confidential Transfer Extension against a custom Token-22 program ID"
)]
struct Cli {
    /// Fee payer + mint authority + freeze authority + CTE authority.
    #[arg(long)]
    keypair: PathBuf,

    /// RPC URL (defaults to local validator).
    #[arg(long, default_value = "http://localhost:8899")]
    rpc: String,

    /// Token-22 program ID on this chain. On staccana devnet this is the freshly
    /// deployed v8 program (NOT the canonical mainnet `TokenzQdB...` address).
    #[arg(long)]
    token_program: String,

    /// Keypair file for the mint account being created. The pubkey of this
    /// keypair becomes the mint pubkey; print it after success.
    #[arg(long)]
    mint: PathBuf,

    /// Decimals for the mint. wssol mirrors mainnet wSOL at 9.
    #[arg(long, default_value_t = 9)]
    decimals: u8,

    /// Auto-approve incoming confidential transfers (no per-recipient gating).
    /// True for permissionless wssol; flip to false if a custodian / auditor
    /// model is desired.
    #[arg(long, default_value_t = true)]
    auto_approve: bool,

    /// Optional ElGamal auditor pubkey (32 bytes hex). If set, all confidential
    /// transfers are decryptable by this party. Leave unset for a fully private
    /// mint with no third-party visibility.
    #[arg(long)]
    auditor_elgamal: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let payer = read_keypair_file(&cli.keypair)
        .map_err(|e| anyhow!("reading keypair {}: {}", cli.keypair.display(), e))?;
    let mint_kp: Keypair = read_keypair_file(&cli.mint)
        .map_err(|e| anyhow!("reading mint keypair {}: {}", cli.mint.display(), e))?;
    let token_program = Pubkey::from_str(&cli.token_program).context("parsing --token-program")?;

    let auditor_elgamal = match cli.auditor_elgamal.as_deref() {
        None => None,
        Some(hex_str) => {
            let bytes = hex::decode(hex_str.trim_start_matches("0x"))
                .context("decoding --auditor-elgamal hex")?;
            if bytes.len() != 32 {
                return Err(anyhow!(
                    "--auditor-elgamal must be 32 bytes (got {})",
                    bytes.len()
                ));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Some(arr.into())
        }
    };

    eprintln!("[wssol-init] payer:           {}", payer.pubkey());
    eprintln!("[wssol-init] token-22 program: {}", token_program);
    eprintln!("[wssol-init] mint pubkey:     {}", mint_kp.pubkey());
    eprintln!("[wssol-init] decimals:        {}", cli.decimals);
    eprintln!("[wssol-init] auto_approve:    {}", cli.auto_approve);
    eprintln!(
        "[wssol-init] auditor:         {}",
        if auditor_elgamal.is_some() {
            "set (third-party can decrypt)"
        } else {
            "none (fully private)"
        }
    );

    // Compute account size for Mint + CTE extension, then rent-exempt minimum.
    let space = ExtensionType::try_calculate_account_len::<Mint>(&[
        ExtensionType::ConfidentialTransferMint,
    ])
    .context("computing mint+CTE account size")?;

    let rpc = RpcClient::new_with_commitment(cli.rpc.clone(), CommitmentConfig::confirmed());
    let lamports = rpc
        .get_minimum_balance_for_rent_exemption(space)
        .context("get_minimum_balance_for_rent_exemption")?;
    eprintln!(
        "[wssol-init] account size:    {} bytes ({} lamports rent-exempt)",
        space, lamports
    );

    // Ix 1: create the mint account, owner = Token-22 program.
    let create_ix = system_instruction::create_account(
        &payer.pubkey(),
        &mint_kp.pubkey(),
        lamports,
        space as u64,
        &token_program,
    );

    // Ix 2: initialize the CTE on the new mint account.
    let init_cte_ix = cte_ix::initialize_mint(
        &token_program,
        &mint_kp.pubkey(),
        Some(payer.pubkey()),                      // CTE authority
        cli.auto_approve,
        auditor_elgamal,
    )
    .context("building initialize-CTE-mint ix")?;

    // Ix 3: initialize_mint_2 (base mint init — decimals, authorities).
    let init_mint_ix = token_ix::initialize_mint2(
        &token_program,
        &mint_kp.pubkey(),
        &payer.pubkey(),         // mint authority
        Some(&payer.pubkey()),   // freeze authority
        cli.decimals,
    )
    .context("building initialize_mint_2 ix")?;

    let blockhash = rpc.get_latest_blockhash().context("get_latest_blockhash")?;
    let tx = Transaction::new_signed_with_payer(
        &[create_ix, init_cte_ix, init_mint_ix],
        Some(&payer.pubkey()),
        &[&payer, &mint_kp],
        blockhash,
    );

    eprintln!("[wssol-init] sending tx (3 ixs: create_account + init_CTE + init_mint2)...");
    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("send_and_confirm_transaction")?;
    eprintln!("[wssol-init] confirmed: {}", sig);

    println!("{}", mint_kp.pubkey());
    Ok(())
}

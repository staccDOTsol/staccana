//! `staccana-bridge-cli` — user-facing CLI for the Staccana bridge.
//!
//! Three subcommands, mirroring the bridge's user-visible operations:
//!
//! - `deposit`  — submit a deposit ix to the per-asset mainnet vault program
//!                (mainnet → staccana). Federation observes, signs, user (or
//!                relayer) submits the resulting attestation to staccana to
//!                actually receive bridge mint tokens.
//! - `withdraw` — submit a burn ix to the staccana bridge program
//!                (staccana → mainnet). Federation observes, signs; the user
//!                separately presents the attestation to the per-asset
//!                mainnet vault to claim their underlying.
//! - `ratio`    — read and display the current per-asset `R_q64` from the
//!                staccana bridge's `RatioState` PDA via RPC.
//!
//! ### Examples
//!
//! ```text
//! staccana-bridge-cli deposit \
//!     --asset stSOL \
//!     --amount 1.5 \
//!     --mainnet-keypair ~/.config/solana/id.json \
//!     --staccana-dest <pubkey> \
//!     --mainnet-rpc https://api.mainnet-beta.solana.com
//!
//! staccana-bridge-cli withdraw \
//!     --asset stSOL \
//!     --amount 1.0 \
//!     --staccana-keypair ~/.config/staccana.json \
//!     --mainnet-dest <pubkey> \
//!     --staccana-rpc https://mp.fun/
//!
//! staccana-bridge-cli ratio \
//!     --asset stSOL \
//!     --staccana-rpc https://mp.fun/
//! ```
//!
//! All wire-format heavy lifting lives in the library half (`lib.rs` and the
//! per-flow modules); this binary is the clap glue plus RPC plumbing.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use solana_client::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Keypair, Signer};
use solana_sdk::transaction::Transaction;

use staccana_bridge_cli::asset::{format_amount, parse_amount, AssetId};
use staccana_bridge_cli::deposit::build_deposit_instruction;
use staccana_bridge_cli::ratio::RatioState;
use staccana_bridge_cli::withdraw::{build_burn_instruction, BurnAccounts};
use staccana_bridge_cli::{mainnet_vault_program_id, STACCANA_BRIDGE_PROGRAM_ID};

#[derive(Parser, Debug)]
#[command(
    name = "staccana-bridge-cli",
    version,
    about = "Bridge CLI for Staccana: deposit, withdraw, and read R."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Submit a deposit ix to the per-asset mainnet vault (mainnet → staccana).
    ///
    /// On its own, this only kicks off the bridge flow. The federation must
    /// observe and sign the resulting `Deposit` event, after which the user
    /// (or a relayer) submits the attestation to staccana to actually receive
    /// the bridge mint.
    Deposit {
        /// Asset label (`stSOL`, `ssUSDC`).
        #[arg(long)]
        asset: String,
        /// Amount to deposit, in human units (e.g. `1.5`). Converted to base
        /// units using the asset's decimals.
        #[arg(long)]
        amount: String,
        /// Path to the mainnet keypair to sign with.
        #[arg(long)]
        mainnet_keypair: PathBuf,
        /// Recipient pubkey on staccana that will receive the bridge mint.
        #[arg(long)]
        staccana_dest: String,
        /// Mainnet RPC endpoint.
        #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
        mainnet_rpc: String,
        /// Override the per-asset mainnet vault program id (defaults to the
        /// CLI's compiled-in placeholder; must be set until v0 vault programs
        /// are deployed).
        #[arg(long)]
        mainnet_vault_program_id: Option<String>,
        /// Build the ix and print it without submitting. Useful for review or
        /// for piping into a hardware-wallet-aware signer.
        #[arg(long)]
        dry_run: bool,
    },

    /// Submit a burn ix to the staccana bridge program (staccana → mainnet).
    ///
    /// The user is responsible for separately submitting the resulting
    /// attestation to the mainnet vault to actually claim the underlying.
    Withdraw {
        /// Asset label (`stSOL`, `ssUSDC`).
        #[arg(long)]
        asset: String,
        /// Amount of bridge mint tokens to burn, in human units (e.g. `1.0`).
        #[arg(long)]
        amount: String,
        /// Path to the staccana keypair to sign with.
        #[arg(long)]
        staccana_keypair: PathBuf,
        /// Recipient pubkey on mainnet that will receive the unwrapped underlying.
        #[arg(long)]
        mainnet_dest: String,
        /// Staccana RPC endpoint.
        #[arg(long)]
        staccana_rpc: String,
        /// Override the staccana bridge program id (defaults to the CLI's
        /// compiled-in placeholder).
        #[arg(long)]
        bridge_program_id: Option<String>,
        /// Override the staccana Token-22 mint pubkey for this asset.
        ///
        /// In a fully wired CLI this is read from the `AssetConfig` PDA; the
        /// override exists for early/test deployments where the registry is
        /// not yet populated.
        #[arg(long)]
        staccana_mint: String,
        /// Override the bridge program's `BridgeState` account pubkey.
        #[arg(long)]
        bridge_state: String,
        /// User ATA holding the mint balance to burn. Defaults to deriving
        /// from `(authority, staccana_mint)` once the CLI links against
        /// `spl-associated-token-account`; for now it must be supplied.
        #[arg(long)]
        user_ata: String,
        /// Build the ix and print it without submitting.
        #[arg(long)]
        dry_run: bool,
    },

    /// Read and display the current per-asset ratio R from staccana RPC.
    Ratio {
        /// Asset label (`stSOL`, `ssUSDC`).
        #[arg(long)]
        asset: String,
        /// Staccana RPC endpoint.
        #[arg(long)]
        staccana_rpc: String,
        /// Override the staccana bridge program id (defaults to the CLI's
        /// compiled-in placeholder).
        #[arg(long)]
        bridge_program_id: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Deposit {
            asset,
            amount,
            mainnet_keypair,
            staccana_dest,
            mainnet_rpc,
            mainnet_vault_program_id: vault_override,
            dry_run,
        } => run_deposit(
            &asset,
            &amount,
            &mainnet_keypair,
            &staccana_dest,
            &mainnet_rpc,
            vault_override.as_deref(),
            dry_run,
        ),
        Command::Withdraw {
            asset,
            amount,
            staccana_keypair,
            mainnet_dest,
            staccana_rpc,
            bridge_program_id,
            staccana_mint,
            bridge_state,
            user_ata,
            dry_run,
        } => run_withdraw(
            &asset,
            &amount,
            &staccana_keypair,
            &mainnet_dest,
            &staccana_rpc,
            bridge_program_id.as_deref(),
            &staccana_mint,
            &bridge_state,
            &user_ata,
            dry_run,
        ),
        Command::Ratio {
            asset,
            staccana_rpc,
            bridge_program_id,
        } => run_ratio(&asset, &staccana_rpc, bridge_program_id.as_deref()),
    }
}

/// Resolve a CLI-provided pubkey override or fall back to a compiled-in
/// default. Returns a wrapped error mentioning the flag name on failure so
/// users see which arg was malformed.
fn resolve_program_id(override_str: Option<&str>, fallback: Pubkey, flag: &str) -> Result<Pubkey> {
    match override_str {
        Some(s) => {
            Pubkey::from_str(s).with_context(|| format!("--{flag} is not a valid base58 pubkey"))
        }
        None => Ok(fallback),
    }
}

/// Parse a base58 pubkey from the CLI with a useful error message.
fn parse_pubkey(s: &str, flag: &str) -> Result<Pubkey> {
    Pubkey::from_str(s).with_context(|| format!("--{flag} is not a valid base58 pubkey"))
}

/// Load a keypair JSON file (the standard `solana-keygen`-format JSON array).
fn load_keypair(path: &PathBuf) -> Result<Keypair> {
    read_keypair_file(path).map_err(|e| anyhow!("failed to load keypair {}: {e}", path.display()))
}

fn run_deposit(
    asset_label: &str,
    amount_str: &str,
    keypair_path: &PathBuf,
    staccana_dest: &str,
    rpc_url: &str,
    vault_override: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let asset = AssetId::from_label(asset_label)?;
    let amount_base = parse_amount(amount_str, asset.default_decimals())?;
    let dest = parse_pubkey(staccana_dest, "staccana-dest")?;
    let payer = load_keypair(keypair_path)?;
    let vault_program = resolve_program_id(
        vault_override,
        mainnet_vault_program_id(asset),
        "mainnet-vault-program-id",
    )?;

    println!(
        "deposit: asset={} amount={} ({} base units) dest={} vault_program={}",
        asset.label(),
        format_amount(amount_base, asset.default_decimals()),
        amount_base,
        dest,
        vault_program,
    );

    // Account list for a real mainnet vault is per-vault and TBD; we attach
    // the payer as a signer and let the integrator extend this list. The
    // wire-format ix data is fully constructed and is the load-bearing part.
    //
    // TODO(integrator): once the per-asset mainnet vault programs land,
    // replace this with the real account list (vault state PDA, vault token
    // account, user token account, system program, token program, etc.).
    let metas = vec![solana_program::instruction::AccountMeta::new(
        payer.pubkey(),
        true,
    )];
    let ix = build_deposit_instruction(vault_program, asset, amount_base, dest, metas);

    if dry_run {
        print_instruction_summary(&ix);
        return Ok(());
    }

    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
    let blockhash = rpc
        .get_latest_blockhash()
        .context("failed to fetch latest blockhash from mainnet RPC")?;
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("failed to submit deposit transaction")?;
    println!("submitted: signature={sig}");
    println!(
        "next step: wait for federation attestation, then run the staccana \
         mint flow with that attestation"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_withdraw(
    asset_label: &str,
    amount_str: &str,
    keypair_path: &PathBuf,
    mainnet_dest_str: &str,
    rpc_url: &str,
    bridge_program_override: Option<&str>,
    staccana_mint_str: &str,
    bridge_state_str: &str,
    user_ata_str: &str,
    dry_run: bool,
) -> Result<()> {
    let asset = AssetId::from_label(asset_label)?;
    let amount_base = parse_amount(amount_str, asset.default_decimals())?;
    let mainnet_dest = parse_pubkey(mainnet_dest_str, "mainnet-dest")?;
    let bridge_program_id = resolve_program_id(
        bridge_program_override,
        STACCANA_BRIDGE_PROGRAM_ID,
        "bridge-program-id",
    )?;
    let staccana_mint = parse_pubkey(staccana_mint_str, "staccana-mint")?;
    let bridge_state = parse_pubkey(bridge_state_str, "bridge-state")?;
    let user_ata = parse_pubkey(user_ata_str, "user-ata")?;
    let user_authority = load_keypair(keypair_path)?;

    let (ratio_state, _) = asset.ratio_state_pda(&bridge_program_id);
    let (nonce_out, _) = asset.nonce_out_pda(&bridge_program_id);

    println!(
        "withdraw: asset={} amount={} ({} base units) mainnet_dest={} bridge={} ratio_pda={} nonce_pda={}",
        asset.label(),
        format_amount(amount_base, asset.default_decimals()),
        amount_base,
        mainnet_dest,
        bridge_program_id,
        ratio_state,
        nonce_out,
    );

    let accounts = BurnAccounts {
        bridge_state,
        staccana_mint,
        user_ata,
        user_authority: user_authority.pubkey(),
        ratio_state,
        nonce_out,
    };
    let ix = build_burn_instruction(
        bridge_program_id,
        asset,
        amount_base,
        mainnet_dest,
        accounts,
    );

    if dry_run {
        print_instruction_summary(&ix);
        return Ok(());
    }

    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
    let blockhash = rpc
        .get_latest_blockhash()
        .context("failed to fetch latest blockhash from staccana RPC")?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&user_authority.pubkey()),
        &[&user_authority],
        blockhash,
    );
    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("failed to submit burn transaction")?;
    println!("submitted: signature={sig}");
    println!(
        "next step: wait for federation attestation, then present it to the \
         mainnet vault to claim the underlying"
    );
    Ok(())
}

fn run_ratio(
    asset_label: &str,
    rpc_url: &str,
    bridge_program_override: Option<&str>,
) -> Result<()> {
    let asset = AssetId::from_label(asset_label)?;
    let bridge_program_id = resolve_program_id(
        bridge_program_override,
        STACCANA_BRIDGE_PROGRAM_ID,
        "bridge-program-id",
    )?;
    let (pda, _) = asset.ratio_state_pda(&bridge_program_id);

    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
    let data = rpc
        .get_account_data(&pda)
        .with_context(|| format!("failed to read ratio PDA {pda} from staccana RPC"))?;
    let state = RatioState::from_bytes(&data).context("failed to deserialize ratio state")?;

    // Print both the raw Q64.64 hex (load-bearing) and a decimal approximation
    // (display-only). Users should never read decimal `R` and round-trip it
    // back into the chain; the hex is the authoritative form.
    println!(
        "asset:                 {} (id={})",
        asset.label(),
        state.asset_id
    );
    println!("ratio PDA:             {pda}");
    println!("R (Q64.64 hex):        {}", state.r_as_hex());
    println!("R (~ decimal, lossy):  {:.18}", state.r_as_f64());
    println!("last_published_slot:   {}", state.last_published_slot);
    println!("last_nonce:            {}", state.last_nonce);
    println!("bump:                  {}", state.bump);
    // Note: vault_value and mint_supply are NOT stored on-chain (SPEC §5.2);
    // the bridge program recomputes R from federation-attested inputs and discards
    // them. To audit the latest attested values, query the federation-attestor's logs.
    Ok(())
}

/// Pretty-print an `Instruction` for `--dry-run`. Shows the program id,
/// account list (with signer/writable flags), and the data as hex.
fn print_instruction_summary(ix: &solana_program::instruction::Instruction) {
    println!("--- dry run ---");
    println!("program_id: {}", ix.program_id);
    for (i, meta) in ix.accounts.iter().enumerate() {
        let s = if meta.is_signer { "signer " } else { "       " };
        let w = if meta.is_writable {
            "writable "
        } else {
            "readonly "
        };
        println!("  [{i:>2}] {s}{w}{}", meta.pubkey);
    }
    println!("data ({} bytes): 0x{}", ix.data.len(), hex_encode(&ix.data));
}

/// Local hex encoder so we don't pull in another dependency just for display.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encode_matches_known_values() {
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0x00]), "00");
        assert_eq!(hex_encode(&[0xFF]), "ff");
        assert_eq!(hex_encode(&[0xDE, 0xAD, 0xBE, 0xEF]), "deadbeef");
    }

    #[test]
    fn resolve_program_id_uses_override_when_provided() {
        let override_pk = Pubkey::new_unique();
        let fallback = Pubkey::new_unique();
        let resolved =
            resolve_program_id(Some(&override_pk.to_string()), fallback, "test-flag").unwrap();
        assert_eq!(resolved, override_pk);
    }

    #[test]
    fn resolve_program_id_uses_fallback_when_none() {
        let fallback = Pubkey::new_unique();
        let resolved = resolve_program_id(None, fallback, "test-flag").unwrap();
        assert_eq!(resolved, fallback);
    }

    #[test]
    fn resolve_program_id_errors_on_garbage_override() {
        let fallback = Pubkey::new_unique();
        let err = resolve_program_id(Some("not-a-pubkey"), fallback, "test-flag").unwrap_err();
        assert!(err.to_string().contains("test-flag"));
    }
}

//! `staccana-bridge-release-relay` — federation publisher for the *outbound*
//! (staccana → mainnet) leg of the bridge.
//!
//! Mirror of `bridge_mint_relay` but the other way: watches staccana for
//! `BurnEvent`s emitted by `programs/bridge::burn`, has M federation members sign
//! the `MAINNET_RELEASE_V1` attestation, and submits `release_with_attestation`
//! to the mainnet `bridge-vault` program. The vault's on-chain handler verifies
//! the M sigs (single batched ed25519 precompile ix preferred — this requires the
//! mainnet vault to have been upgraded with `parse_ed25519_batch_at`, which
//! landed at sig EeLFcwWtLsg…) and transfers `(release_amount - release_fee)`
//! underlying to the attested mainnet recipient.
//!
//! Same shape as the inbound relay:
//!   - One-shot mode (`--burn-sig`): backrun a single staccana burn
//!   - Daemon mode (intended): wired into `staccana-bridge-publisher.timer` so
//!     each minute sweeps recent staccana sigs and processes any new burn
//!
//! The submit path is idempotent — replaying a burn whose nonce_out is already
//! consumed fails with "address already in use" on the staccana-side `nonce_out`
//! marker PDA, which we swallow.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use borsh::BorshSerialize;
use clap::Parser;
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    ed25519_program,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, Keypair, Signer},
    sysvar,
    transaction::Transaction,
};
use staccana_federation_attestor::bridge_msg::{build_release_message, RELEASE_MSG_LEN};
use staccana_federation_attestor::bridge_observer::{
    extract_burn_events, BridgeRpcClient, BurnEvent, SolanaRpcClient,
};

/// `sha256("global:release_with_attestation")[0..8]`. Anchor builds these at
/// codegen time; we recompute here to keep the dep graph flat.
fn anchor_discriminator(name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{name}").as_bytes());
    let out = h.finalize();
    let mut d = [0u8; 8];
    d.copy_from_slice(&out[..8]);
    d
}

const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    0x8c, 0x97, 0x25, 0x8f, 0x4e, 0x24, 0x89, 0xf1, 0xbb, 0x3d, 0x10, 0x29, 0x14, 0x8e, 0x0d, 0x83,
    0x0b, 0x5a, 0x13, 0x99, 0xda, 0xff, 0x10, 0x84, 0x04, 0x8e, 0x7b, 0xd8, 0xdb, 0xe9, 0xf8, 0x59,
]);

/// Mirror of `bridge_vault::instructions::release_with_attestation::ReleaseArgs`.
/// Layout MUST match the on-chain Borsh order or we silently mis-encode.
#[derive(BorshSerialize)]
struct ReleaseArgs {
    asset_id: u32,
    release_amount: u64,
    recipient: [u8; 32],
    nonce: u64,
    federation_indices: Vec<u8>,
}

const ED25519_PUBKEY_SIZE: usize = 32;
const ED25519_SIGNATURE_SIZE: usize = 64;
const ED25519_OFFSETS_SIZE: usize = 14;

/// Batched ed25519 precompile: M sigs over a shared message. Same shape as the
/// inbound mint relay. See `bridge_mint_relay.rs::build_ed25519_batch_precompile_ix`
/// for the byte-level layout.
fn build_ed25519_batch_precompile_ix(keypairs: &[&Keypair], message: &[u8]) -> Instruction {
    let m = keypairs.len();
    assert!(m > 0 && m <= 16, "M out of range");
    let header = 2;
    let pubkeys_start = header + m * ED25519_OFFSETS_SIZE;
    let signatures_start = pubkeys_start + m * ED25519_PUBKEY_SIZE;
    let message_start = signatures_start + m * ED25519_SIGNATURE_SIZE;
    let total = message_start + message.len();

    let mut data = vec![0u8; total];
    data[0] = m as u8;

    for (i, kp) in keypairs.iter().enumerate() {
        let pk_offset = pubkeys_start + i * ED25519_PUBKEY_SIZE;
        let sig_offset = signatures_start + i * ED25519_SIGNATURE_SIZE;
        let msg_offset = message_start;

        let off_base = header + i * ED25519_OFFSETS_SIZE;
        data[off_base..off_base + 2].copy_from_slice(&(sig_offset as u16).to_le_bytes());
        data[off_base + 2..off_base + 4].copy_from_slice(&u16::MAX.to_le_bytes());
        data[off_base + 4..off_base + 6].copy_from_slice(&(pk_offset as u16).to_le_bytes());
        data[off_base + 6..off_base + 8].copy_from_slice(&u16::MAX.to_le_bytes());
        data[off_base + 8..off_base + 10].copy_from_slice(&(msg_offset as u16).to_le_bytes());
        data[off_base + 10..off_base + 12].copy_from_slice(&(message.len() as u16).to_le_bytes());
        data[off_base + 12..off_base + 14].copy_from_slice(&u16::MAX.to_le_bytes());

        let pk_bytes = kp.pubkey().to_bytes();
        data[pk_offset..pk_offset + ED25519_PUBKEY_SIZE].copy_from_slice(&pk_bytes);

        let sig = kp.sign_message(message);
        let sig_bytes: &[u8] = sig.as_ref();
        assert_eq!(sig_bytes.len(), ED25519_SIGNATURE_SIZE);
        data[sig_offset..sig_offset + ED25519_SIGNATURE_SIZE].copy_from_slice(sig_bytes);
    }
    data[message_start..total].copy_from_slice(message);

    Instruction {
        program_id: ed25519_program::ID,
        accounts: vec![],
        data,
    }
}

/// Standard ATA derivation for a (owner, mint, token_program) triple.
fn derive_ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    let (pda, _bump) = Pubkey::find_program_address(
        &[owner.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    );
    pda
}

/// `createAssociatedTokenAccountIdempotent` (variant 1).
fn build_create_ata_idempotent_ix(
    payer: &Pubkey,
    ata: &Pubkey,
    owner: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*ata, false),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
            AccountMeta::new_readonly(*token_program, false),
        ],
        data: vec![1u8],
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "staccana-bridge-release-relay",
    about = "Submit a federation-signed `release_with_attestation` ix to the mainnet bridge-vault for one staccana burn."
)]
struct Cli {
    /// Staccana RPC for fetching the burn transaction.
    #[arg(long, default_value = "http://localhost:8899")]
    staccana_rpc: String,

    /// Staccana bridge program id.
    #[arg(long, default_value = "Bridge1111111111111111111111111111111111111")]
    staccana_bridge: String,

    /// Mainnet RPC for submitting the release.
    #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
    solana_rpc: String,

    /// Mainnet bridge-vault program id.
    #[arg(long, default_value = "BwimCCoPP5of41ukG1wA1gLz5wXQ4mmbcmjdFT9M1mBL")]
    bridge_vault: String,

    /// Token program for the underlying on mainnet (Token-22 for Staccana).
    #[arg(long, default_value = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")]
    token_program: String,

    /// Federation signer keypair directory.
    #[arg(long, default_value = "/etc/staccana/federation")]
    federation_dir: PathBuf,

    /// Federation threshold M.
    #[arg(long, default_value_t = 5)]
    m: u8,

    /// Mainnet fee-payer keypair. Pays the new `nonce_out` PDA's rent + the ATA
    /// setup tx if the recipient ATA doesn't exist. Needs SOL on mainnet.
    #[arg(long)]
    payer: PathBuf,

    /// Staccana burn signature to relay.
    #[arg(long)]
    burn_sig: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let staccana_rpc = SolanaRpcClient::new(cli.staccana_rpc.clone());
    let logs = staccana_rpc
        .transaction_logs(&cli.burn_sig)
        .with_context(|| format!("fetch logs for {}", cli.burn_sig))?
        .ok_or_else(|| anyhow!("no logs for sig {}", cli.burn_sig))?;
    let events = extract_burn_events(&logs, &cli.burn_sig);
    if events.is_empty() {
        return Err(anyhow!(
            "no BurnEvent in tx logs — wrong sig, or burn failed"
        ));
    }
    eprintln!(
        "[release-relay] found {} burn event(s) in {}",
        events.len(),
        cli.burn_sig
    );

    let payer = read_keypair_file(&cli.payer)
        .map_err(|e| anyhow!("read payer keypair {}: {e}", cli.payer.display()))?;
    let vault_program = Pubkey::from_str(&cli.bridge_vault).context("parse --bridge-vault")?;
    let token_program = Pubkey::from_str(&cli.token_program).context("parse --token-program")?;
    let mainnet_rpc = RpcClient::new_with_commitment(
        cli.solana_rpc.clone(),
        CommitmentConfig::confirmed(),
    );

    let mut signers: Vec<(u8, Keypair)> = Vec::with_capacity(cli.m as usize);
    for i in 1..=cli.m {
        let p = cli.federation_dir.join(format!("signer-{i}.json"));
        let kp = read_keypair_file(&p)
            .map_err(|e| anyhow!("read federation signer-{i} from {}: {e}", p.display()))?;
        signers.push((i - 1, kp));
    }

    for event in events {
        process_event(&event, &payer, &mainnet_rpc, vault_program, token_program, &signers)?;
    }
    Ok(())
}

fn process_event(
    event: &BurnEvent,
    payer: &Keypair,
    mainnet_rpc: &RpcClient,
    vault_program: Pubkey,
    token_program: Pubkey,
    signers: &[(u8, Keypair)],
) -> Result<()> {
    eprintln!(
        "[release-relay] processing: asset_id={} gross_release={} mainnet_dest={} nonce_out={}",
        event.asset_id,
        event.gross_release,
        bs58::encode(event.mainnet_dest).into_string(),
        event.nonce_out,
    );

    let asset_id_le = event.asset_id.to_le_bytes();
    let nonce_le = event.nonce_out.to_le_bytes();

    let (vault_config_pda, _) =
        Pubkey::find_program_address(&[b"vault", asset_id_le.as_ref()], &vault_program);
    let (federation_set_pda, _) =
        Pubkey::find_program_address(&[b"federation"], &vault_program);
    let (nonce_out_pda, _) = Pubkey::find_program_address(
        &[b"nonce_out", asset_id_le.as_ref(), nonce_le.as_ref()],
        &vault_program,
    );

    // Read VaultConfig to learn underlying_mint + vault_token_account + flags.
    // Layout (after 8-byte Anchor disc):
    //   asset_id (4) || underlying_label (32) || underlying_mint (32) ||
    //   vault_token_account (32) || total_locked (8) || decimals (1) ||
    //   deposit_fee_bps (2) || release_fee_bps (2) || bump (1) || flags (1)
    // staccana_mint isn't on the vault side — that's the staccana bridge AssetConfig.
    let cfg_data = mainnet_rpc
        .get_account_data(&vault_config_pda)
        .with_context(|| format!("read vault_config {vault_config_pda}"))?;
    if cfg_data.len() < 117 {
        return Err(anyhow!(
            "vault_config too small ({} bytes) — has the program been upgraded?",
            cfg_data.len()
        ));
    }
    let underlying_mint = Pubkey::new_from_array(cfg_data[44..76].try_into().unwrap());
    let vault_token_account = Pubkey::new_from_array(cfg_data[76..108].try_into().unwrap());
    let flags = cfg_data[cfg_data.len() - 1];
    let is_native = (flags & 0b0000_0010) != 0; // AssetFlag::NATIVE_SOL

    eprintln!(
        "[release-relay] underlying_mint={underlying_mint} vault_token_account={vault_token_account} is_native={is_native}",
    );

    let recipient_pk = Pubkey::new_from_array(event.mainnet_dest);
    // For wSOL the recipient IS the mainnet wallet (lamport-direct transfer).
    // For SPL we need the recipient's ATA on mainnet for `underlying_mint`.
    let recipient_account = if is_native {
        recipient_pk
    } else {
        let ata = derive_ata(&recipient_pk, &underlying_mint, &token_program);
        // Pre-create in a setup tx if missing — keeps the release tx lean.
        if mainnet_rpc.get_account(&ata).is_err() {
            eprintln!("[release-relay] recipient ATA does not exist on mainnet — creating in a setup tx");
            let setup_ix = build_create_ata_idempotent_ix(
                &payer.pubkey(),
                &ata,
                &recipient_pk,
                &underlying_mint,
                &token_program,
            );
            let bh = mainnet_rpc
                .get_latest_blockhash()
                .context("get_latest_blockhash for ATA setup")?;
            let setup_tx = Transaction::new_signed_with_payer(
                &[setup_ix],
                Some(&payer.pubkey()),
                &[payer],
                bh,
            );
            let sig = mainnet_rpc
                .send_and_confirm_transaction(&setup_tx)
                .context("send_and_confirm_transaction (mainnet createATA setup)")?;
            eprintln!("[release-relay] createATA setup sig: {sig}");
        }
        ata
    };

    let msg = build_release_message(
        event.asset_id,
        event.gross_release,
        &event.mainnet_dest,
        event.nonce_out,
    );
    debug_assert_eq!(msg.len(), RELEASE_MSG_LEN);

    let kp_refs: Vec<&Keypair> = signers.iter().map(|(_, kp)| kp).collect();
    let federation_indices: Vec<u8> = signers.iter().map(|(i, _)| *i).collect();

    let release_args = ReleaseArgs {
        asset_id: event.asset_id,
        release_amount: event.gross_release,
        recipient: event.mainnet_dest,
        nonce: event.nonce_out,
        federation_indices,
    };
    let mut data = Vec::with_capacity(8 + 4 + 8 + 32 + 8 + 4 + signers.len());
    data.extend_from_slice(&anchor_discriminator("release_with_attestation"));
    release_args
        .serialize(&mut data)
        .context("borsh-serialize ReleaseArgs")?;

    let release_ix = Instruction {
        program_id: vault_program,
        accounts: vec![
            AccountMeta::new(payer.pubkey(), true),                  // payer
            AccountMeta::new(vault_config_pda, false),               // vault_config
            AccountMeta::new_readonly(federation_set_pda, false),    // federation_set
            AccountMeta::new(nonce_out_pda, false),                  // nonce_out (init)
            AccountMeta::new_readonly(underlying_mint, false),       // underlying_mint
            AccountMeta::new(vault_token_account, false),            // vault_token_account
            AccountMeta::new(recipient_account, false),              // recipient
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
            AccountMeta::new_readonly(token_program, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
        ],
        data,
    };

    let ixs = vec![build_ed25519_batch_precompile_ix(&kp_refs, &msg), release_ix];

    let bh = mainnet_rpc
        .get_latest_blockhash()
        .context("get_latest_blockhash from mainnet_rpc")?;
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer], bh);

    let serialized_len = bincode::serialize(&tx).map(|b| b.len()).unwrap_or(0);
    eprintln!(
        "[release-relay] submitting tx: ix_count={} serialized_bytes={}",
        ixs.len(),
        serialized_len,
    );
    let sig = mainnet_rpc
        .send_and_confirm_transaction(&tx)
        .context("send_and_confirm_transaction (mainnet release)")?;
    println!("[release-relay] released: {sig}");
    println!(
        "[release-relay] => recipient={recipient_account} should now be credited gross={} (less mainnet release fee)",
        event.gross_release,
    );
    Ok(())
}

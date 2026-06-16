//! `staccana-bridge-mint-relay` — federation publisher for the staccana ↔ Solana
//! bridge.
//!
//! Two modes:
//!
//! - **One-shot** (`--deposit-sig`): backrun a single mainnet `Deposit` event by
//!   collecting M federation signatures and submitting the staccana-side `mint` ix.
//!   Used to backfill historical deposits before the daemon was running.
//!
//! - **Daemon** (`--daemon --cursor-file PATH`): poll the mainnet vault for new
//!   `Deposit` signatures since the persisted cursor, process each one (sign + submit),
//!   advance the cursor. Runs forever until killed.
//!
//! The submit path is idempotent — re-running on an already-consumed nonce fails cleanly
//! with `Allocate` (the staccana-side `nonce_in` PDA already exists), which we swallow.
//!
//! Architecture:
//!   1. Fetch the mainnet tx via `--solana-rpc` and parse out the `DepositEvent`.
//!   2. Read the staccana-side `AssetConfig` PDA to learn `staccana_mint` (we
//!      avoid baking that into a CLI flag because the source of truth is on-chain).
//!   3. Load the first M signer-N.json keypairs from `--federation-dir` and have
//!      each one sign the canonical `STACCANA_MINT_V1` message bytes.
//!   4. Construct one batched ed25519 precompile ix containing all M signatures
//!      (the bridge handler decodes via `parse_ed25519_batch_at`), optionally a
//!      `createAssociatedTokenAccountIdempotent` setup tx if the recipient's ATA
//!      doesn't exist, and the bridge `mint` ix itself.
//!   5. Send + confirm.
//!
//! "For the culture" mode: this binary lives co-located on val-1 with all 9
//! federation signer keypairs, so it can sign with any M of them in one process —
//! no inter-host gossip required. A multi-host federation would need real
//! aggregation, which is out of scope for v1 launch.
//!
//! Architecture:
//!   1. Fetch the mainnet tx via `--solana-rpc` and parse out the `DepositEvent`.
//!   2. Read the staccana-side `AssetConfig` PDA to learn `staccana_mint` (we
//!      avoid baking that into a CLI flag because the source of truth is on-chain).
//!   3. Load the first M signer-N.json keypairs from `--federation-dir` and have
//!      each one sign the canonical `STACCANA_MINT_V1` message bytes.
//!   4. Construct M ed25519 precompile ixs, a `createAssociatedTokenAccountIdempotent`
//!      ix for the recipient ATA on the staccana mirror mint, and the bridge `mint`
//!      ix itself. The bridge handler validates that the M sigs precede it in the
//!      same tx.
//!   5. Send + confirm.
//!
//! Once this run lands successfully we'll know the bridge round-trip works
//! end-to-end. The next step (out of scope here) is wiring the equivalent
//! aggregation/publish path into the daemon for going-forward operation.

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
use staccana_federation_attestor::bridge_msg::{build_mint_message, MINT_MSG_LEN};
use staccana_federation_attestor::bridge_observer::{
    extract_deposit_events, BridgeRpcClient, DepositEvent, SolanaRpcClient,
};

/// Anchor's standard discriminator: `sha256("global:<ix_name>")[0..8]`.
fn anchor_discriminator(name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{name}").as_bytes());
    let out = h.finalize();
    let mut d = [0u8; 8];
    d.copy_from_slice(&out[..8]);
    d
}

/// The standard Associated Token Account program id (same on every cluster, including
/// staccana — it's bundled at genesis as a built-in).
const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    0x8c, 0x97, 0x25, 0x8f, 0x4e, 0x24, 0x89, 0xf1, 0xbb, 0x3d, 0x10, 0x29, 0x14, 0x8e, 0x0d, 0x83,
    0x0b, 0x5a, 0x13, 0x99, 0xda, 0xff, 0x10, 0x84, 0x04, 0x8e, 0x7b, 0xd8, 0xdb, 0xe9, 0xf8, 0x59,
]);

/// Borsh layout of `bridge::instructions::mint::MintArgs`. Field order MUST match the
/// on-chain struct exactly — Borsh is positional. If the program is renamed or the
/// struct shape changes, the canary unit tests in `bridge-init` would catch it; we
/// keep the constant inline here to avoid a heavy path-dep on the Anchor crate.
#[derive(BorshSerialize)]
struct MintArgs {
    asset_id: u32,
    value_after_fee: u64,
    recipient: [u8; 32],
    nonce: u64,
    federation_indices: Vec<u8>,
}

/// Layout constants for the ed25519 precompile. Mirrors the helper in
/// `tools/claim-cli/src/tx.rs` — copied rather than re-exported to keep the dep graph
/// flat. See Solana's `solana-ed25519-program` for the byte-level spec.
const ED25519_PUBKEY_SIZE: usize = 32;
const ED25519_SIGNATURE_SIZE: usize = 64;
const ED25519_OFFSETS_SIZE: usize = 14;
const ED25519_OFFSETS_START: usize = 2;
const ED25519_DATA_START: usize = ED25519_OFFSETS_SIZE + ED25519_OFFSETS_START;

/// Batched ed25519 precompile ix carrying M signatures over a single shared message.
///
/// Layout (per Solana's ed25519 precompile spec):
///
/// ```text
/// [num_sigs: u8] [padding: u8]
/// repeated M times: [14-byte (sig_off, sig_ix=self, pk_off, pk_ix=self, msg_off, msg_size, msg_ix=self) record]
/// repeated M times: [pubkey 32]
/// repeated M times: [signature 64]
/// [message bytes]
/// ```
///
/// All sig/pubkey/message offsets reference *this* ix (`u16::MAX` instruction index).
/// The shared message saves M-1 copies vs the M-separate-ix layout: a 5-sig federation
/// with a 68-byte preimage shrinks from ~900 bytes to ~620 bytes of ix data.
///
/// The on-chain bridge handler decodes via `parse_ed25519_batch_at` (see
/// `programs/bridge/src/ed25519.rs`).
fn build_ed25519_batch_precompile_ix(keypairs: &[&Keypair], message: &[u8]) -> Instruction {
    let m = keypairs.len();
    assert!(m > 0 && m <= 16, "M out of range");
    let header = 2;
    let offsets_total = m * ED25519_OFFSETS_SIZE;
    let pubkeys_start = header + offsets_total;
    let signatures_start = pubkeys_start + m * ED25519_PUBKEY_SIZE;
    let message_start = signatures_start + m * ED25519_SIGNATURE_SIZE;
    let total = message_start + message.len();

    let mut data = vec![0u8; total];
    data[0] = m as u8;
    // data[1] = 0 padding (already zero-initialized)

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

/// Derive the ATA for `(owner, mint, token_program)`. ATA is the standard PDA at
/// `[owner, token_program, mint]` under the Associated Token Account program.
fn derive_ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    let (pda, _bump) = Pubkey::find_program_address(
        &[owner.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    );
    pda
}

/// `createAssociatedTokenAccountIdempotent`. ix data = [1] (variant 1 of the ATA
/// program). Won't fail if the account already exists.
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
    name = "staccana-bridge-mint-relay",
    about = "Submit a federation-signed `mint` ix to staccana for one mainnet deposit."
)]
struct Cli {
    /// Mainnet RPC for fetching the deposit transaction.
    #[arg(long, default_value = "https://api.mainnet-beta.solana.com")]
    solana_rpc: String,

    /// Mainnet bridge-vault program id (the deposit emitter). Used purely for
    /// logging/sanity — actual event extraction is signature-based.
    #[arg(long, default_value = "BwimCCoPP5of41ukG1wA1gLz5wXQ4mmbcmjdFT9M1mBL")]
    bridge_vault: String,

    /// Staccana RPC.
    #[arg(long, default_value = "http://localhost:8899")]
    staccana_rpc: String,

    /// Staccana bridge program id (the wrapper-mint program).
    #[arg(long, default_value = "Bridge1111111111111111111111111111111111111")]
    staccana_bridge: String,

    /// Token program for the staccana mirror mint (Token-22 for the Staccana asset).
    #[arg(long, default_value = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")]
    token_program: String,

    /// Directory containing federation member keypairs `signer-1.json` ..
    /// `signer-N.json`. We pick the first M of these to sign the attestation.
    #[arg(long, default_value = "/etc/staccana/federation")]
    federation_dir: PathBuf,

    /// Federation threshold M (signatures required). Must match on-chain
    /// `FederationSet.m`. Default 5 mirrors the v1 5-of-9 set.
    #[arg(long, default_value_t = 5)]
    m: u8,

    /// Fee-payer keypair for the staccana submit. Pays rent for the new
    /// `nonce_in` PDA and the recipient ATA (if it doesn't exist yet).
    #[arg(long)]
    payer: PathBuf,

    /// Mainnet deposit transaction signature to backrun.
    #[arg(long)]
    deposit_sig: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let solana_rpc = SolanaRpcClient::new(cli.solana_rpc.clone());
    let logs = solana_rpc
        .transaction_logs(&cli.deposit_sig)
        .with_context(|| format!("fetch logs for {}", cli.deposit_sig))?
        .ok_or_else(|| anyhow!("no logs for sig {}", cli.deposit_sig))?;
    let events = extract_deposit_events(&logs, &cli.deposit_sig);
    if events.is_empty() {
        return Err(anyhow!(
            "no DepositEvent found in tx logs — wrong sig, or the deposit failed"
        ));
    }
    eprintln!(
        "[mint-relay] found {} deposit event(s) in {}",
        events.len(),
        cli.deposit_sig
    );

    let payer = read_keypair_file(&cli.payer)
        .map_err(|e| anyhow!("read payer keypair {}: {e}", cli.payer.display()))?;
    let bridge_program = Pubkey::from_str(&cli.staccana_bridge).context("parse --staccana-bridge")?;
    let token_program = Pubkey::from_str(&cli.token_program).context("parse --token-program")?;
    let staccana_rpc = RpcClient::new_with_commitment(
        cli.staccana_rpc.clone(),
        CommitmentConfig::confirmed(),
    );

    // Load the M signer keypairs. signer-i.json is members[i-1] on-chain (the
    // FederationSet stores them in the order they were registered, and the init
    // tooling preserves that 1-indexed convention).
    let mut signers: Vec<(u8, Keypair)> = Vec::with_capacity(cli.m as usize);
    for i in 1..=cli.m {
        let p = cli.federation_dir.join(format!("signer-{i}.json"));
        let kp = read_keypair_file(&p)
            .map_err(|e| anyhow!("read federation signer-{i} from {}: {e}", p.display()))?;
        signers.push((i - 1, kp));
    }
    eprintln!(
        "[mint-relay] loaded {} federation signers (indices {:?})",
        signers.len(),
        signers.iter().map(|(i, _)| *i).collect::<Vec<_>>()
    );

    for event in events {
        process_event(&event, &payer, &staccana_rpc, bridge_program, token_program, &signers)?;
    }
    Ok(())
}

fn process_event(
    event: &DepositEvent,
    payer: &Keypair,
    staccana_rpc: &RpcClient,
    bridge_program: Pubkey,
    token_program: Pubkey,
    signers: &[(u8, Keypair)],
) -> Result<()> {
    eprintln!(
        "[mint-relay] processing: asset_id={} amount_after_fee={} dest={} nonce={}",
        event.asset_id,
        event.amount_after_fee,
        bs58::encode(event.dest).into_string(),
        event.nonce
    );

    let asset_id_le = event.asset_id.to_le_bytes();
    let nonce_le = event.nonce.to_le_bytes();

    let (asset_config_pda, _) =
        Pubkey::find_program_address(&[b"asset", asset_id_le.as_ref()], &bridge_program);
    let (ratio_state_pda, _) =
        Pubkey::find_program_address(&[b"ratio", asset_id_le.as_ref()], &bridge_program);
    let (federation_set_pda, _) = Pubkey::find_program_address(&[b"federation"], &bridge_program);
    let (nonce_in_pda, _) = Pubkey::find_program_address(
        &[b"nonce_in", asset_id_le.as_ref(), nonce_le.as_ref()],
        &bridge_program,
    );

    // Pull the staccana_mint pubkey out of AssetConfig. Layout (post 8-byte Anchor
    // disc): asset_id (4) || label (32) || mainnet_vault (32) || staccana_mint (32) ...
    // so staccana_mint sits at bytes 8 + 4 + 32 + 32 .. + 32 = bytes 76..108.
    let cfg_data = staccana_rpc
        .get_account_data(&asset_config_pda)
        .with_context(|| format!("read asset_config {asset_config_pda}"))?;
    if cfg_data.len() < 108 {
        return Err(anyhow!(
            "asset_config too small ({} bytes) — has the program been upgraded?",
            cfg_data.len()
        ));
    }
    let staccana_mint = Pubkey::new_from_array(cfg_data[76..108].try_into().unwrap());
    eprintln!("[mint-relay] staccana_mint: {staccana_mint}");

    // Recipient ATA on staccana — the bridge mint handler binds the ATA owner to the
    // attested `args.recipient`. We'll createIdempotent so the relay works whether or
    // not the user has interacted with the mirror mint before.
    let recipient_pk = Pubkey::new_from_array(event.dest);
    let recipient_ata = derive_ata(&recipient_pk, &staccana_mint, &token_program);
    eprintln!("[mint-relay] recipient_ata: {recipient_ata} (owner={recipient_pk})");

    // Build the canonical mint message (must match the on-chain preimage exactly).
    let msg = build_mint_message(
        event.asset_id,
        event.amount_after_fee,
        &event.dest,
        event.nonce,
    );
    debug_assert_eq!(msg.len(), MINT_MSG_LEN);

    // Pre-create the recipient ATA in a SEPARATE tx if it doesn't exist yet. We keep
    // it out of the mint tx because including the ATA program account + its ix in the
    // signed-mint tx pushes us over the 1232-byte legacy-tx ceiling once we have 5
    // batched ed25519 sigs (1266 vs 1232). Pre-creating leaves the mint tx lean.
    let ata_exists = staccana_rpc.get_account(&recipient_ata).is_ok();
    if !ata_exists {
        eprintln!("[mint-relay] recipient ATA does not exist — creating in a setup tx");
        let setup_ix = build_create_ata_idempotent_ix(
            &payer.pubkey(),
            &recipient_ata,
            &recipient_pk,
            &staccana_mint,
            &token_program,
        );
        let bh = staccana_rpc
            .get_latest_blockhash()
            .context("get_latest_blockhash for ATA setup")?;
        let setup_tx = Transaction::new_signed_with_payer(
            &[setup_ix],
            Some(&payer.pubkey()),
            &[payer],
            bh,
        );
        let sig = staccana_rpc
            .send_and_confirm_transaction(&setup_tx)
            .context("send_and_confirm_transaction (createATA setup)")?;
        eprintln!("[mint-relay] createATA setup sig: {sig}");
    } else {
        eprintln!("[mint-relay] recipient ATA already exists — skipping setup");
    }

    // ONE batched ed25519 precompile ix carrying all M signatures over the shared
    // message. The bridge handler reads this ix via `parse_ed25519_batch_at`.
    let kp_refs: Vec<&Keypair> = signers.iter().map(|(_, kp)| kp).collect();
    let mut ixs: Vec<Instruction> = vec![build_ed25519_batch_precompile_ix(&kp_refs, &msg)];

    // Construct the bridge `mint` ix. Account order matches `BridgeMint<'info>` in
    // `programs/bridge/src/instructions/mint.rs`.
    let federation_indices: Vec<u8> = signers.iter().map(|(i, _)| *i).collect();
    let mint_args = MintArgs {
        asset_id: event.asset_id,
        value_after_fee: event.amount_after_fee,
        recipient: event.dest,
        nonce: event.nonce,
        federation_indices,
    };
    let mut data = Vec::with_capacity(8 + 4 + 8 + 32 + 8 + 4 + signers.len());
    data.extend_from_slice(&anchor_discriminator("mint"));
    mint_args
        .serialize(&mut data)
        .context("borsh-serialize MintArgs")?;

    let mint_ix = Instruction {
        program_id: bridge_program,
        accounts: vec![
            AccountMeta::new(payer.pubkey(), true),                  // payer
            AccountMeta::new_readonly(asset_config_pda, false),      // asset_config
            AccountMeta::new_readonly(ratio_state_pda, false),       // ratio_state
            AccountMeta::new_readonly(federation_set_pda, false),    // federation_set
            AccountMeta::new(staccana_mint, false),                  // staccana_mint
            AccountMeta::new(recipient_ata, false),                  // recipient_ata
            AccountMeta::new(nonce_in_pda, false),                   // nonce_in
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
            AccountMeta::new_readonly(token_program, false),
            AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
        ],
        data,
    };
    ixs.push(mint_ix);

    let bh = staccana_rpc
        .get_latest_blockhash()
        .context("get_latest_blockhash from staccana_rpc")?;
    let tx = Transaction::new_signed_with_payer(&ixs, Some(&payer.pubkey()), &[payer], bh);

    // Print per-ix sizes + the actual serialized tx size so we know where the bytes
    // are going if we hit the 1232 ceiling.
    for (i, ix) in ixs.iter().enumerate() {
        eprintln!(
            "[mint-relay]   ix[{i}]: program_id={} accounts={} data_len={}",
            ix.program_id,
            ix.accounts.len(),
            ix.data.len()
        );
    }
    let unique_keys = tx.message.account_keys.len();
    let serialized_len = bincode::serialize(&tx).map(|b| b.len()).unwrap_or(0);
    eprintln!(
        "[mint-relay] submitting tx: ix_count={} unique_account_keys={} serialized_bytes={}",
        ixs.len(),
        unique_keys,
        serialized_len,
    );
    // Note: 5-of-9 batched ed25519 ix = 2 (header) + 5*14 (offsets) + 5*32 (pks) +
    // 5*64 (sigs) + 68 (shared msg) = 620 bytes. Plus ~120 bytes for the mint ix and
    // a small createATA — total tx well under the 1232-byte legacy ceiling.
    let sig = staccana_rpc
        .send_and_confirm_transaction(&tx)
        .context("send_and_confirm_transaction (staccana mint)")?;
    println!("[mint-relay] minted: {sig}");
    println!(
        "[mint-relay] => recipient_ata={} should now hold {} mirror Staccana base units",
        recipient_ata, event.amount_after_fee
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity — Anchor renames `mint` would silently brick us; pin the disc.
    /// `python3 -c "import hashlib; print(hashlib.sha256(b'global:mint').hexdigest()[:16])"
    /// → 33e685a4017f83ad`.
    #[test]
    fn mint_discriminator_pinned() {
        let got = anchor_discriminator("mint");
        let expected = [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad];
        assert_eq!(got, expected);
    }

    /// Pin the well-known ATA program id constant — silently wrong bytes would
    /// derive wrong PDAs and waste a tx.
    #[test]
    fn ata_program_id_matches_well_known() {
        let parsed = Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap();
        assert_eq!(parsed, ASSOCIATED_TOKEN_PROGRAM_ID);
    }

    /// Layout sanity — the args are positional Borsh; field reordering would silently
    /// produce wrong bytes. Pin the leading bytes for known inputs.
    #[test]
    fn mint_args_serializes_in_field_order() {
        let args = MintArgs {
            asset_id: 0x0102_0304,
            value_after_fee: 0x1112_1314_1516_1718,
            recipient: [0xAB; 32],
            nonce: 0x2122_2324_2526_2728,
            federation_indices: vec![0, 1, 2, 3, 4],
        };
        let bytes = borsh::to_vec(&args).unwrap();
        // 4 + 8 + 32 + 8 + (4 + 5) = 61
        assert_eq!(bytes.len(), 61);
        assert_eq!(&bytes[0..4], &0x0102_0304u32.to_le_bytes());
        assert_eq!(&bytes[4..12], &0x1112_1314_1516_1718u64.to_le_bytes());
        assert_eq!(&bytes[12..44], &[0xABu8; 32]);
        assert_eq!(&bytes[44..52], &0x2122_2324_2526_2728u64.to_le_bytes());
        // Borsh encodes Vec<u8> as len_le_u32 || bytes
        assert_eq!(&bytes[52..56], &5u32.to_le_bytes());
        assert_eq!(&bytes[56..61], &[0u8, 1, 2, 3, 4]);
    }

}

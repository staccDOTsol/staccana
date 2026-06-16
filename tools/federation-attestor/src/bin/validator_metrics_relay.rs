//! `staccana-validator-metrics-relay` — federation publisher for validator
//! metrics (the `update_validator_metrics` ix).
//!
//! Sibling of `bridge_mint_relay` / `bridge_release_relay`. Same single-host
//! "federation seat" architecture: all 9 federation signer keys live on
//! val-1, this binary picks the first M of them and constructs an attested
//! tx in one shot. A multi-host federation would need real signature
//! aggregation; out of scope for v1.
//!
//! ## Wire format vs. bridge
//!
//! The bridge handler decodes via `parse_ed25519_batch_at` — one batched
//! precompile ix carrying M signatures. The validator-subsidy handler
//! decodes via `parse_ed25519_at` and walks back M precompile ixs, each
//! with `num_signatures = 1`. So this relay emits **M individual**
//! precompile ixs (not one batch) immediately preceding the
//! `update_validator_metrics` ix in the same tx.
//!
//! ## Metrics semantics on staccana
//!
//! On native-stake chains, `(uptime_bps, delegated_stake, votes_cast)`
//! map to observable RPC fields and the federation just relays. Staccana
//! has native stake disabled (no new stake accounts post-genesis), so the
//! federation's role here is broader: it attests to "this validator is
//! participating in good faith" via whatever heuristics it agrees to —
//! gossip presence, RPC liveness, attested epoch attendance, etc.
//!
//! For the v1 launch this binary takes the metric values as args. Once we
//! wire in proper observation (RPC poll for vote credits, gossip for
//! liveness, etc.) the daemon mode (`--daemon`) will compute them
//! automatically per epoch. Until then, treat this as the privileged
//! manual path that replaces `admin_set_validator_metrics`.
//!
//! ## Usage
//!
//! ```bash
//! staccana-validator-metrics-relay attest \
//!   --validator <identity-pubkey> \
//!   --uptime-bps 10000 \
//!   --delegated-stake 1000000000 \
//!   --votes-cast 1 \
//!   --federation-dir /etc/staccana/federation \
//!   --keypair /path/to/relayer.json \
//!   --rpc http://127.0.0.1:8899
//! ```
//!
//! The relayer keypair pays tx fees and is otherwise unprivileged (the
//! ix is permissionless on the on-chain side — verification is in the M
//! signatures).

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use borsh::BorshSerialize;
use clap::{Parser, Subcommand};
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

const SUBSIDY_PROGRAM_ID: &str = "Subsidy111111111111111111111111111111111111";

/// Mirrors `staccana_validator_subsidy::subsidy::METRICS_DOMAIN`. Hardcoded
/// here rather than dep-pulling the program crate (which drags Anchor in).
const METRICS_DOMAIN: &[u8] = b"STACCANA_VALIDATOR_METRICS_V1";

/// ed25519 precompile layout — shared with the bridge relays.
const ED25519_PUBKEY_SIZE: usize = 32;
const ED25519_SIGNATURE_SIZE: usize = 64;
const ED25519_OFFSETS_SIZE: usize = 14;

#[derive(Parser)]
#[command(name = "staccana-validator-metrics-relay")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sign + submit one `update_validator_metrics` attestation.
    Attest {
        /// Validator identity pubkey to attest for.
        #[arg(long)]
        validator: String,

        /// Uptime in basis points (10_000 = 100%).
        #[arg(long, default_value_t = 10_000u16)]
        uptime_bps: u16,

        /// Delegated stake (lamports). On staccana with native stake disabled
        /// this is whatever value the federation agrees on for "active
        /// participation"; default of 1 SOL == 1e9 makes total_weight nonzero
        /// without skewing the share math.
        #[arg(long, default_value_t = 1_000_000_000u64)]
        delegated_stake: u64,

        /// Votes cast this window. Federation-attested heuristic; default 1
        /// for "alive".
        #[arg(long, default_value_t = 1u64)]
        votes_cast: u64,

        /// Slot to record on the message. Defaults to current cluster slot.
        #[arg(long)]
        slot: Option<u64>,

        /// Nonce. Strictly increasing per validator. Defaults to
        /// `ValidatorRecord.last_metrics_nonce + 1`.
        #[arg(long)]
        nonce: Option<u64>,

        /// Directory containing `signer-1.json` … `signer-N.json`.
        #[arg(long, default_value = "/etc/staccana/federation")]
        federation_dir: PathBuf,

        /// Relayer keypair (pays tx fees + provides the Signer<'_> for
        /// the on-chain ix). Permissionless on chain.
        #[arg(long)]
        keypair: PathBuf,

        /// RPC endpoint.
        #[arg(long, default_value = "http://127.0.0.1:8899")]
        rpc: String,
    },
}

fn anchor_discriminator(name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{name}").as_bytes());
    let out = h.finalize();
    let mut d = [0u8; 8];
    d.copy_from_slice(&out[..8]);
    d
}

fn pda(seeds: &[&[u8]], program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(seeds, program_id).0
}

/// Build the canonical `STACCANA_VALIDATOR_METRICS_V1` byte string. Mirrors
/// `staccana_validator_subsidy::subsidy::build_metrics_message` exactly —
/// any drift here means the on-chain handler rejects with
/// `BadAttestationMessage`.
fn build_metrics_message(
    validator: &[u8; 32],
    uptime_bps: u16,
    delegated_stake: u64,
    votes_cast: u64,
    slot: u64,
    nonce: u64,
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(METRICS_DOMAIN.len() + 32 + 2 + 8 + 8 + 8 + 8);
    msg.extend_from_slice(METRICS_DOMAIN);
    msg.extend_from_slice(validator);
    msg.extend_from_slice(&uptime_bps.to_le_bytes());
    msg.extend_from_slice(&delegated_stake.to_le_bytes());
    msg.extend_from_slice(&votes_cast.to_le_bytes());
    msg.extend_from_slice(&slot.to_le_bytes());
    msg.extend_from_slice(&nonce.to_le_bytes());
    msg
}

/// Build a batched ed25519 precompile ix carrying M signatures over a
/// single shared message. Mirrors the bridge's
/// `build_ed25519_batch_precompile_ix`. The validator-subsidy program now
/// uses `parse_ed25519_batch_at` to decode (sibling commit), so M
/// signatures fit in one precompile ix instead of M — saving 75 × (M-1)
/// bytes of tx data, which is the difference between fitting in the
/// 1232-byte tx limit at M=5 and not.
fn build_batched_precompile_ix(keypairs: &[&Keypair], message: &[u8]) -> Instruction {
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
    // data[1] = 0 padding (already zero)

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

#[derive(BorshSerialize)]
struct UpdateValidatorMetricsArgs {
    validator: [u8; 32],
    uptime_bps: u16,
    delegated_stake: u64,
    votes_cast: u64,
    slot: u64,
    nonce: u64,
    federation_indices: Vec<u8>,
}

fn read_subsidy_config_federation(
    rpc: &RpcClient,
    program_id: &Pubkey,
) -> Result<(u8, u8, Vec<Pubkey>)> {
    let cfg_pda = pda(&[b"subsidy_config"], program_id);
    let acct = rpc.get_account(&cfg_pda).context("read SubsidyConfig")?;
    let d = &acct.data;
    if d.len() < 142 {
        return Err(anyhow!("SubsidyConfig account too small"));
    }
    let m = d[140];
    let n = d[141];
    let mut members = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        let off = 142 + i * 32;
        if d.len() < off + 32 {
            return Err(anyhow!("SubsidyConfig truncated at federation member {i}"));
        }
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&d[off..off + 32]);
        members.push(Pubkey::new_from_array(pk));
    }
    Ok((m, n, members))
}

fn read_validator_record_nonce(
    rpc: &RpcClient,
    program_id: &Pubkey,
    validator: &Pubkey,
) -> Result<u64> {
    let rec_pda = pda(&[b"validator", validator.as_ref()], program_id);
    let acct = match rpc.get_account(&rec_pda) {
        Ok(a) => a,
        Err(_) => return Ok(0),
    };
    // Layout: 8 disc + 32 validator + 2 uptime + 8 stake + 8 votes
    //       + 8 last_metrics_slot + 8 last_metrics_nonce + ...
    let nonce_off = 8 + 32 + 2 + 8 + 8 + 8;
    if acct.data.len() < nonce_off + 8 {
        return Ok(0);
    }
    Ok(u64::from_le_bytes(
        acct.data[nonce_off..nonce_off + 8]
            .try_into()
            .map_err(|_| anyhow!("nonce slice"))?,
    ))
}

/// Match each provided keypair against the on-chain federation member set
/// and return `(keypair_idx_in_set, original_idx)` pairs for the first M
/// matches. Returns an error if fewer than M keypairs match.
fn pick_quorum<'a>(
    federation_keys: &'a [Keypair],
    federation_members: &[Pubkey],
    m: u8,
) -> Result<Vec<(u8, &'a Keypair)>> {
    let mut picks: Vec<(u8, &Keypair)> = Vec::new();
    for kp in federation_keys.iter() {
        let kp_pk = kp.pubkey();
        if let Some(idx) = federation_members.iter().position(|p| *p == kp_pk) {
            picks.push((idx as u8, kp));
            if picks.len() == m as usize {
                break;
            }
        }
    }
    if picks.len() < m as usize {
        return Err(anyhow!(
            "found only {} federation keys matching on-chain set; need {}",
            picks.len(),
            m
        ));
    }
    Ok(picks)
}

fn load_federation_keys(dir: &PathBuf) -> Result<Vec<Keypair>> {
    let mut keys = Vec::new();
    for n in 1..=32u32 {
        let p = dir.join(format!("signer-{n}.json"));
        if !p.exists() {
            break;
        }
        let kp = read_keypair_file(&p)
            .map_err(|e| anyhow!("read federation key {}: {}", p.display(), e))?;
        keys.push(kp);
    }
    if keys.is_empty() {
        return Err(anyhow!(
            "no federation keys found in {}; expected signer-1.json, signer-2.json, …",
            dir.display()
        ));
    }
    Ok(keys)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Attest {
            validator,
            uptime_bps,
            delegated_stake,
            votes_cast,
            slot,
            nonce,
            federation_dir,
            keypair,
            rpc,
        } => {
            let validator_pk = Pubkey::from_str(&validator)?;
            let program_id = Pubkey::from_str(SUBSIDY_PROGRAM_ID)?;
            let payer = read_keypair_file(&keypair)
                .map_err(|e| anyhow!("read relayer keypair {}: {}", keypair.display(), e))?;
            let rpc_client =
                RpcClient::new_with_commitment(rpc.clone(), CommitmentConfig::confirmed());

            let (m, n, federation_members) =
                read_subsidy_config_federation(&rpc_client, &program_id)?;
            eprintln!(
                "[validator-metrics-relay] federation: M-of-N = {}-of-{}",
                m, n
            );

            let federation_keys = load_federation_keys(&federation_dir)?;
            eprintln!(
                "[validator-metrics-relay] loaded {} federation keypairs from {}",
                federation_keys.len(),
                federation_dir.display()
            );

            let quorum = pick_quorum(&federation_keys, &federation_members, m)?;

            let resolved_slot = match slot {
                Some(s) => s,
                None => rpc_client.get_slot().context("get_slot")?,
            };
            let resolved_nonce = match nonce {
                Some(n) => n,
                None => {
                    let prev =
                        read_validator_record_nonce(&rpc_client, &program_id, &validator_pk)?;
                    prev + 1
                }
            };
            eprintln!(
                "[validator-metrics-relay] validator: {}",
                validator_pk
            );
            eprintln!(
                "[validator-metrics-relay] uptime_bps={} delegated_stake={} votes_cast={} slot={} nonce={}",
                uptime_bps, delegated_stake, votes_cast, resolved_slot, resolved_nonce
            );

            let validator_bytes = validator_pk.to_bytes();
            let message = build_metrics_message(
                &validator_bytes,
                uptime_bps,
                delegated_stake,
                votes_cast,
                resolved_slot,
                resolved_nonce,
            );

            // Build a SINGLE batched ed25519 precompile ix with all M
            // signatures, then the update_validator_metrics ix. Order
            // matters — the on-chain handler reads the precompile ix
            // immediately preceding itself.
            let kp_refs: Vec<&Keypair> = quorum.iter().map(|(_, kp)| *kp).collect();
            let federation_indices: Vec<u8> =
                quorum.iter().map(|(idx, _)| *idx).collect();
            let mut ixs: Vec<Instruction> = Vec::with_capacity(2);
            ixs.push(build_batched_precompile_ix(&kp_refs, &message));

            let cfg_pda = pda(&[b"subsidy_config"], &program_id);
            let rec_pda = pda(&[b"validator", validator_pk.as_ref()], &program_id);
            let args = UpdateValidatorMetricsArgs {
                validator: validator_bytes,
                uptime_bps,
                delegated_stake,
                votes_cast,
                slot: resolved_slot,
                nonce: resolved_nonce,
                federation_indices,
            };
            let mut data = anchor_discriminator("update_validator_metrics").to_vec();
            args.serialize(&mut data)?;

            let update_ix = Instruction {
                program_id,
                accounts: vec![
                    AccountMeta::new_readonly(payer.pubkey(), true), // relayer (signer)
                    AccountMeta::new_readonly(cfg_pda, false),       // subsidy_config
                    AccountMeta::new(rec_pda, false),                // validator_record (mut)
                    AccountMeta::new_readonly(sysvar::instructions::ID, false),
                ],
                data,
            };
            ixs.push(update_ix);

            let bh = rpc_client.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(
                &ixs,
                Some(&payer.pubkey()),
                &[&payer],
                bh,
            );
            eprintln!(
                "[validator-metrics-relay] sending tx with {} ed25519 sigs + 1 update ix",
                m
            );
            let sig = rpc_client.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
        }
    }
    Ok(())
}

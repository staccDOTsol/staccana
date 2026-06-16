//! Command-line entry for the staccana lazy-claim CLI.
//!
//! Usage:
//!
//! ```text
//! staccana-claim-cli \
//!     --keypair  ~/.config/solana/id.json \
//!     --snapshot snap.json \
//!     --rpc      https://mp.fun/
//! ```
//!
//! See the crate-level docs in `lib.rs` for the end-to-end flow. This binary is intentionally
//! thin — every code path it exercises is covered by tests in the library modules.
//!
//! The integrator is expected to register the `staccana-claim-cli` crate in the workspace
//! `Cargo.toml`; this binary does not modify that file.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use solana_client::rpc_client::RpcClient;
use solana_program::pubkey::Pubkey;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signature::read_keypair_file;
use solana_sdk::signer::Signer;

use staccana_claim_cli::{
    build_claim_instruction, build_claim_message, build_ed25519_precompile_instruction,
    build_inclusion_proof, claimed_marker_pda, load_snapshot_accounts, partition_claimable,
    submit_claim_transaction, ClaimArgs, LAZY_CLAIM_PROGRAM_ID,
};

#[derive(Parser, Debug)]
#[command(
    name = "staccana-claim-cli",
    about = "Claim a staccana account using your mainnet keypair.",
    version
)]
struct Args {
    /// Path to the user's mainnet keypair file (Solana JSON keypair format).
    #[arg(long)]
    keypair: PathBuf,
    /// Path to the staccana snapshot JSON file.
    #[arg(long)]
    snapshot: PathBuf,
    /// Staccana RPC endpoint.
    #[arg(long)]
    rpc: String,
    /// Optional: lazy-claim program state account holding `claimable_root`.
    /// If omitted, derived from `["state"]` against `LAZY_CLAIM_PROGRAM_ID`.
    #[arg(long)]
    program_state: Option<String>,
    /// Optional: treasury PDA. If omitted, derived from `["treasury"]` against
    /// the placeholder treasury program (will need a real treasury program ID
    /// once that lands).
    #[arg(long)]
    treasury_pda: Option<String>,
}

fn parse_pubkey(s: &str) -> Result<Pubkey> {
    let bytes = bs58::decode(s)
        .into_vec()
        .with_context(|| format!("invalid base58 pubkey: {s:?}"))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "pubkey {s:?} did not decode to 32 bytes (got {})",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(Pubkey::new_from_array(arr))
}

fn main() -> Result<()> {
    let args = Args::parse();

    // 1. Load the user's mainnet keypair.
    let keypair = read_keypair_file(&args.keypair)
        .map_err(|e| anyhow!("failed to read keypair file {:?}: {e}", args.keypair))?;
    let user_pubkey = keypair.pubkey();
    eprintln!("Loaded keypair for {user_pubkey}");

    // 2. Load the snapshot and partition for claimable accounts.
    let raw = load_snapshot_accounts(&args.snapshot)
        .with_context(|| format!("loading snapshot from {:?}", args.snapshot))?;
    eprintln!("Snapshot rows: {}", raw.len());
    let claimable = partition_claimable(&raw).context("partitioning claimable accounts")?;
    eprintln!("Claimable accounts: {}", claimable.len());

    // 3. Build the inclusion proof for the user's pubkey.
    let proof = build_inclusion_proof(&claimable, &user_pubkey)
        .with_context(|| format!("building inclusion proof for {user_pubkey}"))?;
    let recomputed = proof.recomputed_root();
    if recomputed != proof.root {
        return Err(anyhow!(
            "self-check failed: recomputed root does not match computed root",
        ));
    }
    eprintln!(
        "Built inclusion proof: lamports={}, depth={}, root={}",
        proof.lamports,
        proof.proof.len(),
        bs58::encode(proof.root.to_bytes()).into_string(),
    );

    // 4. Build the ed25519 precompile ix signing the claim message.
    let message = build_claim_message(&user_pubkey, proof.lamports);
    let ed25519_ix = build_ed25519_precompile_instruction(&keypair, &message);

    // 5. Build the claim ix.
    let program_state = match args.program_state.as_deref() {
        Some(s) => parse_pubkey(s)?,
        None => Pubkey::find_program_address(&[b"state"], &LAZY_CLAIM_PROGRAM_ID).0,
    };
    let treasury_pda = match args.treasury_pda.as_deref() {
        Some(s) => parse_pubkey(s)?,
        // TODO: derive from the real TREASURY_PROGRAM_ID once SPEC §2.1 fills it in.
        None => Pubkey::find_program_address(&[b"treasury"], &LAZY_CLAIM_PROGRAM_ID).0,
    };
    let claimed_marker = claimed_marker_pda(&user_pubkey);
    let claim_args = ClaimArgs::new(
        user_pubkey,
        proof.lamports,
        proof.proof.clone(),
        proof.proof_flags.clone(),
    );
    let claim_ix = build_claim_instruction(
        &claim_args,
        program_state,
        treasury_pda,
        claimed_marker,
        keypair.pubkey(),
    )
    .context("building claim instruction")?;

    // 6. Submit.
    let rpc = RpcClient::new_with_commitment(args.rpc.clone(), CommitmentConfig::confirmed());
    eprintln!("Submitting claim transaction to {} ...", args.rpc);
    let signature = submit_claim_transaction(&rpc, &keypair, ed25519_ix, claim_ix)
        .context("submitting claim transaction")?;
    println!("{signature}");
    Ok(())
}

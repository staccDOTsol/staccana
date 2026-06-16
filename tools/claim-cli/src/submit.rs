//! RPC submission wrapper for the claim transaction.
//!
//! Bundles the ed25519 precompile ix and the claim ix into a single transaction. The
//! transaction is signed by the user's mainnet keypair so it doubles as the fee payer
//! identity — in practice the lazy-claim program redirects fees to the treasury PDA
//! (`docs/SPEC.md` §4.4) so the user does not need a staccana balance to claim, but the
//! signature is still required to prove account ownership.
//!
//! Production callers should run a `simulateTransaction` before sending; this module exposes
//! a single entrypoint that does send-and-confirm. Tests in this crate stop at the boundary
//! of constructing the transaction object and avoid hitting the network.

use solana_client::rpc_client::RpcClient;
use solana_program::instruction::Instruction;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

/// All errors the submit module can produce.
#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    #[error("RPC client error: {0}")]
    Rpc(#[from] solana_client::client_error::ClientError),
}

/// Build, sign, and submit a transaction containing exactly the ed25519 precompile ix
/// followed by the claim ix.
///
/// Spec §4.4: this is the structure that triggers gas exemption — exactly one ed25519
/// precompile and one claim ix targeting the same payload, no extras.
pub fn submit_claim_transaction(
    rpc: &RpcClient,
    payer: &Keypair,
    ed25519_ix: Instruction,
    claim_ix: Instruction,
) -> Result<Signature, SubmitError> {
    let recent_blockhash = rpc.get_latest_blockhash()?;
    let mut tx = Transaction::new_with_payer(&[ed25519_ix, claim_ix], Some(&payer.pubkey()));
    tx.sign(&[payer], recent_blockhash);
    let signature = rpc.send_and_confirm_transaction_with_spinner_and_commitment(
        &tx,
        CommitmentConfig::confirmed(),
    )?;
    Ok(signature)
}

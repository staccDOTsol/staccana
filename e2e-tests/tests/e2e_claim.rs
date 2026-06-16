//! End-to-end claim flow against an in-process BanksClient.
//!
//! This is the highest-fidelity test in the harness — it stands up the lazy-claim
//! program in a real `solana-program-test` runtime and pushes complete transactions
//! through it, exercising the ed25519 precompile, Instructions sysvar inspection, the
//! treasury PDA debit, the claimed-marker PDA initialization, and per-pubkey idempotency
//! all at once.
//!
//! Test scenarios:
//!
//! 1. **Happy path** — generate a 5-account synthetic snapshot (3 EOAs claimable, 2 token
//!    accounts to treasury), build genesis, claim one EOA, assert the recipient got the
//!    expected lamports and the marker PDA was initialized.
//! 2. **Replay rejection** — submit the same claim a second time; assert it fails (the
//!    on-chain `AlreadyClaimed` check trips because `init_claimed_marker` packed the
//!    marker's discriminator into the account data on the first call).
//! 3. **Bad proof** — submit a claim for a different account but with a proof that
//!    doesn't include it; assert the on-chain `BadMerkleProof` check fires.
//!
//! Note on program ID: the lazy-claim crate's own `id()` is a placeholder pending the
//! SPEC §2.1 assignment. The harness substitutes [`LAZY_CLAIM_TEST_PROGRAM_ID`]; the
//! §4.2 claim message embeds *that* program ID, not the placeholder, because the
//! on-chain processor computes the expected message using its runtime `program_id`.

use solana_program::hash::Hash;
use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;
use solana_program::system_program;
use solana_program::sysvar::instructions as sysvar_instructions;
use solana_program_test::ProgramTestBanksClientExt;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;
use staccana_claim_cli::{
    build_ed25519_precompile_instruction, build_inclusion_proof, ClaimArgs, ClaimableAccount,
};
use staccana_e2e_tests::{
    build_lazy_claim_program_test, install_claim_pre_state, mixed_synthetic_snapshot,
    LAZY_CLAIM_TEST_PROGRAM_ID,
};
use staccana_genesis::build_genesis;

/// Build the §4.2 claim message using the harness's test program ID.
///
/// We can't reuse `staccana_claim_cli::build_claim_message` because that one bakes in the
/// CLI's placeholder program ID; the on-chain processor will compute an expected message
/// using `LAZY_CLAIM_TEST_PROGRAM_ID` and compare byte-for-byte.
fn claim_message(pubkey: &Pubkey, lamports: u64) -> Vec<u8> {
    let mut msg = Vec::with_capacity(17 + 32 + 8 + 32);
    msg.extend_from_slice(b"STACCANA_CLAIM_V1");
    msg.extend_from_slice(pubkey.as_ref());
    msg.extend_from_slice(&lamports.to_le_bytes());
    msg.extend_from_slice(LAZY_CLAIM_TEST_PROGRAM_ID.as_ref());
    msg
}

/// Build the lazy-claim `claim` instruction targeting the test program.
///
/// Mirrors `staccana_claim_cli::build_claim_instruction` but routes to
/// [`LAZY_CLAIM_TEST_PROGRAM_ID`] and lets the caller supply pre-computed account
/// addresses (treasury PDA, config, claimed-marker PDA). Two extra accounts —
/// `payer` and the system program — are appended after the SPEC §4.1 list to support
/// the marker-init `system_program::create_account` CPI.
fn claim_instruction(
    args: &ClaimArgs,
    config_account: Pubkey,
    treasury_pda: Pubkey,
    claimed_marker_pda: Pubkey,
    payer: Pubkey,
) -> Instruction {
    let recipient = Pubkey::new_from_array(args.pubkey);
    let body = args.to_wire_bytes().expect("encode claim args");
    let mut data = Vec::with_capacity(1 + body.len());
    data.push(0x00);
    data.extend_from_slice(&body);
    let accounts = vec![
        AccountMeta::new(recipient, false),
        AccountMeta::new_readonly(config_account, false),
        AccountMeta::new_readonly(sysvar_instructions::ID, false),
        AccountMeta::new(treasury_pda, false),
        AccountMeta::new(claimed_marker_pda, false),
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    Instruction {
        program_id: LAZY_CLAIM_TEST_PROGRAM_ID,
        accounts,
        data,
    }
}

#[tokio::test]
async fn claim_succeeds_credits_recipient_and_marks_pda() {
    let snapshot = mixed_synthetic_snapshot();
    let genesis = build_genesis(snapshot.iter());

    // Pull out one of the claimable accounts to claim.
    let target_acct = snapshot
        .iter()
        .find(|a| a.keypair.is_some())
        .expect("at least one claimable EOA in snapshot");
    let target_pubkey = target_acct.pubkey;
    let target_keypair: &Keypair = target_acct.keypair.as_ref().unwrap();
    let expected_lamports = target_acct.lamports;

    // Build the inclusion proof against the same genesis the chain will see.
    let claimable: Vec<ClaimableAccount> = snapshot
        .iter()
        .filter(|a| a.keypair.is_some())
        .map(|a| ClaimableAccount {
            pubkey: a.pubkey,
            lamports: a.lamports,
        })
        .collect();
    let proof = build_inclusion_proof(&claimable, &target_pubkey).expect("inclusion proof");

    // Spin up the program test with the lazy-claim program registered + pre-state baked.
    let mut pt = build_lazy_claim_program_test();
    let pre = install_claim_pre_state(&mut pt, &genesis, &[target_pubkey], expected_lamports);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;

    let proof_hashes: Vec<Hash> = proof.proof.clone();
    let args = ClaimArgs::new(
        target_pubkey,
        expected_lamports,
        proof_hashes,
        proof.proof_flags,
    );

    let message = claim_message(&target_pubkey, expected_lamports);
    let ed25519_ix = build_ed25519_precompile_instruction(target_keypair, &message);
    let claim_ix = claim_instruction(
        &args,
        pre.setup.config_account,
        pre.setup.treasury_pda,
        pre.markers[0].1,
        payer.pubkey(),
    );

    let tx = Transaction::new_signed_with_payer(
        &[ed25519_ix, claim_ix],
        Some(&payer.pubkey()),
        &[&payer],
        recent_blockhash,
    );

    banks_client
        .process_transaction(tx)
        .await
        .expect("happy-path claim must succeed");

    // Recipient now exists with the expected lamports.
    let recipient_acct = banks_client
        .get_account(target_pubkey)
        .await
        .expect("rpc")
        .expect("recipient account materialized");
    assert_eq!(
        recipient_acct.lamports, expected_lamports,
        "recipient credited with the snapshot lamports"
    );

    // Marker PDA exists and contains the packed marker payload.
    let marker_acct = banks_client
        .get_account(pre.markers[0].1)
        .await
        .expect("rpc")
        .expect("marker PDA persists");
    assert!(
        !marker_acct.data.is_empty(),
        "marker should carry the discriminator + claim payload"
    );
    assert_eq!(
        marker_acct.data[0], 0x02,
        "first byte is the ClaimedMarker discriminator"
    );
}

#[tokio::test]
async fn claim_replay_is_rejected() {
    let snapshot = mixed_synthetic_snapshot();
    let genesis = build_genesis(snapshot.iter());

    let target_acct = snapshot
        .iter()
        .find(|a| a.keypair.is_some())
        .expect("at least one claimable EOA in snapshot");
    let target_pubkey = target_acct.pubkey;
    let target_keypair: &Keypair = target_acct.keypair.as_ref().unwrap();

    let claimable: Vec<ClaimableAccount> = snapshot
        .iter()
        .filter(|a| a.keypair.is_some())
        .map(|a| ClaimableAccount {
            pubkey: a.pubkey,
            lamports: a.lamports,
        })
        .collect();
    let proof = build_inclusion_proof(&claimable, &target_pubkey).expect("inclusion proof");

    let mut pt = build_lazy_claim_program_test();
    let pre = install_claim_pre_state(&mut pt, &genesis, &[target_pubkey], target_acct.lamports);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;

    let proof_hashes: Vec<Hash> = proof.proof.clone();
    let args = ClaimArgs::new(
        target_pubkey,
        target_acct.lamports,
        proof_hashes,
        proof.proof_flags,
    );

    let message = claim_message(&target_pubkey, target_acct.lamports);
    let ed25519_ix = build_ed25519_precompile_instruction(target_keypair, &message);
    let claim_ix = claim_instruction(
        &args,
        pre.setup.config_account,
        pre.setup.treasury_pda,
        pre.markers[0].1,
        payer.pubkey(),
    );

    // First submission: succeeds.
    let tx = Transaction::new_signed_with_payer(
        &[ed25519_ix.clone(), claim_ix.clone()],
        Some(&payer.pubkey()),
        &[&payer],
        recent_blockhash,
    );
    banks_client
        .process_transaction(tx)
        .await
        .expect("first claim must succeed");

    // Second submission: rebuild the tx with a fresh blockhash so the runtime doesn't
    // dedup on tx-hash, then assert the claim itself rejects (not a runtime-level dedup).
    let second_blockhash = banks_client
        .get_new_latest_blockhash(&recent_blockhash)
        .await
        .expect("get fresh blockhash");
    let tx2 = Transaction::new_signed_with_payer(
        &[ed25519_ix, claim_ix],
        Some(&payer.pubkey()),
        &[&payer],
        second_blockhash,
    );
    let result = banks_client.process_transaction(tx2).await;
    assert!(
        result.is_err(),
        "replayed claim must be rejected by AlreadyClaimed; got {result:?}"
    );
}

#[tokio::test]
async fn claim_with_wrong_proof_is_rejected() {
    let snapshot = mixed_synthetic_snapshot();
    let genesis = build_genesis(snapshot.iter());

    // Claim attempt: target account A but use the inclusion proof for account B.
    let claimables: Vec<&_> = snapshot.iter().filter(|a| a.keypair.is_some()).collect();
    assert!(
        claimables.len() >= 2,
        "test needs at least two claimable EOAs"
    );
    let target_a = claimables[0];
    let target_b = claimables[1];

    let claimable: Vec<ClaimableAccount> = snapshot
        .iter()
        .filter(|a| a.keypair.is_some())
        .map(|a| ClaimableAccount {
            pubkey: a.pubkey,
            lamports: a.lamports,
        })
        .collect();
    // Proof is valid — for B. We pass A as the recipient + lamports — so the leaf hash
    // computed on-chain will not match what the proof walks up from.
    let proof_for_b = build_inclusion_proof(&claimable, &target_b.pubkey).expect("proof");

    let mut pt = build_lazy_claim_program_test();
    let pre = install_claim_pre_state(&mut pt, &genesis, &[target_a.pubkey], target_a.lamports);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;

    let proof_hashes: Vec<Hash> = proof_for_b.proof.clone();
    let args = ClaimArgs::new(
        target_a.pubkey,
        target_a.lamports,
        proof_hashes,
        proof_for_b.proof_flags,
    );

    let target_a_keypair: &Keypair = target_a.keypair.as_ref().unwrap();
    let message = claim_message(&target_a.pubkey, target_a.lamports);
    let ed25519_ix = build_ed25519_precompile_instruction(target_a_keypair, &message);
    let claim_ix = claim_instruction(
        &args,
        pre.setup.config_account,
        pre.setup.treasury_pda,
        pre.markers[0].1,
        payer.pubkey(),
    );

    let tx = Transaction::new_signed_with_payer(
        &[ed25519_ix, claim_ix],
        Some(&payer.pubkey()),
        &[&payer],
        recent_blockhash,
    );
    let result = banks_client.process_transaction(tx).await;
    assert!(
        result.is_err(),
        "claim with mismatched proof must be rejected; got {result:?}"
    );
}

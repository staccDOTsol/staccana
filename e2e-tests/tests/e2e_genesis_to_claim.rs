//! End-to-end pipeline walkthrough: synthetic JSON snapshot → MockSnapshot →
//! `build_genesis` → `genesis_emit::compose` → on-chain claim of every claimable
//! account → SOL conservation invariant check.
//!
//! This test chains together what every crate in the pipeline produces:
//!
//! 1. **Synthetic snapshot** is serialized to a tempfile in the JSON format
//!    `staccana_snapshot_fork::mock::MockSnapshot` consumes.
//! 2. **MockSnapshot** loads the file back into `AccountRecord`s.
//! 3. **`build_genesis`** consumes the records and produces a `GenesisOutput`.
//! 4. **`genesis_emit::compose`** turns the output into the typed `ComposedGenesis`
//!    handoff struct (we sanity-check the composition; the actual `genesis.bin` write is
//!    out of scope for v0).
//! 5. **`ProgramTest`** is set up with the lazy-claim program registered; the harness
//!    pre-state mirrors what genesis would install at slot 0.
//! 6. **Every claimable account** is claimed via a separate transaction.
//! 7. **Invariant I1** is asserted: total claimed lamports + remaining treasury balance
//!    == sum of all snapshot lamports.

use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;
use solana_program::system_program;
use solana_program::sysvar::instructions as sysvar_instructions;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;
use staccana_claim_cli::{
    build_ed25519_precompile_instruction, build_inclusion_proof, ClaimArgs, ClaimableAccount,
};
use staccana_e2e_tests::{
    build_lazy_claim_program_test, install_claim_pre_state, mixed_synthetic_snapshot,
    snapshot_to_json, LAZY_CLAIM_TEST_PROGRAM_ID,
};
use staccana_genesis::{build_genesis, partition::Account};
use staccana_genesis_emit::compose;
use staccana_snapshot_fork::mock::MockSnapshot;

fn claim_message(pubkey: &Pubkey, lamports: u64) -> Vec<u8> {
    let mut msg = Vec::with_capacity(17 + 32 + 8 + 32);
    msg.extend_from_slice(b"STACCANA_CLAIM_V1");
    msg.extend_from_slice(pubkey.as_ref());
    msg.extend_from_slice(&lamports.to_le_bytes());
    msg.extend_from_slice(LAZY_CLAIM_TEST_PROGRAM_ID.as_ref());
    msg
}

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
async fn full_pipeline_claim_all_and_verify_sol_conservation() {
    // (1) Build the synthetic snapshot in-memory and serialize to a tempfile.
    let snapshot = mixed_synthetic_snapshot();
    let json = snapshot_to_json(&snapshot);
    let tmp = tempfile_with_contents(&json);

    // (2) Load it back through MockSnapshot to exercise the snapshot-fork loader.
    let mock = MockSnapshot::new(tmp.path());
    let records = mock.load_records().expect("load mock snapshot");
    assert_eq!(records.len(), snapshot.len(), "mock load preserves count");

    // Sanity: the loaded records partition the same way as our in-memory snapshot.
    let total_lamports_loaded: u128 = records.iter().map(|r| r.lamports() as u128).sum();
    let total_lamports_in_memory: u128 = snapshot.iter().map(|a| a.lamports as u128).sum();
    assert_eq!(total_lamports_loaded, total_lamports_in_memory);

    // (3) Run the real `build_genesis` over the loaded records.
    let genesis = build_genesis(records);

    // (4) Compose into the typed handoff struct. We only verify the composition is
    //     consistent with the genesis output; emitting `genesis.bin` is out of v0 scope.
    let composed = compose(&genesis);
    assert_eq!(
        composed.lazy_claim_account.claimable_root,
        genesis.claimable_root.0.to_bytes(),
        "composed root matches genesis root"
    );
    assert_eq!(
        composed.treasury_pda_lamports,
        genesis.treasury.lamports_for_pda(),
        "composed treasury matches genesis treasury"
    );
    assert_eq!(composed.claimable_count, genesis.claimable_count as u64);

    // (5) Set up ProgramTest with claim pre-state for every claimable pubkey.
    let claimable_targets: Vec<Pubkey> = snapshot
        .iter()
        .filter(|a| a.keypair.is_some())
        .map(|a| a.pubkey)
        .collect();
    assert_eq!(
        claimable_targets.len(),
        genesis.claimable_count,
        "every claimable account from the synthetic set is in the genesis output"
    );

    // Total claimable lamports — needed by the harness so the pre-credited treasury PDA
    // has enough headroom to pay every claim (in addition to the treasury_total it already
    // carries from the non-claimable accounts). SPEC §8 I1 holds end-to-end: the treasury
    // PDA's starting balance equals snapshot_total, and after every claim equals
    // treasury_total.
    let claimable_pool: u64 = snapshot
        .iter()
        .filter(|a| a.keypair.is_some())
        .map(|a| a.lamports)
        .sum();

    let mut pt = build_lazy_claim_program_test();
    let pre = install_claim_pre_state(&mut pt, &genesis, &claimable_targets, claimable_pool);
    let (mut banks_client, payer, _initial_blockhash) = pt.start().await;

    // (6) Claim each account in its own transaction so we get independent blockhashes
    //     and the BanksClient runs them serially.
    let claimable: Vec<ClaimableAccount> = snapshot
        .iter()
        .filter(|a| a.keypair.is_some())
        .map(|a| ClaimableAccount {
            pubkey: a.pubkey,
            lamports: a.lamports,
        })
        .collect();

    let mut total_claimed: u64 = 0;
    for (i, target_acct) in snapshot.iter().filter(|a| a.keypair.is_some()).enumerate() {
        let target_kp: &Keypair = target_acct.keypair.as_ref().unwrap();
        let proof = build_inclusion_proof(&claimable, &target_acct.pubkey).expect("proof");
        let args = ClaimArgs::new(
            target_acct.pubkey,
            target_acct.lamports,
            proof.proof.clone(),
            proof.proof_flags,
        );

        let message = claim_message(&target_acct.pubkey, target_acct.lamports);
        let ed25519_ix = build_ed25519_precompile_instruction(target_kp, &message);
        let claim_ix = claim_instruction(
            &args,
            pre.setup.config_account,
            pre.setup.treasury_pda,
            pre.markers[i].1,
            payer.pubkey(),
        );

        let blockhash = banks_client
            .get_latest_blockhash()
            .await
            .expect("blockhash");
        let tx = Transaction::new_signed_with_payer(
            &[ed25519_ix, claim_ix],
            Some(&payer.pubkey()),
            &[&payer],
            blockhash,
        );
        banks_client
            .process_transaction(tx)
            .await
            .unwrap_or_else(|e| panic!("claim {} must succeed: {e:?}", target_acct.pubkey));
        total_claimed = total_claimed
            .checked_add(target_acct.lamports)
            .expect("total_claimed fits");
    }

    // (7) Invariant I1: claimed + remaining treasury == total snapshot lamports.
    //
    // Total snapshot lamports = claimable_total + treasury_total. The harness pre-credited
    // the treasury PDA with `treasury_total + claimable_pool` (= snapshot_total), and the
    // processor debited it once per claim. After every claim, remaining_treasury =
    // snapshot_total - total_claimed; with total_claimed == claimable_total in the happy
    // path that lands at exactly treasury_total. The assertion below restates I1 as
    // remaining_treasury + total_claimed == snapshot_total, which is the form that holds
    // for any partial-claim subset too.
    let treasury_acct = banks_client
        .get_account(pre.setup.treasury_pda)
        .await
        .expect("rpc")
        .expect("treasury PDA persists");
    let snapshot_total: u128 = snapshot.iter().map(|a| a.lamports as u128).sum();
    let claimable_total: u128 = snapshot
        .iter()
        .filter(|a| a.keypair.is_some())
        .map(|a| a.lamports as u128)
        .sum();
    let treasury_total: u128 = snapshot
        .iter()
        .filter(|a| a.keypair.is_none())
        .map(|a| a.lamports as u128)
        .sum();

    assert_eq!(
        total_claimed as u128, claimable_total,
        "total claimed equals claimable_total"
    );
    assert_eq!(
        treasury_acct.lamports as u128 + total_claimed as u128,
        treasury_total + claimable_total,
        "remaining treasury + total claimed == treasury + claimable (= snapshot total)"
    );
    assert_eq!(
        treasury_acct.lamports as u128 + total_claimed as u128,
        snapshot_total,
        "SPEC §8 I1 — genesis SOL conservation holds end-to-end"
    );
}

/// Write `contents` to an auto-cleaning tempfile and return the handle. The test holds
/// the handle for the duration of the run; `MockSnapshot` reads the path eagerly so the
/// drop-on-scope-exit semantics are fine.
fn tempfile_with_contents(contents: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .prefix("staccana-e2e-snapshot-")
        .suffix(".json")
        .tempfile()
        .expect("create tempfile");
    f.write_all(contents.as_bytes()).expect("write tempfile");
    f
}

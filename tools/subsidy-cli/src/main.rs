//! Two subcommands:
//!
//!   - `init`              : runs `init_subsidy` once. SubsidyConfig + ValidatorRegistry PDAs.
//!   - `register-validator`: appends a validator pubkey to the registry.
//!
//! Both ixs are gated on the staccana ADMIN_AUTHORITY (= upgrade-authority key).
//! Same Anchor-1.x discriminator + borsh wire as the on-chain program; we
//! avoid pulling the program crate as a dep so this binary stays small and
//! decoupled from anchor-lang's heavy compile.

use anyhow::{anyhow, Context};
use borsh::BorshSerialize;
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{read_keypair_file, write_keypair_file, Keypair, Signer},
    system_program,
    sysvar::instructions::ID as SYSVAR_INSTRUCTIONS_ID,
    transaction::Transaction,
};
use std::{path::PathBuf, str::FromStr};

const SUBSIDY_PROGRAM_ID: &str = "Subsidy111111111111111111111111111111111111";

#[derive(Parser)]
struct Cli {
    /// Fee payer + signer (must be the program's ADMIN_AUTHORITY for `init`,
    /// must be `subsidy_config.governance` for `register-validator`).
    #[arg(long)]
    keypair: PathBuf,

    /// RPC URL.
    #[arg(long, default_value = "http://localhost:8899")]
    rpc: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-shot init of `SubsidyConfig` + `ValidatorRegistry`.
    Init {
        /// Governance pubkey to bind into SubsidyConfig (later signer for
        /// register/stake/unstake). Defaults to the keypair's pubkey.
        #[arg(long)]
        governance: Option<String>,

        /// Bridge program ID (read from /etc/staccana/program-ids.json or pass
        /// LA7h3hjvD62MeTtdeE4h2vq3EGxbU1oqzHtewp4xb9b for the live deploy).
        #[arg(long)]
        bridge_program_id: String,

        /// Productive vault account (placeholder OK in v1 — no productive
        /// position yet).
        #[arg(long, default_value = "11111111111111111111111111111111")]
        productive_vault: String,

        /// Bridge `asset_id` (u32) of the productive position. Placeholder
        /// 0 OK in v1 since no productive position is wired yet.
        #[arg(long, default_value_t = 0u32)]
        productive_asset_id: u32,

        /// Total treasury lamports the bootstrap-reserve math is sized
        /// against. Defaults to the actual on-chain balance of the
        /// program's treasury PDA at init time.
        #[arg(long)]
        treasury_total: Option<u64>,

        /// Federation members JSON ({ threshold: M, pubkeys: [...] }).
        #[arg(long)]
        federation: PathBuf,
    },

    /// Append a validator identity pubkey to the registry.
    RegisterValidator {
        /// Validator identity pubkey.
        #[arg(long)]
        validator: String,
    },

    /// Remove a validator identity pubkey from the registry. Closes the
    /// per-validator `ValidatorRecord` PDA and refunds rent to the signer.
    UnregisterValidator {
        /// Validator identity pubkey.
        #[arg(long)]
        validator: String,
    },

    /// Governance: retune the bootstrap-reserve per-epoch drip rate.
    /// `bootstrap_distribute` will then pay `target_per_epoch` lamports
    /// total per epoch (split pro-rata across registered validators).
    SetBootstrapPerEpoch {
        /// Target lamports/epoch (TOTAL — per-validator share is this divided
        /// by the registered count weighted by metrics).
        #[arg(long)]
        target_per_epoch: u64,
    },

    /// Admin-only: set per-validator metrics directly (bypasses federation
    /// attestation). Bootstrap-only escape hatch — see the on-chain ix doc.
    AdminSetMetrics {
        #[arg(long)]
        validator: String,
        #[arg(long, default_value_t = 10_000u16)]
        uptime_bps: u16,
        #[arg(long)]
        delegated_stake: u64,
        #[arg(long)]
        votes_cast: u64,
    },

    /// Permissionless: trigger bootstrap_distribute for an epoch. Reads the
    /// registry from chain, builds the remaining_accounts array, signs +
    /// submits.
    BootstrapDistribute {
        #[arg(long)]
        epoch: u64,
    },

    /// One-shot migration: drain treasury → flip owner → refund.
    /// Builds a single tx with three ixs:
    ///   ix 0 lazy-claim::DrainTreasury(amount, authority)  — drains
    ///        treasury (owned by lazy-claim) into authority's wallet.
    ///   ix 1 validator-subsidy::migrate_treasury_owner    — calls
    ///        system_program::assign(treasury, validator-subsidy);
    ///        allowed because treasury.lamports = 0 in this slot.
    ///   ix 2 system_program::transfer(authority → treasury, amount) —
    ///        refunds the lamports back to the now-subsidy-owned PDA.
    /// Atomic: tx fails as a unit if any leg fails. Idempotent on success
    /// (lazy-claim's DrainTreasury rejects with `IllegalOwner` once
    /// treasury.owner != lazy-claim).
    MigrateTreasuryOwner {},

    /// Delegate native Solana stake from the staccana treasury to a
    /// validator's vote account. Drips via native warmup (~1 epoch).
    /// Repeat with smaller amounts for a more gradual schedule.
    /// NOTE: native Stake program CPI is currently disabled on this chain;
    /// kept for if/when stake creation is re-enabled.
    DelegateTreasuryStake {
        /// Vote account pubkey to delegate to.
        #[arg(long)]
        vote_account: String,

        /// Lamports to delegate (this call only).
        #[arg(long)]
        amount: u64,

        /// Path to a fresh keypair file for the new stake account. Will be
        /// created (and partial-signed) by this CLI. Save the resulting
        /// pubkey — needed for any future deactivate/withdraw.
        #[arg(long)]
        stake_account: PathBuf,
    },
}

#[derive(BorshSerialize)]
struct UnregisterValidatorArgs {
    validator: [u8; 32],
}

#[derive(BorshSerialize)]
struct SetBootstrapPerEpochArgs {
    target_per_epoch: u64,
}

#[derive(BorshSerialize)]
struct AdminSetMetricsArgs {
    validator: [u8; 32],
    uptime_bps: u16,
    delegated_stake: u64,
    votes_cast: u64,
}

#[derive(BorshSerialize)]
struct BootstrapDistributeArgs {
    epoch: u64,
}

#[derive(BorshSerialize)]
struct DelegateTreasuryStakeArgs {
    amount: u64,
}

#[derive(BorshSerialize)]
struct InitSubsidyArgs {
    governance: [u8; 32],
    bridge_program_id: [u8; 32],
    productive_vault: [u8; 32],
    productive_asset_id: u32, // bridge asset_id, NOT a pubkey
    treasury_total: u64,
    federation_m: u8,         // threshold first per on-chain struct order
    federation_n: u8,         // member count second
    federation_members: Vec<[u8; 32]>,
}

#[derive(BorshSerialize)]
struct RegisterValidatorArgs {
    validator: [u8; 32],
}

#[derive(serde::Deserialize)]
struct FederationFile {
    threshold: u8,
    pubkeys: Vec<String>,
}

fn discriminator(name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("global:{name}").as_bytes());
    let r = h.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&r[..8]);
    out
}

fn pda(seeds: &[&[u8]], program_id: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(seeds, program_id).0
}

fn pubkey_arr(s: &str) -> anyhow::Result<[u8; 32]> {
    Ok(Pubkey::from_str(s)?.to_bytes())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let payer =
        read_keypair_file(&cli.keypair).map_err(|e| anyhow!("read keypair {}: {}", cli.keypair.display(), e))?;
    let program_id = Pubkey::from_str(SUBSIDY_PROGRAM_ID)?;
    let rpc = RpcClient::new_with_commitment(cli.rpc.clone(), CommitmentConfig::confirmed());

    eprintln!("[subsidy-cli] program: {}", program_id);
    eprintln!("[subsidy-cli] payer:   {}", payer.pubkey());
    eprintln!("[subsidy-cli] rpc:     {}", cli.rpc);

    match cli.cmd {
        Cmd::Init {
            governance,
            bridge_program_id,
            productive_vault,
            productive_asset_id,
            treasury_total,
            federation,
        } => {
            let governance = match governance {
                Some(g) => Pubkey::from_str(&g)?,
                None => payer.pubkey(),
            };
            let fed: FederationFile = serde_json::from_slice(
                &std::fs::read(&federation).context("read federation file")?,
            )?;
            let federation_n = fed.pubkeys.len() as u8;
            let federation_m = fed.threshold;
            anyhow::ensure!(
                federation_m > 0 && federation_n >= federation_m,
                "federation: threshold {} > member count {}",
                federation_m,
                federation_n
            );
            let federation_members = fed
                .pubkeys
                .iter()
                .map(|p| pubkey_arr(p))
                .collect::<anyhow::Result<Vec<_>>>()?;

            let cfg_pda = pda(&[b"subsidy_config"], &program_id);
            let reg_pda = pda(&[b"validator_registry"], &program_id);
            let treasury_pda = pda(&[b"treasury"], &program_id);

            let treasury_total = match treasury_total {
                Some(t) => t,
                None => rpc.get_balance(&treasury_pda).unwrap_or(0),
            };
            eprintln!("[subsidy-cli] subsidy_config:    {}", cfg_pda);
            eprintln!("[subsidy-cli] validator_registry:{}", reg_pda);
            eprintln!("[subsidy-cli] treasury_pda:      {}", treasury_pda);
            eprintln!("[subsidy-cli] governance:        {}", governance);
            eprintln!("[subsidy-cli] bridge_program_id: {}", bridge_program_id);
            eprintln!("[subsidy-cli] treasury_total:    {} lamports", treasury_total);
            eprintln!(
                "[subsidy-cli] federation:        {}-of-{}",
                federation_m, federation_n
            );

            let args = InitSubsidyArgs {
                governance: governance.to_bytes(),
                bridge_program_id: pubkey_arr(&bridge_program_id)?,
                productive_vault: pubkey_arr(&productive_vault)?,
                productive_asset_id,
                treasury_total,
                federation_m,
                federation_n,
                federation_members,
            };
            let mut data = discriminator("init_subsidy").to_vec();
            args.serialize(&mut data)?;

            let ix = Instruction {
                program_id,
                accounts: vec![
                    AccountMeta::new(payer.pubkey(), true), // authority [signer, writable]
                    AccountMeta::new(cfg_pda, false),       // subsidy_config (init)
                    AccountMeta::new(reg_pda, false),       // validator_registry (init)
                    AccountMeta::new_readonly(system_program::ID, false),
                ],
                data,
            };

            let bh = rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);
            eprintln!("[subsidy-cli] sending init_subsidy tx…");
            let sig = rpc.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
            println!("subsidy_config:     {}", cfg_pda);
            println!("validator_registry: {}", reg_pda);
        }
        Cmd::RegisterValidator { validator } => {
            let validator_pk = Pubkey::from_str(&validator)?;
            let cfg_pda = pda(&[b"subsidy_config"], &program_id);
            let reg_pda = pda(&[b"validator_registry"], &program_id);
            let rec_pda = pda(&[b"validator", validator_pk.as_ref()], &program_id);
            eprintln!("[subsidy-cli] validator:          {}", validator_pk);
            eprintln!("[subsidy-cli] validator_record:   {}", rec_pda);

            let args = RegisterValidatorArgs {
                validator: validator_pk.to_bytes(),
            };
            let mut data = discriminator("register_validator").to_vec();
            args.serialize(&mut data)?;

            let ix = Instruction {
                program_id,
                accounts: vec![
                    AccountMeta::new(payer.pubkey(), true),    // authority [signer, writable]
                    AccountMeta::new_readonly(cfg_pda, false), // subsidy_config
                    AccountMeta::new(reg_pda, false),          // validator_registry
                    AccountMeta::new(rec_pda, false),          // validator_record (init)
                    AccountMeta::new_readonly(system_program::ID, false),
                ],
                data,
            };

            let bh = rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);
            eprintln!("[subsidy-cli] sending register_validator tx…");
            let sig = rpc.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
            println!("validator_record: {}", rec_pda);
        }
        Cmd::UnregisterValidator { validator } => {
            let validator_pk = Pubkey::from_str(&validator)?;
            let cfg_pda = pda(&[b"subsidy_config"], &program_id);
            let reg_pda = pda(&[b"validator_registry"], &program_id);
            let rec_pda = pda(&[b"validator", validator_pk.as_ref()], &program_id);
            eprintln!("[subsidy-cli] validator:          {}", validator_pk);
            eprintln!("[subsidy-cli] validator_record:   {}", rec_pda);

            let args = UnregisterValidatorArgs {
                validator: validator_pk.to_bytes(),
            };
            let mut data = discriminator("unregister_validator").to_vec();
            args.serialize(&mut data)?;

            let ix = Instruction {
                program_id,
                accounts: vec![
                    AccountMeta::new(payer.pubkey(), true), // authority (signer + rent recipient)
                    AccountMeta::new_readonly(cfg_pda, false), // subsidy_config
                    AccountMeta::new(reg_pda, false),       // validator_registry (mut for slot mutation)
                    AccountMeta::new(rec_pda, false),       // validator_record (mut + close)
                ],
                data,
            };

            let bh = rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);
            eprintln!("[subsidy-cli] sending unregister_validator tx…");
            let sig = rpc.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
            println!("closed validator_record: {}", rec_pda);
        }
        Cmd::SetBootstrapPerEpoch { target_per_epoch } => {
            let cfg_pda = pda(&[b"subsidy_config"], &program_id);
            eprintln!("[subsidy-cli] subsidy_config:    {}", cfg_pda);
            eprintln!("[subsidy-cli] target_per_epoch:  {} lamports", target_per_epoch);

            let args = SetBootstrapPerEpochArgs { target_per_epoch };
            let mut data = discriminator("set_bootstrap_per_epoch").to_vec();
            args.serialize(&mut data)?;

            let ix = Instruction {
                program_id,
                accounts: vec![
                    AccountMeta::new(payer.pubkey(), true), // authority (governance signer)
                    AccountMeta::new(cfg_pda, false),       // subsidy_config (mut)
                ],
                data,
            };
            let bh = rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);
            eprintln!("[subsidy-cli] sending set_bootstrap_per_epoch tx…");
            let sig = rpc.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
        }
        Cmd::AdminSetMetrics {
            validator,
            uptime_bps,
            delegated_stake,
            votes_cast,
        } => {
            let validator_pk = Pubkey::from_str(&validator)?;
            let cfg_pda = pda(&[b"subsidy_config"], &program_id);
            let reg_pda = pda(&[b"validator_registry"], &program_id);
            let rec_pda = pda(&[b"validator", validator_pk.as_ref()], &program_id);
            eprintln!("[subsidy-cli] validator:        {}", validator_pk);
            eprintln!("[subsidy-cli] validator_record: {}", rec_pda);

            let args = AdminSetMetricsArgs {
                validator: validator_pk.to_bytes(),
                uptime_bps,
                delegated_stake,
                votes_cast,
            };
            let mut data = discriminator("admin_set_validator_metrics").to_vec();
            args.serialize(&mut data)?;

            let ix = Instruction {
                program_id,
                accounts: vec![
                    AccountMeta::new(payer.pubkey(), true),
                    AccountMeta::new_readonly(cfg_pda, false),
                    AccountMeta::new_readonly(reg_pda, false),
                    AccountMeta::new(rec_pda, false),
                ],
                data,
            };

            let bh = rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);
            eprintln!("[subsidy-cli] sending admin_set_validator_metrics tx…");
            let sig = rpc.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
        }
        Cmd::BootstrapDistribute { epoch } => {
            let cfg_pda = pda(&[b"subsidy_config"], &program_id);
            let reg_pda = pda(&[b"validator_registry"], &program_id);
            let treasury_pda = pda(&[b"treasury"], &program_id);
            let accrual_pda = pda(
                &[b"accrual", &epoch.to_le_bytes()],
                &program_id,
            );
            eprintln!("[subsidy-cli] epoch:             {}", epoch);
            eprintln!("[subsidy-cli] epoch_accrual:     {}", accrual_pda);
            eprintln!("[subsidy-cli] treasury_pda:      {}", treasury_pda);

            // Read registry from chain to enumerate validators.
            let reg_acct = rpc.get_account(&reg_pda)?;
            let reg_bytes = reg_acct.data;
            // Layout: [8 disc] [4 count u32 LE] [32 * MAX_VALIDATORS]
            if reg_bytes.len() < 12 {
                return Err(anyhow!("registry account too small"));
            }
            let count =
                u32::from_le_bytes(reg_bytes[8..12].try_into().unwrap()) as usize;
            eprintln!("[subsidy-cli] validator count:   {}", count);
            let mut remaining = Vec::with_capacity(count * 2);
            for k in 0..count {
                let off = 12 + k * 32;
                if reg_bytes.len() < off + 32 {
                    return Err(anyhow!("registry truncated at validator {}", k));
                }
                let mut pkb = [0u8; 32];
                pkb.copy_from_slice(&reg_bytes[off..off + 32]);
                let validator_pk = Pubkey::new_from_array(pkb);
                let rec_pda = pda(
                    &[b"validator", validator_pk.as_ref()],
                    &program_id,
                );
                eprintln!(
                    "[subsidy-cli]   [{}] {} (record {})",
                    k, validator_pk, rec_pda
                );
                // Order in remaining_accounts must be: record, identity, record, identity, ...
                remaining.push(AccountMeta::new(rec_pda, false));
                remaining.push(AccountMeta::new(validator_pk, false));
            }

            let args = BootstrapDistributeArgs { epoch };
            let mut data = discriminator("bootstrap_distribute").to_vec();
            args.serialize(&mut data)?;

            let mut accounts = vec![
                AccountMeta::new(payer.pubkey(), true),                  // relayer
                AccountMeta::new(cfg_pda, false),                        // subsidy_config (mut)
                AccountMeta::new_readonly(reg_pda, false),               // validator_registry
                AccountMeta::new(accrual_pda, false),                    // epoch_accrual (init_if_needed)
                AccountMeta::new(treasury_pda, false),                   // treasury (mut)
                AccountMeta::new_readonly(system_program::ID, false),    // system_program
            ];
            accounts.extend(remaining);

            let ix = Instruction {
                program_id,
                accounts,
                data,
            };

            let bh = rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], bh);
            eprintln!("[subsidy-cli] sending bootstrap_distribute tx…");
            let sig = rpc.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
        }
        Cmd::MigrateTreasuryOwner {} => {
            // Genesis-baked owner of treasury (= ASCII "LAZY_CLAIM_PROGRAM_PLACEHOLDER11").
            let lazy_claim_program_id =
                Pubkey::from_str("68fnSf8CZjxLM2xHmswktgz3a77KLQT2nbhjWbpKWsYU")?;
            let treasury_pda = pda(&[b"treasury"], &program_id);
            let cur = rpc.get_account(&treasury_pda)?;
            eprintln!("[subsidy-cli] treasury_pda:      {}", treasury_pda);
            eprintln!("[subsidy-cli] balance:           {} lamports", cur.lamports);
            eprintln!("[subsidy-cli] current owner:     {}", cur.owner);
            eprintln!("[subsidy-cli] target owner:      {}", program_id);

            // Single ix: lazy-claim::AssignTreasuryOwner(new_owner = validator-subsidy).
            //   discriminator (0x05) + 32-byte new_owner pubkey
            // Lazy-claim owns treasury; treasury has zero data; therefore the owner
            // program is permitted by the runtime to call AccountInfo::assign() on
            // it directly. No drain/refund dance needed.
            let mut data = Vec::with_capacity(33);
            data.push(0x05u8);
            data.extend_from_slice(program_id.as_ref());
            let ix = Instruction {
                program_id: lazy_claim_program_id,
                accounts: vec![
                    AccountMeta::new(payer.pubkey(), true),    // authority signer
                    AccountMeta::new(treasury_pda, false),     // treasury (writable)
                ],
                data,
            };

            let bh = rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&payer.pubkey()),
                &[&payer],
                bh,
            );
            eprintln!("[subsidy-cli] sending assign_treasury_owner tx…");
            let sig = rpc.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
            println!("treasury {} now owned by {}", treasury_pda, program_id);
        }
        Cmd::DelegateTreasuryStake {
            vote_account,
            amount,
            stake_account,
        } => {
            if amount == 0 {
                return Err(anyhow!("--amount must be > 0"));
            }
            let vote_pk = Pubkey::from_str(&vote_account)?;
            let cfg_pda = pda(&[b"subsidy_config"], &program_id);
            let treasury_pda = pda(&[b"treasury"], &program_id);

            // Load (or create) the fresh stake-account keypair. Saving the file
            // up-front means a future `deactivate` / `withdraw` ix can be
            // signed without re-discovering the pubkey from chain logs.
            let stake_kp = if stake_account.exists() {
                read_keypair_file(&stake_account)
                    .map_err(|e| anyhow!("read stake-account keypair {}: {}", stake_account.display(), e))?
            } else {
                if let Some(parent) = stake_account.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent).with_context(|| {
                            format!("create dir for stake-account keypair {}", parent.display())
                        })?;
                    }
                }
                let kp = Keypair::new();
                write_keypair_file(&kp, &stake_account).map_err(|e| {
                    anyhow!("write stake-account keypair {}: {}", stake_account.display(), e)
                })?;
                eprintln!(
                    "[subsidy-cli] generated fresh stake-account keypair at {}",
                    stake_account.display()
                );
                kp
            };

            // Native stake program + sysvars. Anchor 1.x stripped the
            // stake_history sysvar re-export; hardcode all four canonical
            // addresses so we don't drag in agave-program crates.
            let stake_program_id =
                Pubkey::from_str("Stake11111111111111111111111111111111111111")?;
            let clock_sysvar = Pubkey::from_str("SysvarC1ock11111111111111111111111111111111")?;
            let rent_sysvar = Pubkey::from_str("SysvarRent111111111111111111111111111111111")?;
            let stake_history_sysvar =
                Pubkey::from_str("SysvarStakeHistory1111111111111111111111111")?;
            let stake_config = Pubkey::from_str("StakeConfig11111111111111111111111111111111")?;

            eprintln!("[subsidy-cli] vote_account:      {}", vote_pk);
            eprintln!("[subsidy-cli] stake_account:     {}", stake_kp.pubkey());
            eprintln!("[subsidy-cli] treasury_pda:      {}", treasury_pda);
            eprintln!("[subsidy-cli] amount (lamports): {}", amount);

            let args = DelegateTreasuryStakeArgs { amount };
            let mut data = discriminator("delegate_treasury_stake").to_vec();
            args.serialize(&mut data)?;

            // Account ordering must match `DelegateTreasuryStake` in
            // programs/validator-subsidy/src/instructions/delegate_treasury_stake.rs.
            let ix = Instruction {
                program_id,
                accounts: vec![
                    AccountMeta::new(payer.pubkey(), true),         // authority
                    AccountMeta::new_readonly(cfg_pda, false),      // subsidy_config
                    AccountMeta::new(treasury_pda, false),          // treasury (PDA, mut)
                    AccountMeta::new(stake_kp.pubkey(), true),      // stake_account (signer, mut)
                    AccountMeta::new_readonly(vote_pk, false),      // vote_account
                    AccountMeta::new_readonly(stake_program_id, false), // stake_program
                    AccountMeta::new_readonly(system_program::ID, false), // system_program
                    AccountMeta::new_readonly(clock_sysvar, false), // clock
                    AccountMeta::new_readonly(stake_history_sysvar, false), // stake_history
                    AccountMeta::new_readonly(stake_config, false), // stake_config
                    AccountMeta::new_readonly(rent_sysvar, false),  // rent
                ],
                data,
            };

            let bh = rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(
                &[ix],
                Some(&payer.pubkey()),
                &[&payer, &stake_kp],
                bh,
            );
            eprintln!("[subsidy-cli] sending delegate_treasury_stake tx…");
            let sig = rpc.send_and_confirm_transaction(&tx)?;
            println!("[done] {}", sig);
            println!("stake_account: {}", stake_kp.pubkey());
            println!("delegated:     {} lamports → {}", amount, vote_pk);
            println!("note: native warmup ≈ 1 epoch to fully active.");
        }
    }
    let _ = SYSVAR_INSTRUCTIONS_ID; // keep the unused-import lint quiet for future flows
    Ok(())
}

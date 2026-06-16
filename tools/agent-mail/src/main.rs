use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    signature::{read_keypair_file, Keypair, Signer},
    transaction::Transaction,
};
use staccana_agent_mail::{
    agent_record_pda, build_claim_ix, build_initialize_faucet_ix, build_register_agent_ix,
    build_unregister_agent_ix, decode_amount_rows, default_agent_faucet_program_id,
    default_token_2022_program_id, encode_amount_rows, faucet_config_pda, parse_key_hex,
    parse_pubkey, ClaimAccounts, InitializeFaucetAccounts, InitializeFaucetArgs,
    RegisterAgentAccounts, UnregisterAgentAccounts, AGENT_FAUCET_PROGRAM_ID,
    DEFAULT_CHANNEL_KEY_HEX, TOKEN_2022_PROGRAM_ID,
};
use staccana_agent_messaging::{estimate_packetized_chars, PACKET_PAYLOAD_CHARS};

#[derive(Parser)]
#[command(name = "agent-mail")]
#[command(about = "Staccana agent private-message amount codec and MSG faucet CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Encode text into confidential-transfer amount packets.
    Encode {
        #[arg(long)]
        text: String,
        #[arg(long, default_value_t = 0)]
        nonce: u64,
        #[arg(long, default_value = DEFAULT_CHANNEL_KEY_HEX)]
        key_hex: String,
    },
    /// Decode confidential-transfer amount packets back into text.
    Decode {
        #[arg(long, value_delimiter = ',')]
        amounts: Vec<u64>,
        #[arg(long, default_value_t = 0)]
        nonce: u64,
        #[arg(long, default_value = DEFAULT_CHANNEL_KEY_HEX)]
        key_hex: String,
    },
    /// Estimate packet and MegaTxn envelope size for a message length.
    Estimate {
        #[arg(long)]
        chars: usize,
    },
    /// Agent-only MSG carrier-token faucet instructions.
    Faucet {
        #[command(subcommand)]
        command: FaucetCommand,
    },
}

#[derive(Subcommand)]
enum FaucetCommand {
    /// Print the faucet config PDA and optional agent record PDA.
    Pdas {
        #[arg(long)]
        mint: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value = AGENT_FAUCET_PROGRAM_ID)]
        program_id: String,
    },
    /// Initialize a faucet config PDA for an MSG carrier mint.
    Init {
        #[arg(long)]
        mint: String,
        #[arg(long)]
        authority_keypair: PathBuf,
        #[arg(long)]
        payer_keypair: Option<PathBuf>,
        #[arg(long)]
        quota_per_epoch: u64,
        #[arg(long)]
        epoch_slots: u64,
        #[arg(long, default_value_t = 0)]
        start_slot: u64,
        #[arg(long, default_value = "https://rpc.mp.fun")]
        rpc: String,
        #[arg(long, default_value = AGENT_FAUCET_PROGRAM_ID)]
        program_id: String,
        /// Actually submit. Without this, the CLI only prints the instruction summary.
        #[arg(long)]
        send: bool,
    },
    /// Register an agent identity so it can claim MSG quota.
    Register {
        #[arg(long)]
        mint: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        authority_keypair: PathBuf,
        #[arg(long, default_value = "https://rpc.mp.fun")]
        rpc: String,
        #[arg(long, default_value = AGENT_FAUCET_PROGRAM_ID)]
        program_id: String,
        #[arg(long)]
        send: bool,
    },
    /// Disable an agent identity.
    Unregister {
        #[arg(long)]
        mint: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        authority_keypair: PathBuf,
        #[arg(long, default_value = "https://rpc.mp.fun")]
        rpc: String,
        #[arg(long, default_value = AGENT_FAUCET_PROGRAM_ID)]
        program_id: String,
        #[arg(long)]
        send: bool,
    },
    /// Claim MSG carrier units into an agent-owned Token-22 account.
    Claim {
        #[arg(long)]
        mint: String,
        #[arg(long)]
        agent_keypair: PathBuf,
        #[arg(long)]
        agent_token_account: String,
        #[arg(long)]
        amount: u64,
        #[arg(long, default_value = "https://rpc.mp.fun")]
        rpc: String,
        #[arg(long, default_value = AGENT_FAUCET_PROGRAM_ID)]
        program_id: String,
        #[arg(long, default_value = TOKEN_2022_PROGRAM_ID)]
        token_program: String,
        #[arg(long)]
        send: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Encode {
            text,
            nonce,
            key_hex,
        } => {
            let key = parse_key_hex(&key_hex)?;
            for row in encode_amount_rows(&text, &key, nonce)? {
                println!(
                    "{}\t{}\t{}\t{}",
                    row.sequence, row.final_packet, row.amount, row.chunk
                );
            }
        }
        Command::Decode {
            amounts,
            nonce,
            key_hex,
        } => {
            let key = parse_key_hex(&key_hex)?;
            println!("{}", decode_amount_rows(&amounts, &key, nonce)?);
        }
        Command::Estimate { chars } => {
            let estimate = estimate_packetized_chars(chars, PACKET_PAYLOAD_CHARS);
            println!("packets={}", estimate.packets);
            println!("wrapped_message_bytes={}", estimate.wrapped_message_bytes);
            println!("account_locks={}", estimate.account_locks);
            println!("fits_by_bytes={}", estimate.fits_by_bytes);
            println!("fits_by_account_locks={}", estimate.fits_by_account_locks);
        }
        Command::Faucet { command } => run_faucet(command)?,
    }
    Ok(())
}

fn run_faucet(command: FaucetCommand) -> Result<()> {
    match command {
        FaucetCommand::Pdas {
            mint,
            agent,
            program_id,
        } => {
            let program_id = parse_pubkey(&program_id, "program-id")?;
            let mint = parse_pubkey(&mint, "mint")?;
            let (config, config_bump) = faucet_config_pda(&program_id, &mint);
            println!("program_id={program_id}");
            println!("mint={mint}");
            println!("config={config}");
            println!("config_bump={config_bump}");
            if let Some(agent) = agent {
                let agent = parse_pubkey(&agent, "agent")?;
                let (record, record_bump) = agent_record_pda(&program_id, &config, &agent);
                println!("agent={agent}");
                println!("agent_record={record}");
                println!("agent_record_bump={record_bump}");
            }
        }
        FaucetCommand::Init {
            mint,
            authority_keypair,
            payer_keypair,
            quota_per_epoch,
            epoch_slots,
            start_slot,
            rpc,
            program_id,
            send,
        } => {
            let program_id = parse_pubkey(&program_id, "program-id")?;
            let mint = parse_pubkey(&mint, "mint")?;
            let authority = load_keypair(&authority_keypair)?;
            let payer_holder = match payer_keypair {
                Some(path) => Some(load_keypair(&path)?),
                None => None,
            };
            let payer = payer_holder.as_ref().unwrap_or(&authority);
            let (config, _) = faucet_config_pda(&program_id, &mint);
            let ix = build_initialize_faucet_ix(
                program_id,
                InitializeFaucetAccounts {
                    config,
                    mint,
                    authority: authority.pubkey(),
                    payer: payer.pubkey(),
                },
                InitializeFaucetArgs {
                    quota_per_epoch,
                    epoch_slots,
                    start_slot,
                },
            );
            println!(
                "init faucet: mint={mint} config={config} authority={} payer={} quota_per_epoch={quota_per_epoch} epoch_slots={epoch_slots} start_slot={start_slot}",
                authority.pubkey(),
                payer.pubkey()
            );
            let extra = if authority.pubkey() == payer.pubkey() {
                vec![]
            } else {
                vec![&authority]
            };
            submit_or_print(&rpc, payer, &extra, ix, send)?;
        }
        FaucetCommand::Register {
            mint,
            agent,
            authority_keypair,
            rpc,
            program_id,
            send,
        } => {
            let program_id = parse_pubkey(&program_id, "program-id")?;
            let mint = parse_pubkey(&mint, "mint")?;
            let agent = parse_pubkey(&agent, "agent")?;
            let authority = load_keypair(&authority_keypair)?;
            let (config, _) = faucet_config_pda(&program_id, &mint);
            let (agent_record, _) = agent_record_pda(&program_id, &config, &agent);
            let ix = build_register_agent_ix(
                program_id,
                RegisterAgentAccounts {
                    config,
                    authority: authority.pubkey(),
                    agent_record,
                },
                agent,
            );
            println!(
                "register agent: mint={mint} config={config} agent={agent} agent_record={agent_record} authority={}",
                authority.pubkey()
            );
            submit_or_print(&rpc, &authority, &[], ix, send)?;
        }
        FaucetCommand::Unregister {
            mint,
            agent,
            authority_keypair,
            rpc,
            program_id,
            send,
        } => {
            let program_id = parse_pubkey(&program_id, "program-id")?;
            let mint = parse_pubkey(&mint, "mint")?;
            let agent = parse_pubkey(&agent, "agent")?;
            let authority = load_keypair(&authority_keypair)?;
            let (config, _) = faucet_config_pda(&program_id, &mint);
            let (agent_record, _) = agent_record_pda(&program_id, &config, &agent);
            let ix = build_unregister_agent_ix(
                program_id,
                UnregisterAgentAccounts {
                    config,
                    authority: authority.pubkey(),
                    agent_record,
                },
            );
            println!(
                "unregister agent: mint={mint} config={config} agent={agent} agent_record={agent_record} authority={}",
                authority.pubkey()
            );
            submit_or_print(&rpc, &authority, &[], ix, send)?;
        }
        FaucetCommand::Claim {
            mint,
            agent_keypair,
            agent_token_account,
            amount,
            rpc,
            program_id,
            token_program,
            send,
        } => {
            let program_id = parse_pubkey(&program_id, "program-id")?;
            let mint = parse_pubkey(&mint, "mint")?;
            let agent_token_account = parse_pubkey(&agent_token_account, "agent-token-account")?;
            let token_program = parse_pubkey(&token_program, "token-program")?;
            let agent = load_keypair(&agent_keypair)?;
            let (config, _) = faucet_config_pda(&program_id, &mint);
            let (agent_record, _) = agent_record_pda(&program_id, &config, &agent.pubkey());
            let ix = build_claim_ix(
                program_id,
                ClaimAccounts {
                    config,
                    agent_record,
                    mint,
                    agent_token_account,
                    agent: agent.pubkey(),
                    token_program,
                },
                amount,
            );
            println!(
                "claim MSG: mint={mint} config={config} agent={} agent_record={agent_record} token_account={agent_token_account} amount={amount}",
                agent.pubkey()
            );
            submit_or_print(&rpc, &agent, &[], ix, send)?;
        }
    }
    Ok(())
}

fn load_keypair(path: &PathBuf) -> Result<Keypair> {
    read_keypair_file(path).map_err(|e| anyhow!("failed to load keypair {}: {e}", path.display()))
}

fn submit_or_print(
    rpc_url: &str,
    payer: &Keypair,
    extra_signers: &[&Keypair],
    ix: Instruction,
    send: bool,
) -> Result<()> {
    print_instruction_summary(&ix);
    if !send {
        println!("not_submitted=true");
        println!("pass --send to simulate and submit");
        return Ok(());
    }

    let rpc = RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
    let blockhash = rpc
        .get_latest_blockhash()
        .with_context(|| format!("failed to fetch latest blockhash from {rpc_url}"))?;

    let mut signers: Vec<&dyn Signer> = vec![payer];
    for signer in extra_signers {
        if signer.pubkey() != payer.pubkey() {
            signers.push(*signer);
        }
    }

    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &signers, blockhash);
    let sim = rpc
        .simulate_transaction(&tx)
        .context("transaction simulation failed at RPC layer")?;
    println!("simulation_error={:?}", sim.value.err);
    if let Some(logs) = sim.value.logs {
        for log in logs {
            println!("sim_log={log}");
        }
    }
    if sim.value.err.is_some() {
        bail!("simulation failed; transaction not submitted");
    }

    let sig = rpc
        .send_and_confirm_transaction(&tx)
        .context("failed to submit transaction")?;
    println!("signature={sig}");
    Ok(())
}

fn print_instruction_summary(ix: &Instruction) {
    println!("program_id={}", ix.program_id);
    println!("accounts={}", ix.accounts.len());
    for (i, meta) in ix.accounts.iter().enumerate() {
        println!(
            "account[{i}]={} signer={} writable={}",
            meta.pubkey, meta.is_signer, meta.is_writable
        );
    }
    println!("data_len={}", ix.data.len());
    println!("data_hex={}", hex(&ix.data));
}

fn hex(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(LUT[(byte >> 4) as usize] as char);
        out.push(LUT[(byte & 0x0f) as usize] as char);
    }
    out
}

#[allow(dead_code)]
fn _defaults_compile() {
    let _ = default_agent_faucet_program_id();
    let _ = default_token_2022_program_id();
}

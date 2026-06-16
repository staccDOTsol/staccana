use std::str::FromStr;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
use solana_system_interface::program as system_program;
use staccana_agent_messaging::{
    amount_to_packet, decode_packets, encode_text_packets, packet_to_amount,
};

pub const DEFAULT_CHANNEL_KEY_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
pub const AGENT_FAUCET_PROGRAM_ID: &str = "5oBGxGcvcSzpPDdk6grLh7QrC82vjAAEdE2RPkiXmJx2";
pub const TOKEN_2022_PROGRAM_ID: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
pub const FAUCET_CONFIG_SEED: &[u8] = b"agent_faucet";
pub const AGENT_RECORD_SEED: &[u8] = b"agent";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedPacketRow {
    pub sequence: u8,
    pub final_packet: bool,
    pub amount: u64,
    pub chunk: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitializeFaucetArgs {
    pub quota_per_epoch: u64,
    pub epoch_slots: u64,
    pub start_slot: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitializeFaucetAccounts {
    pub config: Pubkey,
    pub mint: Pubkey,
    pub authority: Pubkey,
    pub payer: Pubkey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisterAgentAccounts {
    pub config: Pubkey,
    pub authority: Pubkey,
    pub agent_record: Pubkey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnregisterAgentAccounts {
    pub config: Pubkey,
    pub authority: Pubkey,
    pub agent_record: Pubkey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaimAccounts {
    pub config: Pubkey,
    pub agent_record: Pubkey,
    pub mint: Pubkey,
    pub agent_token_account: Pubkey,
    pub agent: Pubkey,
    pub token_program: Pubkey,
}

pub fn parse_key_hex(hex: &str) -> Result<[u8; 32]> {
    if hex.len() != 64 {
        bail!("--key-hex must be 64 hex chars");
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let start = i * 2;
        *byte = u8::from_str_radix(&hex[start..start + 2], 16)
            .with_context(|| format!("invalid hex byte at offset {start}"))?;
    }
    Ok(out)
}

pub fn parse_pubkey(value: &str, flag: &str) -> Result<Pubkey> {
    Pubkey::from_str(value).with_context(|| format!("--{flag} is not a valid base58 pubkey"))
}

pub fn default_agent_faucet_program_id() -> Pubkey {
    parse_pubkey(AGENT_FAUCET_PROGRAM_ID, "program-id").expect("constant program id is valid")
}

pub fn default_token_2022_program_id() -> Pubkey {
    parse_pubkey(TOKEN_2022_PROGRAM_ID, "token-program").expect("constant token id is valid")
}

pub fn encode_amount_rows(text: &str, key: &[u8; 32], nonce: u64) -> Result<Vec<EncodedPacketRow>> {
    let packets = encode_text_packets(text)?;
    packets
        .into_iter()
        .map(|packet| {
            let amount = packet_to_amount(&packet, key, nonce + packet.sequence as u64)?;
            Ok(EncodedPacketRow {
                sequence: packet.sequence,
                final_packet: packet.final_packet,
                amount,
                chunk: printable_chunk(&packet.chunk),
            })
        })
        .collect()
}

pub fn decode_amount_rows(amounts: &[u64], key: &[u8; 32], nonce: u64) -> Result<String> {
    let mut packets = Vec::with_capacity(amounts.len());
    for (i, amount) in amounts.iter().copied().enumerate() {
        packets.push(amount_to_packet(amount, key, nonce + i as u64)?);
    }
    Ok(decode_packets(&packets)?)
}

pub fn printable_chunk(chunk: &str) -> String {
    chunk.replace('\n', "\\n")
}

pub fn faucet_config_pda(program_id: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[FAUCET_CONFIG_SEED, mint.as_ref()], program_id)
}

pub fn agent_record_pda(program_id: &Pubkey, config: &Pubkey, agent: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[AGENT_RECORD_SEED, config.as_ref(), agent.as_ref()],
        program_id,
    )
}

pub fn build_initialize_faucet_ix(
    program_id: Pubkey,
    accounts: InitializeFaucetAccounts,
    args: InitializeFaucetArgs,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(accounts.config, false),
            AccountMeta::new_readonly(accounts.mint, false),
            AccountMeta::new_readonly(accounts.authority, true),
            AccountMeta::new(accounts.payer, true),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data: initialize_faucet_data(args),
    }
}

pub fn build_register_agent_ix(
    program_id: Pubkey,
    accounts: RegisterAgentAccounts,
    agent: Pubkey,
) -> Instruction {
    let mut data = anchor_instruction_data("register_agent");
    data.extend_from_slice(agent.as_ref());
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(accounts.config, false),
            AccountMeta::new(accounts.authority, true),
            AccountMeta::new(accounts.agent_record, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}

pub fn build_unregister_agent_ix(
    program_id: Pubkey,
    accounts: UnregisterAgentAccounts,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(accounts.config, false),
            AccountMeta::new_readonly(accounts.authority, true),
            AccountMeta::new(accounts.agent_record, false),
        ],
        data: anchor_instruction_data("unregister_agent"),
    }
}

pub fn build_claim_ix(program_id: Pubkey, accounts: ClaimAccounts, amount: u64) -> Instruction {
    let mut data = anchor_instruction_data("claim");
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(accounts.config, false),
            AccountMeta::new(accounts.agent_record, false),
            AccountMeta::new(accounts.mint, false),
            AccountMeta::new(accounts.agent_token_account, false),
            AccountMeta::new_readonly(accounts.agent, true),
            AccountMeta::new_readonly(accounts.token_program, false),
        ],
        data,
    }
}

pub fn initialize_faucet_data(args: InitializeFaucetArgs) -> Vec<u8> {
    let mut data = anchor_instruction_data("initialize");
    data.extend_from_slice(&args.quota_per_epoch.to_le_bytes());
    data.extend_from_slice(&args.epoch_slots.to_le_bytes());
    data.extend_from_slice(&args.start_slot.to_le_bytes());
    data
}

pub fn anchor_instruction_data(name: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"global:");
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    digest[..8].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use staccana_agent_messaging::PACKET_PAYLOAD_CHARS;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn encode_decode_rows_roundtrip() {
        let key = [7u8; 32];
        let text = "hello agent\n";
        let rows = encode_amount_rows(text, &key, 99).unwrap();
        assert_eq!(rows.len(), text.len().div_ceil(PACKET_PAYLOAD_CHARS));
        let amounts: Vec<u64> = rows.iter().map(|row| row.amount).collect();
        assert_eq!(decode_amount_rows(&amounts, &key, 99).unwrap(), text);
    }

    #[test]
    fn faucet_pdas_are_stable_for_program_mint_agent_tuple() {
        let program_id = default_agent_faucet_program_id();
        let mint = pk(2);
        let agent = pk(3);
        let (config, config_bump) = faucet_config_pda(&program_id, &mint);
        let (record, record_bump) = agent_record_pda(&program_id, &config, &agent);
        assert_eq!(faucet_config_pda(&program_id, &mint), (config, config_bump));
        assert_eq!(
            agent_record_pda(&program_id, &config, &agent),
            (record, record_bump)
        );
        assert_ne!(config, record);
    }

    #[test]
    fn initialize_faucet_instruction_is_anchor_compatible_shape() {
        let program_id = default_agent_faucet_program_id();
        let accounts = InitializeFaucetAccounts {
            config: pk(1),
            mint: pk(2),
            authority: pk(3),
            payer: pk(4),
        };
        let ix = build_initialize_faucet_ix(
            program_id,
            accounts,
            InitializeFaucetArgs {
                quota_per_epoch: 10,
                epoch_slots: 20,
                start_slot: 30,
            },
        );
        assert_eq!(ix.program_id, program_id);
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(
            &ix.data[..8],
            anchor_instruction_data("initialize").as_slice()
        );
        assert_eq!(&ix.data[8..16], &10u64.to_le_bytes());
        assert_eq!(&ix.data[16..24], &20u64.to_le_bytes());
        assert_eq!(&ix.data[24..32], &30u64.to_le_bytes());
    }

    #[test]
    fn claim_instruction_uses_agent_signer_and_token_program() {
        let program_id = default_agent_faucet_program_id();
        let token_program = default_token_2022_program_id();
        let ix = build_claim_ix(
            program_id,
            ClaimAccounts {
                config: pk(1),
                agent_record: pk(2),
                mint: pk(3),
                agent_token_account: pk(4),
                agent: pk(5),
                token_program,
            },
            123,
        );
        assert_eq!(ix.accounts.len(), 6);
        assert!(ix.accounts[4].is_signer);
        assert_eq!(ix.accounts[5].pubkey, token_program);
        assert_eq!(&ix.data[..8], anchor_instruction_data("claim").as_slice());
        assert_eq!(&ix.data[8..16], &123u64.to_le_bytes());
    }
}

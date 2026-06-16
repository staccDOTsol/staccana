use solana_sdk::pubkey::Pubkey;
use staccana_agent_faucet::state::{apply_quota_claim, QuotaMathError};
use staccana_agent_mail::{
    agent_record_pda, build_claim_ix, build_initialize_faucet_ix, build_register_agent_ix,
    decode_amount_rows, default_agent_faucet_program_id, default_token_2022_program_id,
    encode_amount_rows, faucet_config_pda, ClaimAccounts, InitializeFaucetAccounts,
    InitializeFaucetArgs, RegisterAgentAccounts,
};

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

#[derive(Clone, Debug)]
struct CommunityAgent {
    pubkey: Pubkey,
    token_account: Pubkey,
    channel_key: [u8; 32],
    text: &'static str,
}

#[derive(Clone, Debug)]
struct EncodedCommunityMessage {
    nonce: u64,
    amounts: Vec<u64>,
    quota: u64,
}

fn encode_with_fitting_quota(agent: &CommunityAgent) -> EncodedCommunityMessage {
    (0u64..)
        .find_map(|nonce| {
            let rows = encode_amount_rows(agent.text, &agent.channel_key, nonce).ok()?;
            let quota = rows
                .iter()
                .try_fold(0u64, |acc, row| acc.checked_add(row.amount))?;
            let amounts: Vec<u64> = rows.iter().map(|row| row.amount).collect();
            Some(EncodedCommunityMessage {
                nonce,
                amounts,
                quota,
            })
        })
        .expect("eventually finds a nonce whose packet amount sum fits u64")
}

#[test]
fn agent_mail_e2e_community_faucet_quota_and_decode() {
    let community = vec![
        CommunityAgent {
            pubkey: pk(0x10),
            token_account: pk(0x80),
            channel_key: [0x10; 32],
            text: "agent alpha online",
        },
        CommunityAgent {
            pubkey: pk(0x11),
            token_account: pk(0x81),
            channel_key: [0x11; 32],
            text: "beta needs price",
        },
        CommunityAgent {
            pubkey: pk(0x12),
            token_account: pk(0x82),
            channel_key: [0x12; 32],
            text: "gamma has route",
        },
        CommunityAgent {
            pubkey: pk(0x13),
            token_account: pk(0x83),
            channel_key: [0x13; 32],
            text: "delta signs job",
        },
        CommunityAgent {
            pubkey: pk(0x14),
            token_account: pk(0x84),
            channel_key: [0x14; 32],
            text: "epsilon pays lp",
        },
        CommunityAgent {
            pubkey: pk(0x15),
            token_account: pk(0x85),
            channel_key: [0x15; 32],
            text: "zeta posts result",
        },
    ];

    let program_id = default_agent_faucet_program_id();
    let token_program = default_token_2022_program_id();
    let mint = pk(2);
    let authority = pk(3);
    let payer = pk(4);
    let (config, _) = faucet_config_pda(&program_id, &mint);

    let encoded: Vec<_> = community.iter().map(encode_with_fitting_quota).collect();
    let per_agent_quota = encoded
        .iter()
        .map(|msg| msg.quota)
        .max()
        .expect("non-empty community");

    let init_ix = build_initialize_faucet_ix(
        program_id,
        InitializeFaucetAccounts {
            config,
            mint,
            authority,
            payer,
        },
        InitializeFaucetArgs {
            quota_per_epoch: per_agent_quota,
            epoch_slots: 432_000,
            start_slot: 0,
        },
    );
    assert_eq!(init_ix.program_id, program_id);
    assert_eq!(init_ix.accounts.len(), 5);

    let mut total_packets = 0usize;
    for (agent, message) in community.iter().zip(encoded.iter()) {
        assert!(
            message.amounts.len() > 1,
            "each fixture should exercise multi-packet assembly"
        );
        total_packets += message.amounts.len();
        assert_eq!(
            decode_amount_rows(&message.amounts, &agent.channel_key, message.nonce).unwrap(),
            agent.text
        );

        let (agent_record, _) = agent_record_pda(&program_id, &config, &agent.pubkey);
        let register_ix = build_register_agent_ix(
            program_id,
            RegisterAgentAccounts {
                config,
                authority,
                agent_record,
            },
            agent.pubkey,
        );
        assert_eq!(register_ix.program_id, program_id);
        assert_eq!(register_ix.accounts[1].pubkey, authority);

        let claim_ixs: Vec<_> = message
            .amounts
            .iter()
            .copied()
            .map(|amount| {
                build_claim_ix(
                    program_id,
                    ClaimAccounts {
                        config,
                        agent_record,
                        mint,
                        agent_token_account: agent.token_account,
                        agent: agent.pubkey,
                        token_program,
                    },
                    amount,
                )
            })
            .collect();
        assert_eq!(claim_ixs.len(), message.amounts.len());
        assert!(claim_ixs.iter().all(|ix| ix.program_id == program_id));
        assert!(claim_ixs.iter().all(|ix| ix.accounts[4].is_signer));

        let mut last_epoch = 0;
        let mut claimed = 0;
        for amount in &message.amounts {
            apply_quota_claim(&mut last_epoch, &mut claimed, 0, message.quota, *amount).unwrap();
        }
        assert_eq!(claimed, message.quota);
        assert_eq!(
            apply_quota_claim(&mut last_epoch, &mut claimed, 0, message.quota, 1),
            Err(QuotaMathError::ClaimExceedsQuota)
        );
    }

    assert!(total_packets >= community.len() * 2);
}

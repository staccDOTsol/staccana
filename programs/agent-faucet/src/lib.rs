//! Agent-only faucet for the `MSG` carrier mint.
//!
//! The faucet is intentionally boring: governance registers agent identities, and each
//! registered agent can claim a fixed per-epoch quota of a cheap Token-22 carrier mint.
//! The faucet PDA must be configured as the mint authority for that mint.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, MintTo, TokenAccount, TokenInterface};

pub mod error;
pub mod state;

pub use error::AgentFaucetError;
pub use state::*;

declare_id!("5oBGxGcvcSzpPDdk6grLh7QrC82vjAAEdE2RPkiXmJx2");

#[program]
pub mod staccana_agent_faucet {
    use super::*;

    pub fn initialize(ctx: Context<InitializeFaucet>, args: InitializeFaucetArgs) -> Result<()> {
        require!(args.epoch_slots > 0, AgentFaucetError::ZeroEpochSlots);
        require!(args.quota_per_epoch > 0, AgentFaucetError::ZeroQuota);

        let config = &mut ctx.accounts.config;
        config.authority = ctx.accounts.authority.key();
        config.mint = ctx.accounts.mint.key();
        config.quota_per_epoch = args.quota_per_epoch;
        config.epoch_slots = args.epoch_slots;
        config.start_slot = args.start_slot;
        config.bump = ctx.bumps.config;

        emit!(FaucetInitializedEvent {
            authority: config.authority,
            mint: config.mint,
            quota_per_epoch: config.quota_per_epoch,
            epoch_slots: config.epoch_slots,
            start_slot: config.start_slot,
        });

        Ok(())
    }

    pub fn register_agent(ctx: Context<RegisterAgent>, agent: Pubkey) -> Result<()> {
        let record = &mut ctx.accounts.agent_record;
        record.agent = agent;
        record.active = true;
        record.last_claim_epoch = 0;
        record.claimed_in_epoch = 0;
        record.bump = ctx.bumps.agent_record;

        emit!(AgentRegisteredEvent {
            faucet: ctx.accounts.config.key(),
            agent,
        });

        Ok(())
    }

    pub fn unregister_agent(ctx: Context<UnregisterAgent>) -> Result<()> {
        let record = &mut ctx.accounts.agent_record;
        record.active = false;

        emit!(AgentUnregisteredEvent {
            faucet: ctx.accounts.config.key(),
            agent: record.agent,
        });

        Ok(())
    }

    pub fn claim(ctx: Context<ClaimMsg>, amount: u64) -> Result<()> {
        require!(amount > 0, AgentFaucetError::ZeroClaim);
        require!(
            ctx.accounts.agent_record.active,
            AgentFaucetError::AgentInactive
        );

        let clock = Clock::get()?;
        let epoch = current_faucet_epoch(
            clock.slot,
            ctx.accounts.config.start_slot,
            ctx.accounts.config.epoch_slots,
        )
        .map_err(|_| AgentFaucetError::ArithmeticOverflow)?;

        let agent_record = &mut ctx.accounts.agent_record;
        let mut last_claim_epoch = agent_record.last_claim_epoch;
        let mut claimed_in_epoch = agent_record.claimed_in_epoch;
        apply_quota_claim(
            &mut last_claim_epoch,
            &mut claimed_in_epoch,
            epoch,
            ctx.accounts.config.quota_per_epoch,
            amount,
        )
        .map_err(AgentFaucetError::from)?;
        agent_record.last_claim_epoch = last_claim_epoch;
        agent_record.claimed_in_epoch = claimed_in_epoch;

        let mint_key = ctx.accounts.config.mint;
        let bump = [ctx.accounts.config.bump];
        let signer_seeds: &[&[&[u8]]] = &[&[FaucetConfig::SEED, mint_key.as_ref(), &bump]];

        token_interface::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                MintTo {
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.agent_token_account.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
                signer_seeds,
            ),
            amount,
        )?;

        emit!(AgentClaimedEvent {
            faucet: ctx.accounts.config.key(),
            agent: ctx.accounts.agent.key(),
            epoch,
            amount,
            claimed_in_epoch: agent_record.claimed_in_epoch,
        });

        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitializeFaucetArgs {
    pub quota_per_epoch: u64,
    pub epoch_slots: u64,
    pub start_slot: u64,
}

#[derive(Accounts)]
pub struct InitializeFaucet<'info> {
    #[account(
        init,
        payer = payer,
        space = FaucetConfig::SPACE,
        seeds = [FaucetConfig::SEED, mint.key().as_ref()],
        bump
    )]
    pub config: Account<'info, FaucetConfig>,
    pub mint: InterfaceAccount<'info, Mint>,
    pub authority: Signer<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(agent: Pubkey)]
pub struct RegisterAgent<'info> {
    #[account(mut, has_one = authority)]
    pub config: Account<'info, FaucetConfig>,
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init_if_needed,
        payer = authority,
        space = AgentRecord::SPACE,
        seeds = [AgentRecord::SEED, config.key().as_ref(), agent.as_ref()],
        bump
    )]
    pub agent_record: Account<'info, AgentRecord>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UnregisterAgent<'info> {
    #[account(has_one = authority)]
    pub config: Account<'info, FaucetConfig>,
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [AgentRecord::SEED, config.key().as_ref(), agent_record.agent.as_ref()],
        bump = agent_record.bump
    )]
    pub agent_record: Account<'info, AgentRecord>,
}

#[derive(Accounts)]
pub struct ClaimMsg<'info> {
    #[account(
        seeds = [FaucetConfig::SEED, mint.key().as_ref()],
        bump = config.bump,
        has_one = mint
    )]
    pub config: Account<'info, FaucetConfig>,
    #[account(
        mut,
        seeds = [AgentRecord::SEED, config.key().as_ref(), agent.key().as_ref()],
        bump = agent_record.bump,
        constraint = agent_record.agent == agent.key() @ AgentFaucetError::BadAgentRecord
    )]
    pub agent_record: Account<'info, AgentRecord>,
    #[account(mut)]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        mut,
        constraint = agent_token_account.mint == mint.key() @ AgentFaucetError::BadTokenAccount,
        constraint = agent_token_account.owner == agent.key() @ AgentFaucetError::BadTokenAccount
    )]
    pub agent_token_account: InterfaceAccount<'info, TokenAccount>,
    pub agent: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

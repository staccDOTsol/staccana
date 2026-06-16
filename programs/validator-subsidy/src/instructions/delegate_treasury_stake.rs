//! `delegate_treasury_stake` — governance delegates native Solana stake to
//! a validator's vote account.
//!
//! Why this exists
//! ---------------
//!
//! At genesis, only the bootstrap-4 validators (val-1/2/3/4) had native stake
//! delegated. Every external validator joining via `docker run` gets a fresh
//! identity + vote keypair with **0 stake** → can't vote in the leader
//! rotation → can't earn vote credits → `update_validator_metrics`'s
//! `votes_cast` field never increments → their `distribute_yield` weight is
//! always 0 → they earn nothing from the validator-subsidy program even
//! after `register_validator` lands.
//!
//! This ix closes the loop: governance creates a fresh `StakeAccount` from
//! its OWN signer lamports (not treasury — the genesis-baked treasury PDA
//! at `["treasury"]` of THIS program is owned by the lazy-claim placeholder
//! pubkey at slot 0, blocking direct debits from the validator-subsidy
//! program; migrating that ownership is tracked separately). The stake
//! account is initialized with the treasury PDA as staker + withdrawer
//! authority so a future `undelegate` ix (not in this commit) can deactivate
//! and withdraw back to treasury for misbehaving validators or governance
//! rebalancing. The lamports we put in here are effectively a one-way
//! transfer FROM governance INTO treasury custody (gated by the treasury
//! PDA seeds) for the lifetime of the stake position.
//!
//! Native Solana stake activation handles the "drip" via warmup (≈ 1 epoch
//! to fully active). To drip MORE gradually, governance just makes
//! multiple smaller calls — each new stake account warms up
//! independently and the active stakes aggregate at the vote-account
//! level.
//!
//! Authorization: signer must equal `subsidy_config.governance` (same gate
//! as `register_validator` / `stake_to_productive`).

use crate::error::SubsidyError;
use crate::state::SubsidyConfig;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;

/// Native Solana Stake program. Builtin, fixed address; kept as a
/// `pubkey!`-style const to dodge a runtime PDA derivation.
const STAKE_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("Stake11111111111111111111111111111111111111");

/// `StakeInstruction::Initialize` discriminator (variant 0). The stake program
/// uses bincode-style enum encoding — one u32 LE for the variant, then the
/// borsh-style payload.
const STAKE_IX_INITIALIZE: u32 = 0;
/// `StakeInstruction::DelegateStake` discriminator (variant 2).
const STAKE_IX_DELEGATE: u32 = 2;

/// Bytes of a `StakeStateV2` account when zeroed on init. Native stake's
/// `Initialize` ix expects the account to have at least this much space
/// (200 bytes per `solana_stake_program::stake_state::StakeStateV2::size_of()`).
const STAKE_ACCOUNT_SPACE: u64 = 200;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct DelegateTreasuryStakeArgs {
    /// Lamports to delegate. Activates over native stake warmup (~1 epoch).
    /// Caller is responsible for picking values consistent with their
    /// drip schedule — the program imposes no per-call cap.
    pub amount: u64,
}

#[derive(Accounts)]
pub struct DelegateTreasuryStake<'info> {
    /// Governance signer. Must equal `subsidy_config.governance`.
    #[account(
        mut,
        constraint = authority.key() == subsidy_config.governance
            @ SubsidyError::BadInstructionData,
    )]
    pub authority: Signer<'info>,

    #[account(
        seeds = [b"subsidy_config"],
        bump = subsidy_config.bump,
    )]
    pub subsidy_config: Account<'info, SubsidyConfig>,

    /// Treasury PDA — staker + withdrawer authority for the new stake account.
    /// NOT a lamport source in this ix (see module docstring; treasury custody
    /// owner-field is broken by genesis-bake). Validated readonly via PDA
    /// derivation only so `Initialize` writes the right authority pubkey.
    /// CHECK: PDA derivation enforced by Anchor.
    #[account(
        seeds = [b"treasury"],
        bump,
    )]
    pub treasury: AccountInfo<'info>,

    /// Fresh stake account to allocate + delegate. Caller picks the
    /// keypair (off-chain) and partial-signs the tx with it; on-chain we
    /// CPI `system_program::create_account` to allocate + assign to the
    /// stake program.
    /// CHECK: native stake program checks layout post-init.
    #[account(mut)]
    pub stake_account: Signer<'info>,

    /// Vote account of the target validator. Native stake program checks
    /// it's owned by the vote program + has a valid `VoteState`.
    /// CHECK: native stake program validates.
    pub vote_account: AccountInfo<'info>,

    /// Native stake program. Address-pinned; can't be substituted.
    /// CHECK: address constraint enforces identity.
    #[account(address = STAKE_PROGRAM_ID)]
    pub stake_program: AccountInfo<'info>,

    pub system_program: Program<'info, System>,

    /// `Clock` sysvar — required by native `Initialize` + `DelegateStake`.
    pub clock: Sysvar<'info, Clock>,

    /// `StakeHistory` sysvar — required by `DelegateStake` for the
    /// activation-credit calculation. Anchor 1.x dropped the
    /// `solana_program::sysvar::stake_history` re-export so we hardcode
    /// the canonical address here (matches the SystemVar id in
    /// `solana_sdk_ids::sysvar::stake_history::ID`).
    /// CHECK: address-pinned to the standard sysvar id.
    #[account(address = Pubkey::from_str_const("SysvarStakeHistory1111111111111111111111111"))]
    pub stake_history: AccountInfo<'info>,

    /// `StakeConfig` sysvar — required by `DelegateStake` for the warmup
    /// rate. Address-pinned.
    /// CHECK: address-pinned to the standard stake-config account.
    #[account(address = Pubkey::from_str_const("StakeConfig11111111111111111111111111111111"))]
    pub stake_config: AccountInfo<'info>,

    /// `Rent` sysvar — required by `Initialize`.
    pub rent: Sysvar<'info, Rent>,
}

/// Handler:
///
///   1. CPI `system_program::create_account` — allocates STAKE_ACCOUNT_SPACE,
///      assigns to the stake program, funds with rent + delegation amount.
///      Funded by the **authority signer** (system-owned wallet), not
///      treasury — see module docstring on the genesis treasury custody
///      issue. Both `authority` and `stake_account` sign (authority by
///      keypair as the funder, stake_account by keypair as the new account).
///   2. CPI `StakeInstruction::Initialize` — sets the staker + withdrawer
///      authorities to the treasury PDA, so a future undelegate/withdraw ix
///      can pull stake back into treasury custody.
///   3. CPI `StakeInstruction::DelegateStake` — delegates the stake to
///      `vote_account`. Native warmup activates over ~1 epoch.
pub fn handler(
    ctx: Context<DelegateTreasuryStake>,
    args: DelegateTreasuryStakeArgs,
) -> Result<()> {
    require!(args.amount > 0, SubsidyError::BadInstructionData);

    let rent = &ctx.accounts.rent;
    let stake_rent = rent.minimum_balance(STAKE_ACCOUNT_SPACE as usize);
    let total_lamports = stake_rent
        .checked_add(args.amount)
        .ok_or(SubsidyError::BadInstructionData)?;

    let treasury_bump = ctx.bumps.treasury;
    let treasury_seeds: &[&[u8]] = &[b"treasury", core::slice::from_ref(&treasury_bump)];

    // 1. Create the stake account, funded by the AUTHORITY signer.
    //    Authority is system-owned (it's a regular keypair wallet) so
    //    system_program::create_account can debit it directly. Both
    //    authority and stake_account are real keypair signers in the tx,
    //    so a plain `invoke` (not invoke_signed) is sufficient — there
    //    are no PDA signers in this CPI.
    let create_ix = system_instruction::create_account(
        &ctx.accounts.authority.key(),
        &ctx.accounts.stake_account.key(),
        total_lamports,
        STAKE_ACCOUNT_SPACE,
        &STAKE_PROGRAM_ID,
    );
    anchor_lang::solana_program::program::invoke(
        &create_ix,
        &[
            ctx.accounts.authority.to_account_info(),
            ctx.accounts.stake_account.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
    )?;

    // 2. Initialize: set staker + withdrawer authorities to the treasury
    //    PDA so governance can deactivate/withdraw later.
    //
    //    StakeInstruction::Initialize wire format:
    //      u32 LE variant (= 0)
    //      Authorized { staker: Pubkey, withdrawer: Pubkey }
    //      Lockup { unix_timestamp: i64, epoch: u64, custodian: Pubkey }
    //
    //    No lockup — set all-zero (custodian = default Pubkey, ts/epoch = 0).
    let mut init_data = Vec::with_capacity(4 + 32 + 32 + 8 + 8 + 32);
    init_data.extend_from_slice(&STAKE_IX_INITIALIZE.to_le_bytes());
    let treasury_pk = ctx.accounts.treasury.key();
    init_data.extend_from_slice(treasury_pk.as_ref()); // staker
    init_data.extend_from_slice(treasury_pk.as_ref()); // withdrawer
    init_data.extend_from_slice(&0_i64.to_le_bytes()); // lockup.unix_timestamp
    init_data.extend_from_slice(&0_u64.to_le_bytes()); // lockup.epoch
    init_data.extend_from_slice(Pubkey::default().as_ref()); // lockup.custodian

    let init_ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(ctx.accounts.stake_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.rent.key(), false),
        ],
        data: init_data,
    };
    invoke_signed(
        &init_ix,
        &[
            ctx.accounts.stake_account.to_account_info(),
            ctx.accounts.rent.to_account_info(),
            ctx.accounts.stake_program.to_account_info(),
        ],
        &[],
    )?;

    // 3. Delegate: stake account → vote account, treasury PDA signs as
    //    staker authority.
    //
    //    StakeInstruction::DelegateStake wire format: just the u32 variant.
    //    Account ordering (per native stake program docs):
    //      0. stake_account     (writable)
    //      1. vote_account      (readonly)
    //      2. clock sysvar      (readonly)
    //      3. stake_history     (readonly)
    //      4. stake_config      (readonly)
    //      5. staker authority  (signer) — treasury PDA via seeds
    let delegate_ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: STAKE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(ctx.accounts.stake_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.vote_account.key(), false),
            AccountMeta::new_readonly(ctx.accounts.clock.key(), false),
            AccountMeta::new_readonly(ctx.accounts.stake_history.key(), false),
            AccountMeta::new_readonly(ctx.accounts.stake_config.key(), false),
            AccountMeta::new_readonly(ctx.accounts.treasury.key(), true),
        ],
        data: STAKE_IX_DELEGATE.to_le_bytes().to_vec(),
    };
    invoke_signed(
        &delegate_ix,
        &[
            ctx.accounts.stake_account.to_account_info(),
            ctx.accounts.vote_account.to_account_info(),
            ctx.accounts.clock.to_account_info(),
            ctx.accounts.stake_history.to_account_info(),
            ctx.accounts.stake_config.to_account_info(),
            ctx.accounts.treasury.to_account_info(),
            ctx.accounts.stake_program.to_account_info(),
        ],
        &[treasury_seeds],
    )?;

    Ok(())
}

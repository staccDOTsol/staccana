//! `stake_to_productive` — governance-gated CPI into the bridge `mint` ix.
//!
//! Deposits `args.amount` of treasury SOL into the productive position by invoking the
//! bridge's `mint` instruction with the treasury PDA as the depositor / recipient ATA
//! owner. The bridge then issues the corresponding amount of staked-asset wrapper
//! tokens (e.g. pSYRUP) into a treasury-owned ATA.
//!
//! Federation signatures are NOT verified here — the bridge's `mint` ix already checks
//! M-of-N ed25519 precompile signatures over the canonical mint attestation message
//! (SPEC §5.4). The relayer who calls `stake_to_productive` is responsible for bundling
//! those precompile ixs ahead of this one.
//!
//! Authorization: the `authority` signer must equal `subsidy_config.governance` (the
//! Squads vault per SPEC §7.4).
//!
//! Accounting: increments `subsidy_config.productive_deposit_total` by `args.amount`
//! (the underlying lamports value, not the wrapper-token amount — those differ once R
//! drifts above 1.0). Used as a sanity field; not load-bearing in payout math.
//!
//! ## CPI shape
//!
//! Rather than hard-binding to the bridge's `cpi::*` helpers (which would force this
//! crate to track the bridge's account-context type names verbatim), we invoke the
//! bridge via the lower-level `solana_program::program::invoke_signed` path, building
//! the bridge `mint` instruction by hand. This decouples the two crates' Anchor
//! account-context surface; the only contract is the bridge's instruction-data layout
//! plus account-list ordering, which are both pinned in `programs/bridge/src/lib.rs`.
//!
//! TODO(v1.1): once the Anchor 0.30 → 1.x migration lands and the workspace can
//! resolve a single `solana-program` version (see Cargo.toml workspace comment),
//! switch this to typed `staccana_bridge::cpi::mint` for stronger type checking and
//! IDL coupling.

use crate::error::SubsidyError;
use crate::state::SubsidyConfig;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
// `InstructionData::data()` is what packs `discriminator || borsh(args)` into the wire
// bytes. Not in the Anchor prelude as of 0.30, so import explicitly.
use anchor_lang::InstructionData;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct StakeToProductiveArgs {
    /// Amount of underlying SOL (lamports) to deposit into the productive position.
    /// Must equal the `value_after_fee` argument passed to the bridge's `mint` ix —
    /// the federation has already attested to this exact value.
    pub amount: u64,

    /// Bridge `nonce` for this deposit. Strictly per the bridge's nonce invariants
    /// (SPEC §5.4 step 5): the bridge will reject replays with the same `(asset_id,
    /// nonce)` tuple via the `["nonce_in", asset_id, nonce]` PDA.
    pub bridge_nonce: u64,

    /// Federation signer indices (M-of-N) for the bridge `mint` attestation. Forwarded
    /// verbatim as the bridge ix's `federation_indices` field.
    pub federation_indices: Vec<u8>,
}

#[derive(Accounts)]
pub struct StakeToProductive<'info> {
    /// Must equal `subsidy_config.governance`. Pays for any rent the CPI requires (in
    /// practice the `nonce_in` PDA the bridge initializes).
    #[account(
        mut,
        constraint = authority.key() == subsidy_config.governance
            @ SubsidyError::BadInstructionData,
    )]
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [b"subsidy_config"],
        bump = subsidy_config.bump,
    )]
    pub subsidy_config: Account<'info, SubsidyConfig>,

    /// Bridge program. Must equal `subsidy_config.bridge_program_id`.
    /// CHECK: address-verified in the handler against the stored bridge program id.
    pub bridge_program: AccountInfo<'info>,

    // The remaining accounts are the bridge `mint` ix's account list, passed through
    // verbatim. We declare them as `AccountInfo` here rather than typed
    // `Account<...>` because Anchor doesn't generate the bridge's account types into
    // this crate's IDL surface, and the bridge does its own validation on every account
    // anyway.
    /// CHECK: validated by the bridge's `mint` handler.
    #[account(mut)]
    pub bridge_asset_config: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `mint` handler.
    pub bridge_ratio_state: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `mint` handler.
    pub bridge_federation_set: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `mint` handler.
    #[account(mut)]
    pub bridge_staccana_mint: AccountInfo<'info>,
    /// Recipient ATA — must be owned by the treasury PDA so the productive position
    /// accumulates in treasury custody.
    /// CHECK: validated by the bridge's `mint` handler (mint == staccana_mint, etc.).
    #[account(mut)]
    pub treasury_recipient_ata: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `mint` handler.
    #[account(mut)]
    pub bridge_nonce_in: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `mint` handler against the canonical sysvar pubkey.
    pub instructions_sysvar: AccountInfo<'info>,
    /// CHECK: validated by the bridge's `mint` handler.
    pub bridge_token_program: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

/// Handler — sanity-check args, build the bridge `mint` ix, CPI it.
pub fn handler(ctx: Context<StakeToProductive>, args: StakeToProductiveArgs) -> Result<()> {
    require!(args.amount > 0, SubsidyError::ZeroStakeAmount);

    // Snapshot the immutable fields off the config so the mutable re-borrow at the
    // bottom of this function doesn't fight the earlier read.
    let bridge_program_id = ctx.accounts.subsidy_config.bridge_program_id;
    let productive_asset_id = ctx.accounts.subsidy_config.productive_asset_id;
    require_keys_eq!(
        ctx.accounts.bridge_program.key(),
        bridge_program_id,
        SubsidyError::BadInstructionData
    );

    // The bridge `mint` ix data layout (per `programs/bridge/src/instructions/mint.rs`):
    //   - 8-byte Anchor discriminator for "mint"
    //   - asset_id (u32 LE)
    //   - value_after_fee (u64 LE)
    //   - recipient (32 bytes)
    //   - nonce (u64 LE)
    //   - federation_indices (4-byte length prefix + raw bytes — Borsh Vec<u8>)
    //
    // Anchor's discriminator is `sha256("global:mint")[0..8]`. Hard-coding the eight
    // bytes here would fragile-couple us to the bridge's source; instead we use
    // Anchor's `discriminator` machinery via the `staccana_bridge` re-exports, which
    // recomputes the bytes at compile time.
    let recipient_bytes = ctx.accounts.treasury_recipient_ata.key().to_bytes();

    // Construct the bridge's `Mint` ix data. The `staccana_bridge::instruction::Mint`
    // struct is the Anchor-generated wrapper carrying the ix args; serializing it (with
    // the discriminator prepended via `InstructionData::data()`) produces the canonical
    // bridge-`mint` ix data.
    let mint_args = staccana_bridge::instructions::mint::MintArgs {
        asset_id: productive_asset_id,
        value_after_fee: args.amount,
        recipient: recipient_bytes,
        nonce: args.bridge_nonce,
        federation_indices: args.federation_indices.clone(),
    };
    let ix_struct = staccana_bridge::instruction::Mint { args: mint_args };
    let data = ix_struct.data();

    // Account list MUST match the bridge `BridgeMint` account context order:
    //   payer, asset_config, ratio_state, federation_set, staccana_mint, recipient_ata,
    //   nonce_in, instructions_sysvar, token_program, system_program.
    // The treasury PDA signs as the payer for the bridge ix's nonce_in allocation.
    let accounts = vec![
        AccountMeta::new(ctx.accounts.authority.key(), true),
        AccountMeta::new(ctx.accounts.bridge_asset_config.key(), false),
        AccountMeta::new_readonly(ctx.accounts.bridge_ratio_state.key(), false),
        AccountMeta::new_readonly(ctx.accounts.bridge_federation_set.key(), false),
        AccountMeta::new(ctx.accounts.bridge_staccana_mint.key(), false),
        AccountMeta::new(ctx.accounts.treasury_recipient_ata.key(), false),
        AccountMeta::new(ctx.accounts.bridge_nonce_in.key(), false),
        AccountMeta::new_readonly(ctx.accounts.instructions_sysvar.key(), false),
        AccountMeta::new_readonly(ctx.accounts.bridge_token_program.key(), false),
        AccountMeta::new_readonly(ctx.accounts.system_program.key(), false),
    ];

    let ix = Instruction {
        program_id: ctx.accounts.bridge_program.key(),
        accounts,
        data,
    };

    let account_infos = [
        ctx.accounts.authority.to_account_info(),
        ctx.accounts.bridge_asset_config.to_account_info(),
        ctx.accounts.bridge_ratio_state.to_account_info(),
        ctx.accounts.bridge_federation_set.to_account_info(),
        ctx.accounts.bridge_staccana_mint.to_account_info(),
        ctx.accounts.treasury_recipient_ata.to_account_info(),
        ctx.accounts.bridge_nonce_in.to_account_info(),
        ctx.accounts.instructions_sysvar.to_account_info(),
        ctx.accounts.bridge_token_program.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
    ];

    // No PDA signing required — the authority signs as itself. Bridge-side errors
    // surface verbatim through the `?` (Anchor implements `From<ProgramError>` for
    // its `Error` type) so callers can distinguish "bridge rejected the attestation"
    // from "validator-subsidy rejected the args" without re-mapping.
    invoke_signed(&ix, &account_infos, &[])?;

    let cfg = &mut ctx.accounts.subsidy_config;
    cfg.productive_deposit_total = cfg
        .productive_deposit_total
        .checked_add(args.amount)
        .ok_or(SubsidyError::BadInstructionData)?;

    Ok(())
}

//! `mint` — relay an inbound (mainnet → staccana) attestation; mint Token-22 wrapper.
//!
//! Off-chain choreography (SPEC.md §5.4):
//! 1. User deposits `X` underlying to the mainnet vault.
//! 2. Vault deducts mainnet-side fee, emits a `Deposit` event.
//! 3. Federation observes, signs an attestation `(asset_id, value_after_fee, recipient,
//!    nonce)` with M-of-N ed25519 sigs.
//! 4. User (or relayer) bundles the M ed25519 precompile ixs with this `mint` ix.
//!
//! On-chain effects:
//! 1. Verify M federation signatures over the canonical mint message.
//! 2. Apply the staccana-side `mint_fee_bps` to `value_after_fee` (the vault already
//!    deducted the mainnet-side fee, this is the second deduction).
//! 3. Read `R_q64`, compute `mint_amount = (net_value << 64) / R_q64`.
//! 4. CPI into Token-22 to mint to the recipient ATA. The bridge program PDA is the
//!    mint authority — derived from `["asset", asset_id]`, signed for via `with_signer`.
//! 5. Initialize the `["nonce_in", asset_id, nonce]` PDA so any replay rejects.

use crate::attestation::{
    apply_bps_fee, build_mint_message, check_unique_indices, mint_amount_for_value,
};
use crate::ed25519::{parse_ed25519_at, parse_ed25519_batch_at, require_instructions_sysvar};
use crate::error::BridgeError;
use crate::state::{AssetConfig, FederationSet, NonceConsumed, RatioState};
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    self, Mint, MintTo, TokenAccount, TokenInterface,
};

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct MintArgs {
    pub asset_id: u32,
    pub value_after_fee: u64,
    pub recipient: [u8; 32],
    pub nonce: u64,
    pub federation_indices: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: MintArgs)]
pub struct BridgeMint<'info> {
    /// Pays for the new `nonce_in` PDA.
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [b"asset", args.asset_id.to_le_bytes().as_ref()],
        bump = asset_config.bump,
    )]
    pub asset_config: Account<'info, AssetConfig>,

    #[account(
        seeds = [b"ratio", args.asset_id.to_le_bytes().as_ref()],
        bump = ratio_state.bump,
    )]
    pub ratio_state: Account<'info, RatioState>,

    #[account(
        seeds = [b"federation"],
        bump = federation_set.bump,
    )]
    pub federation_set: Account<'info, FederationSet>,

    #[account(
        mut,
        address = asset_config.staccana_mint,
    )]
    pub staccana_mint: InterfaceAccount<'info, Mint>,

    /// Recipient ATA. We don't enforce ownership here — the SPL Token-22 program will
    /// reject if `recipient_ata.owner != args.recipient`. We DO require the mint match.
    #[account(
        mut,
        token::mint = staccana_mint,
    )]
    pub recipient_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init,
        payer = payer,
        space = NonceConsumed::SPACE,
        seeds = [b"nonce_in", args.asset_id.to_le_bytes().as_ref(), args.nonce.to_le_bytes().as_ref()],
        bump,
    )]
    pub nonce_in: Account<'info, NonceConsumed>,

    /// CHECK: validated against the canonical sysvar pubkey in the handler.
    pub instructions_sysvar: UncheckedAccount<'info>,

    /// Token program — `Interface<TokenInterface>` accepts either SPL Token or
    /// Token-2022. The asset's mint pins which one is actually used at the account
    /// level; runtime CPI dispatches accordingly.
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

/// Handler — verify federation sigs, compute mint amount, CPI into Token-22.
pub fn handler(ctx: Context<BridgeMint>, args: MintArgs) -> Result<()> {
    require_instructions_sysvar(&ctx.accounts.instructions_sysvar)?;

    // Recipient binding — the ATA's `owner` field on Token-22 must match the attested
    // recipient pubkey. Refusing to mint to anyone other than the attested recipient
    // closes a hijack vector where a relayer swaps the ATA.
    require!(
        ctx.accounts.recipient_ata.owner.to_bytes() == args.recipient,
        BridgeError::BadInstructionData
    );

    let fed = &ctx.accounts.federation_set;
    require!(fed.n > 0 && fed.m > 0, BridgeError::BadFederationSet);
    require!(
        args.federation_indices.len() == fed.m as usize,
        BridgeError::InsufficientFederationSignatures
    );
    check_unique_indices(&args.federation_indices, fed.n)?;

    let expected_msg =
        build_mint_message(args.asset_id, args.value_after_fee, &args.recipient, args.nonce);

    let sysvar = &ctx.accounts.instructions_sysvar;
    let current_ix_index = solana_instructions_sysvar::load_current_index_checked(sysvar)
        .map_err(|_| BridgeError::BadInstructionsSysvar)?;
    let m = fed.m as usize;

    // Two acceptable layouts (back-compat): the federation may have submitted M
    // separate single-sig precompile ixs immediately preceding this one, OR a
    // single batched precompile ix carrying all M sigs. Try the batched form
    // first since it's the smaller-footprint path used by post-v1 relayers.
    require!(
        (current_ix_index as usize) >= 1,
        BridgeError::InsufficientFederationSignatures
    );

    let batched_ok = matches!(
        parse_ed25519_batch_at(sysvar, (current_ix_index as usize) - 1),
        Ok(ref batch) if batch.len() == m,
    );

    if batched_ok {
        // SAFETY: matched the predicate above.
        let batch = parse_ed25519_batch_at(sysvar, (current_ix_index as usize) - 1)
            .expect("re-parse same ix");
        for (i, &member_idx) in args.federation_indices.iter().enumerate() {
            let parsed = &batch[i];
            require!(
                parsed.message == expected_msg,
                BridgeError::BadAttestationMessage
            );
            let expected_member = fed.members[member_idx as usize];
            require!(
                parsed.pubkey == expected_member.to_bytes(),
                BridgeError::BadFederationSigner
            );
        }
    } else {
        require!(
            (current_ix_index as usize) >= m,
            BridgeError::InsufficientFederationSignatures
        );
        for (i, &member_idx) in args.federation_indices.iter().enumerate() {
            let ix_index = (current_ix_index as usize) - m + i;
            let parsed = parse_ed25519_at(sysvar, ix_index)?;
            require!(
                parsed.message == expected_msg,
                BridgeError::BadAttestationMessage
            );
            let expected_member = fed.members[member_idx as usize];
            require!(
                parsed.pubkey == expected_member.to_bytes(),
                BridgeError::BadFederationSigner
            );
        }
    }

    let cfg = &ctx.accounts.asset_config;
    let ratio = &ctx.accounts.ratio_state;

    // Apply the staccana-side mint fee — implicit "fee" by minting fewer tokens than
    // `value_after_fee / R`. Fees compound R on next federation publish.
    let net_value = apply_bps_fee(args.value_after_fee, cfg.mint_fee_bps);
    let mint_amount = mint_amount_for_value(net_value, ratio.r_q64)?;

    // CPI into the token program with the AssetConfig PDA as mint authority. Outer
    // slice level: one set of seeds. Inner level: the seed components themselves.
    let asset_id_bytes = args.asset_id.to_le_bytes();
    let bump = cfg.bump;
    let signer_seeds: &[&[&[u8]]] = &[&[b"asset", asset_id_bytes.as_ref(), &[bump]]];

    let cpi_accounts = MintTo {
        mint: ctx.accounts.staccana_mint.to_account_info(),
        to: ctx.accounts.recipient_ata.to_account_info(),
        authority: ctx.accounts.asset_config.to_account_info(),
    };
    // Anchor 1.0: `CpiContext::new` takes the program id (`Pubkey`) instead of an
    // `AccountInfo`. The `with_signer` builder is unchanged.
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.key(), cpi_accounts)
        .with_signer(signer_seeds);
    token_interface::mint_to(cpi_ctx, mint_amount)?;

    // Nonce-consumed PDA was init'd by Anchor; populate the bump so future inspectors
    // can verify the derivation.
    ctx.accounts.nonce_in.bump = ctx.bumps.nonce_in;

    emit!(MintEvent {
        asset_id: args.asset_id,
        recipient: args.recipient,
        value_after_fee: args.value_after_fee,
        net_value,
        mint_amount,
        r_q64: ratio.r_q64,
        nonce: args.nonce,
    });

    Ok(())
}

/// Emitted on every successful mint. Off-chain indexers consume this to update user
/// dashboards without re-deriving R or scanning ATA deltas.
#[event]
pub struct MintEvent {
    pub asset_id: u32,
    pub recipient: [u8; 32],
    pub value_after_fee: u64,
    pub net_value: u64,
    pub mint_amount: u64,
    pub r_q64: u128,
    pub nonce: u64,
}

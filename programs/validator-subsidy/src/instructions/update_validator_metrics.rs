//! `update_validator_metrics` — federation-attested update of a validator's per-epoch
//! metrics.
//!
//! Same M-of-N ed25519 precompile pattern as the bridge's `update_ratio` (SPEC §5.3):
//! M federation members co-sign the canonical `STACCANA_VALIDATOR_METRICS_V1` message
//! via separate ed25519 precompile ixs preceding this one. The handler walks back from
//! the current ix index, parses each precompile, and confirms message + signer.
//!
//! Validation:
//! - `uptime_bps` in `[0, 10_000]`.
//! - `nonce` strictly greater than the validator's `last_metrics_nonce`.
//! - All M signers are distinct registered federation members.
//!
//! Effect: writes new metrics + slot + nonce into the [`ValidatorRecord`].

use crate::ed25519::{parse_ed25519_batch_at, require_instructions_sysvar};
use crate::error::SubsidyError;
use crate::state::{SubsidyConfig, ValidatorRecord};
use crate::subsidy::{build_metrics_message, check_unique_indices, check_uptime_bps};
use anchor_lang::prelude::*;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct UpdateValidatorMetricsArgs {
    pub validator: Pubkey,
    pub uptime_bps: u16,
    pub delegated_stake: u64,
    pub votes_cast: u64,
    pub slot: u64,
    pub nonce: u64,
    /// Indices into `SubsidyConfig::federation_members`, one per signature. Length M;
    /// duplicates rejected.
    pub federation_indices: Vec<u8>,
}

#[derive(Accounts)]
#[instruction(args: UpdateValidatorMetricsArgs)]
pub struct UpdateValidatorMetrics<'info> {
    /// Anyone can submit — the verification is in the M signatures. Charging the relayer
    /// is appropriate since the federation may have many willing relayers.
    pub relayer: Signer<'info>,

    #[account(
        seeds = [b"subsidy_config"],
        bump = subsidy_config.bump,
    )]
    pub subsidy_config: Account<'info, SubsidyConfig>,

    #[account(
        mut,
        seeds = [b"validator", args.validator.as_ref()],
        bump = validator_record.bump,
        constraint = validator_record.validator == args.validator
            @ SubsidyError::ValidatorNotRegistered,
    )]
    pub validator_record: Account<'info, ValidatorRecord>,

    /// CHECK: validated against the canonical sysvar pubkey in the handler.
    pub instructions_sysvar: AccountInfo<'info>,
}

/// Handler — verify M federation signatures, then write the new metrics.
pub fn handler(
    ctx: Context<UpdateValidatorMetrics>,
    args: UpdateValidatorMetricsArgs,
) -> Result<()> {
    require_instructions_sysvar(&ctx.accounts.instructions_sysvar)?;
    check_uptime_bps(args.uptime_bps)?;

    let cfg = &ctx.accounts.subsidy_config;
    require!(
        cfg.federation_n > 0 && cfg.federation_m > 0,
        SubsidyError::BadFederationSet
    );
    require!(
        args.federation_indices.len() == cfg.federation_m as usize,
        SubsidyError::InsufficientFederationSignatures
    );
    check_unique_indices(&args.federation_indices, cfg.federation_n)?;

    let validator_bytes = args.validator.to_bytes();
    let expected_msg = build_metrics_message(
        &validator_bytes,
        args.uptime_bps,
        args.delegated_stake,
        args.votes_cast,
        args.slot,
        args.nonce,
    );

    // Walk the SINGLE batched ed25519 precompile ix immediately preceding
    // this one. The batched form (M sigs over a shared message in one ix)
    // saves ~75 × (M-1) bytes of tx size vs. M individual precompile ixs;
    // at M=5 the single-sig form overflows the 1232-byte tx ceiling.
    let sysvar = &ctx.accounts.instructions_sysvar;
    let current_ix_index = solana_instructions_sysvar::load_current_index_checked(sysvar)
        .map_err(|_| SubsidyError::BadInstructionsSysvar)?;
    require!(
        current_ix_index >= 1,
        SubsidyError::InsufficientFederationSignatures
    );
    let precompile_ix_index = (current_ix_index as usize) - 1;
    let parsed_sigs = parse_ed25519_batch_at(sysvar, precompile_ix_index)?;
    let m = cfg.federation_m as usize;
    require!(
        parsed_sigs.len() == m,
        SubsidyError::InsufficientFederationSignatures
    );

    for (i, &member_idx) in args.federation_indices.iter().enumerate() {
        let parsed = &parsed_sigs[i];
        require!(
            parsed.message == expected_msg,
            SubsidyError::BadAttestationMessage
        );
        let expected_member = cfg.federation_members[member_idx as usize];
        require!(
            parsed.pubkey == expected_member.to_bytes(),
            SubsidyError::BadFederationSigner
        );
    }

    let rec = &mut ctx.accounts.validator_record;
    require!(
        args.nonce > rec.last_metrics_nonce,
        SubsidyError::StaleMetricsNonce
    );

    rec.uptime_bps = args.uptime_bps;
    rec.delegated_stake = args.delegated_stake;
    rec.votes_cast = args.votes_cast;
    rec.last_metrics_slot = args.slot;
    rec.last_metrics_nonce = args.nonce;

    Ok(())
}

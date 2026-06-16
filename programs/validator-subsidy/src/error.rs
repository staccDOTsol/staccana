//! Program-level errors for the validator-subsidy instructions.
//!
//! Each variant maps to a distinct on-chain failure mode so that off-chain tooling can
//! distinguish "bad input" from "wrong epoch" from "double-distribution attempt." Variants
//! convert into `anchor_lang::error::Error` via the `#[error_code]` macro.

use anchor_lang::prelude::*;

/// Errors the validator-subsidy instructions can return. Variants are stable — never
/// reorder, only append. The discriminant is exposed via Anchor's standard error
/// machinery and consumed by clients.
#[error_code]
pub enum SubsidyError {
    #[msg("instruction data could not be deserialized or is otherwise malformed")]
    BadInstructionData = 0,

    #[msg("subsidy config PDA seeds do not match the canonical derivation")]
    BadSubsidyConfigPda = 1,

    #[msg("validator registry PDA seeds do not match the canonical derivation")]
    BadValidatorRegistryPda = 2,

    #[msg("validator record PDA seeds do not match the validator pubkey supplied")]
    BadValidatorRecordPda = 3,

    #[msg("epoch accrual PDA seeds do not match the epoch supplied")]
    BadEpochAccrualPda = 4,

    #[msg("instructions sysvar account is not the canonical sysvar")]
    BadInstructionsSysvar = 5,

    #[msg("M federation signatures required, fewer than M ed25519 precompile ix found")]
    InsufficientFederationSignatures = 6,

    #[msg("federation signer index is out of range for the registered set")]
    FederationIndexOutOfRange = 7,

    #[msg("duplicate federation signer index within a single attestation")]
    DuplicateFederationSigner = 8,

    #[msg("ed25519 precompile signed the wrong attestation message")]
    BadAttestationMessage = 9,

    #[msg("ed25519 precompile signed by a key not in the federation set")]
    BadFederationSigner = 10,

    #[msg("ed25519 precompile instruction is malformed")]
    BadEd25519Precompile = 11,

    #[msg("validator pubkey already present in the registry")]
    ValidatorAlreadyRegistered = 12,

    #[msg("validator registry is full — increase MAX_VALIDATORS and re-deploy")]
    ValidatorRegistryFull = 13,

    #[msg("validator pubkey not found in the registry")]
    ValidatorNotRegistered = 14,

    #[msg("uptime_bps must be in [0, 10_000]")]
    UptimeBpsOutOfRange = 15,

    #[msg("metrics nonce must be strictly greater than the validator's last_metrics_nonce")]
    StaleMetricsNonce = 16,

    #[msg("epoch accrual already marked distributed — second call is a no-op error")]
    EpochAlreadyDistributed = 17,

    #[msg("bootstrap_distribute called for an epoch >= BOOTSTRAP_EPOCHS")]
    BootstrapEpochExpired = 18,

    #[msg("epoch_le seed component does not match epoch passed in args")]
    EpochMismatch = 19,

    #[msg("total weight is zero — no validators have any stake/uptime/votes")]
    ZeroTotalWeight = 20,

    #[msg("computed share overflowed u64")]
    ShareOverflow = 21,

    #[msg("yield_observed not yet populated for this epoch — oracle has not attested")]
    YieldNotPopulated = 22,

    #[msg("treasury PDA balance is insufficient to cover the planned distribution")]
    InsufficientTreasuryBalance = 23,

    #[msg("amount to stake is zero — would be a no-op CPI")]
    ZeroStakeAmount = 24,

    #[msg("amount to unstake is zero — would be a no-op CPI")]
    ZeroUnstakeAmount = 25,

    #[msg("M-of-N parameters out of range (M == 0 or N > MAX_FEDERATION_MEMBERS)")]
    BadFederationParams = 26,

    #[msg("federation set is uninitialized or invalid")]
    BadFederationSet = 27,

    #[msg("remaining_accounts list does not match the validator registry contents")]
    RemainingAccountsMismatch = 28,

    #[msg("bootstrap reserve has been fully drained")]
    BootstrapReserveExhausted = 29,

    #[msg("signer is not the configured ADMIN_AUTHORITY for this privileged ix")]
    Unauthorized = 30,
}

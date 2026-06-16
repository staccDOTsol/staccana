//! Program-level errors for the bridge instructions.
//!
//! Each variant maps to a distinct on-chain failure mode so that off-chain tooling can
//! distinguish "you submitted garbage" from "the chain has a bug" from "you tried to
//! double-spend a nonce." Variants convert into `anchor_lang::error::Error` via the
//! `#[error_code]` macro.

use anchor_lang::prelude::*;

/// Errors the bridge instructions can return. Variants are stable — never reorder, only
/// append. The discriminant is exposed via Anchor's standard error machinery and consumed
/// by clients.
#[error_code]
pub enum BridgeError {
    #[msg("instruction data could not be deserialized")]
    BadInstructionData = 0,

    #[msg("asset config PDA seeds do not match the asset_id supplied")]
    BadAssetConfigPda = 1,

    #[msg("ratio state PDA seeds do not match the asset_id supplied")]
    BadRatioStatePda = 2,

    #[msg("federation set PDA seeds do not match the canonical derivation")]
    BadFederationSetPda = 3,

    #[msg("nonce-consumed PDA seeds do not match the (asset_id, nonce) supplied")]
    BadNonceInPda = 4,

    #[msg("nonce-out counter PDA seeds do not match the asset_id supplied")]
    BadNonceOutPda = 5,

    #[msg("instructions sysvar account is not the canonical sysvar")]
    BadInstructionsSysvar = 6,

    #[msg("M federation signatures required, fewer than M ed25519 precompile ix found")]
    InsufficientFederationSignatures = 7,

    #[msg("federation signer index is out of range for the registered set")]
    FederationIndexOutOfRange = 8,

    #[msg("duplicate federation signer index within a single attestation")]
    DuplicateFederationSigner = 9,

    #[msg("ed25519 precompile signed the wrong attestation message")]
    BadAttestationMessage = 10,

    #[msg("ed25519 precompile signed by a key not in the federation set")]
    BadFederationSigner = 11,

    #[msg("ed25519 precompile instruction is malformed")]
    BadEd25519Precompile = 12,

    #[msg("update_ratio called too soon — slot < last_slot + R_PUBLISH_INTERVAL_SLOTS")]
    RatioUpdateTooSoon = 13,

    #[msg("attested asset_id does not match the AssetConfig PDA")]
    AssetIdMismatch = 14,

    #[msg("attested mint_supply is zero — would divide by zero computing R")]
    ZeroMintSupply = 15,

    #[msg("ratio R is zero — bridge has not been initialized for this asset")]
    ZeroRatio = 16,

    #[msg("nonce already consumed for this (asset_id, nonce) tuple")]
    NonceAlreadyConsumed = 17,

    #[msg("computed mint amount overflowed u64")]
    MintAmountOverflow = 18,

    #[msg("computed release amount overflowed u64")]
    ReleaseAmountOverflow = 19,

    #[msg("burn amount is zero")]
    ZeroBurnAmount = 20,

    #[msg("federation set is uninitialized or invalid")]
    BadFederationSet = 21,

    #[msg("M-of-N parameters out of range (M == 0 or N > MAX_FEDERATION_MEMBERS)")]
    BadFederationParams = 22,

    #[msg("R is hard-pinned for this asset (e.g. wSOL); update_ratio is not permitted")]
    RatioLocked = 23,

    #[msg("AMM oracle pool reserves are zero — cannot quote a price")]
    AmmEmptyReserves = 24,

    #[msg("AMM-quoted output amount overflowed u64")]
    AmmQuoteOverflow = 25,

    #[msg("AMM-quoted output is below the user-supplied minimum (slippage exceeded)")]
    AmmSlippageExceeded = 26,

    #[msg("native SOL transfer to bridge escrow failed")]
    NativeSolTransferFailed = 27,

    #[msg("signer is not the configured ADMIN_AUTHORITY for this privileged ix")]
    Unauthorized = 28,
}

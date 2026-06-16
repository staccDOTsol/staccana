//! Program-level errors for the mainnet vault.
//!
//! Variants are stable — never reorder, only append. Discriminants surface to clients
//! via Anchor's `#[error_code]` machinery.

use anchor_lang::prelude::*;

#[error_code]
pub enum VaultError {
    #[msg("instruction data could not be deserialized")]
    BadInstructionData = 0,

    #[msg("vault config PDA seeds do not match the asset_id supplied")]
    BadVaultConfigPda = 1,

    #[msg("federation set PDA seeds do not match the canonical derivation")]
    BadFederationSetPda = 2,

    #[msg("nonce-out marker PDA seeds do not match the (asset_id, nonce) supplied")]
    BadNonceOutPda = 3,

    #[msg("nonce-in counter PDA seeds do not match the asset_id supplied")]
    BadNonceInPda = 4,

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

    #[msg("attested asset_id does not match the VaultConfig PDA")]
    AssetIdMismatch = 12,

    #[msg("nonce already consumed for this (asset_id, nonce) tuple")]
    NonceAlreadyConsumed = 13,

    #[msg("federation set is uninitialized or invalid")]
    BadFederationSet = 14,

    #[msg("M-of-N parameters out of range (M == 0 or N > MAX_FEDERATION_MEMBERS)")]
    BadFederationParams = 15,

    #[msg("deposit amount is zero")]
    ZeroDepositAmount = 16,

    #[msg("release amount is zero")]
    ZeroReleaseAmount = 17,

    #[msg("native SOL transfer to or from the vault failed")]
    NativeSolTransferFailed = 18,

    #[msg("attempted SPL transfer on a wSOL (native-SOL-backed) vault, or vice versa")]
    AssetKindMismatch = 19,

    #[msg("vault token account mint does not match the configured underlying mint")]
    BadVaultTokenAccount = 20,

    #[msg("vault PDA bump mismatch")]
    BadVaultBump = 21,

    #[msg("fee bps exceeds 10000 (100%)")]
    BadFeeBps = 22,

    #[msg("signer is not the configured ADMIN_AUTHORITY for this privileged ix")]
    Unauthorized = 23,
}

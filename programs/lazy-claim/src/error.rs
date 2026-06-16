//! Program-level errors for the lazy-claim instruction.
//!
//! Each variant maps to a distinct on-chain failure mode so that off-chain tooling can
//! distinguish "you submitted garbage" from "the chain has a bug" from "you tried to claim
//! twice." Variants convert into `ProgramError::Custom(code)` via the `From` impl below.

use solana_program::program_error::ProgramError;
use thiserror::Error;

/// Errors the lazy-claim instruction can return. Variants are stable — never reorder, only
/// append. The discriminant is exposed via `ProgramError::Custom` and consumed by clients.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[repr(u32)]
pub enum LazyClaimError {
    #[error("instruction data could not be deserialized")]
    BadInstructionData = 0,

    #[error("unknown instruction discriminator")]
    UnknownInstruction = 1,

    #[error("config account does not match expected layout or owner")]
    BadConfigAccount = 2,

    #[error("Merkle proof length disagrees with proof_flags bit length")]
    ProofLengthMismatch = 3,

    #[error("Merkle proof did not reproduce the embedded claimable_root")]
    BadMerkleProof = 4,

    #[error("recipient pubkey at account index 0 must equal the claimed pubkey")]
    RecipientMismatch = 5,

    #[error("expected an ed25519 precompile instruction directly preceding the claim ix")]
    MissingEd25519Precompile = 6,

    #[error("ed25519 precompile instruction is malformed or unverifiable")]
    BadEd25519Precompile = 7,

    #[error("ed25519 precompile signs the wrong public key")]
    SignerPubkeyMismatch = 8,

    #[error("ed25519 precompile signs the wrong message payload")]
    SignedMessageMismatch = 9,

    #[error("claimed-marker PDA already exists — this pubkey has already claimed")]
    AlreadyClaimed = 10,

    #[error("claimed-marker PDA address does not match the expected derivation")]
    BadClaimedMarkerPda = 11,

    #[error("treasury account does not match the configured treasury PDA")]
    BadTreasuryAccount = 12,

    #[error("instructions sysvar account is not the canonical sysvar")]
    BadInstructionsSysvar = 13,

    #[error("proof buffer PDA address does not match the expected derivation")]
    BadProofBufferPda = 14,

    #[error("proof buffer write would overflow the declared total length")]
    ProofBufferOverflow = 15,

    #[error("proof buffer payload was not fully written before claim_from_buffer")]
    ProofBufferIncomplete = 16,

    #[error("proof buffer header is malformed or has wrong discriminator")]
    BadProofBuffer = 17,

    #[error("proof buffer total length disagrees with declared proof_len")]
    ProofBufferLengthMismatch = 18,
}

impl From<LazyClaimError> for ProgramError {
    fn from(e: LazyClaimError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

//! Program-level errors for the megadrop instructions.
//!
//! Each variant maps to a distinct on-chain failure mode so off-chain tooling can
//! distinguish "bad input" from "tranche locked" from "already claimed." Variants
//! convert into `anchor_lang::error::Error` via the `#[error_code]` macro.

use anchor_lang::prelude::*;

/// Errors the megadrop instructions can return. Variants are stable — never reorder,
/// only append. The discriminant is exposed via Anchor's standard error machinery and
/// consumed by clients.
#[error_code]
pub enum MegadropError {
    #[msg("instruction data could not be deserialized or is otherwise malformed")]
    BadInstructionData = 0,

    #[msg("megadrop config PDA seeds do not match the canonical derivation")]
    BadMegadropConfigPda = 1,

    #[msg("claimed megadrop PDA seeds do not match the holder pubkey supplied")]
    BadClaimedMegadropPda = 2,

    #[msg("Merkle proof did not reproduce the embedded claimable_root")]
    BadMerkleProof = 3,

    #[msg("Merkle proof length disagrees with proof_flags bit length")]
    ProofLengthMismatch = 4,

    #[msg("instructions sysvar account is not the canonical sysvar")]
    BadInstructionsSysvar = 5,

    #[msg("expected an ed25519 precompile instruction directly preceding the claim ix")]
    MissingEd25519Precompile = 6,

    #[msg("ed25519 precompile instruction is malformed or unverifiable")]
    BadEd25519Precompile = 7,

    #[msg("ed25519 precompile signs the wrong public key")]
    SignerPubkeyMismatch = 8,

    #[msg("ed25519 precompile signs the wrong message payload")]
    SignedMessageMismatch = 9,

    #[msg("recipient pubkey does not match holder_pubkey supplied")]
    RecipientMismatch = 10,

    #[msg("treasury account does not match the configured treasury authority")]
    BadTreasuryAccount = 11,

    #[msg("tranche index out of range — must be in [1, 10]")]
    TrancheIndexOutOfRange = 12,

    #[msg("requested tranche has not yet unlocked at the current calendar month")]
    TrancheNotUnlocked = 13,

    #[msg("requested tranche has already been claimed")]
    TrancheAlreadyClaimed = 14,

    #[msg("tranche_indices list is empty — would be a no-op claim")]
    EmptyTrancheList = 15,

    #[msg("duplicate tranche index within a single claim ix")]
    DuplicateTrancheIndex = 16,

    #[msg("clock sysvar reported an invalid Unix timestamp")]
    BadClock = 17,

    #[msg("computed claim amount overflowed u64")]
    ClaimAmountOverflow = 18,

    #[msg("treasury PDA balance is insufficient for the requested claim")]
    InsufficientTreasuryBalance = 19,

    #[msg("total_allocation in args does not match the value committed to in the leaf")]
    TotalAllocationMismatch = 20,

    #[msg("attempted to re-init megadrop config — only one init is allowed")]
    AlreadyInitialized = 21,

    #[msg("init_megadrop args fail sanity (e.g. zero total allocation, zero genesis month)")]
    BadInitArgs = 22,

    #[msg("proof buffer PDA address does not match the expected derivation")]
    BadProofBufferPda = 23,

    #[msg("proof buffer write would overflow the declared total length")]
    ProofBufferOverflow = 24,

    #[msg("proof buffer payload was not fully written before claim_megadrop_from_buffer")]
    ProofBufferIncomplete = 25,

    #[msg("proof buffer header is malformed or has wrong discriminator")]
    BadProofBuffer = 26,

    #[msg("proof buffer total length disagrees with declared proof_len")]
    ProofBufferLengthMismatch = 27,

    #[msg("signer is not the configured ADMIN_AUTHORITY for this privileged ix")]
    Unauthorized = 28,
}

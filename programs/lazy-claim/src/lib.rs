//! Staccana lazy-claim program.
//!
//! Materializes a claimable account on staccana from the genesis Merkle root. A claimant
//! proves Merkle inclusion of their `(pubkey, lamports)` pair plus a fresh ed25519
//! signature with their mainnet keypair; the program credits lamports to the recipient
//! account and stamps a per-pubkey marker PDA so the same claim can never be replayed.
//!
//! See SPEC §4 for the normative wire format and verification order.
//!
//! ## Open TODOs
//!
//! * Validator-side privileged debit on the treasury PDA — see `processor::credit_lamports`
//!   for the genesis wiring requirement.
//! * Gas-exemption rule (SPEC §4.4) is a validator-side patch; intentionally not in this
//!   crate.
//!
//! ## Layout
//!
//! * [`error`]       — program-error variants
//! * [`instruction`] — `ClaimArgs` wire format and discriminator
//! * [`merkle`]      — `verify_inclusion` against the embedded `claimable_root`
//! * [`processor`]   — `process_claim` handler (the hot path)
//! * [`state`]       — `LazyClaimConfig` and `ClaimedMarker` account layouts

pub mod error;
pub mod instruction;
pub mod merkle;
pub mod processor;
pub mod state;

pub use error::LazyClaimError;
pub use instruction::{
    ClaimArgs, ClaimFromBufferArgs, InitProofBufferArgs, LazyClaimInstruction,
    WriteProofBufferArgs,
};
pub use processor::{build_claim_message, process_instruction, CLAIM_MESSAGE_PREFIX};
pub use state::{
    find_claimed_marker_pda, find_proof_buffer_pda, ClaimedMarker, LazyClaimConfig,
    ProofBufferHeader, CLAIMED_MARKER_SEED, PROOF_BUFFER_SEED,
};

#[cfg(all(target_os = "solana", feature = "bpf-entrypoint"))]
solana_program::entrypoint!(process_instruction);

/// Well-known program id placeholder. Replaced at deploy time via the standard
/// `declare_id!` mechanism in production; SPEC §2.1 lists `LAZY_CLAIM_PROGRAM_ID = TBD`.
///
/// MUST stay byte-equal to `staccana_claim_cli::tx::LAZY_CLAIM_PROGRAM_ID` until the real id
/// lands — otherwise the PDA derivations (`["claimed", pubkey]`) drift between the program
/// and its CLI client and silently produce different addresses.
///
/// The bytes are the ASCII string `LAZY_CLAIM_PROGRAM_PLACEHOLDER11` (32 bytes) — chosen so
/// a base58-decoded version is recognizable in logs and so it cannot be confused with the
/// System program (`[0; 32]`).
pub const fn id() -> solana_program::pubkey::Pubkey {
    solana_program::pubkey::Pubkey::new_from_array([
        b'L', b'A', b'Z', b'Y', b'_', b'C', b'L', b'A', b'I', b'M', b'_', b'P', b'R', b'O',
        b'G', b'R', b'A', b'M', b'_', b'P', b'L', b'A', b'C', b'E', b'H', b'O', b'L', b'D',
        b'E', b'R', b'1', b'1',
    ])
}

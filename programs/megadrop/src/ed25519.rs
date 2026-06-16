//! Ed25519 precompile inspection for holder-signed claim verification.
//!
//! Solana's `Ed25519SigVerify111111111111111111111111111111` precompile is the cheap
//! and correct way to verify ed25519 signatures from on-chain code: pile up the needed
//! signature as a preceding instruction in the same transaction, then read it back out
//! of the `Instructions` sysvar to confirm what was actually verified.
//!
//! Identical parsing logic to `staccana_bridge::ed25519` and
//! `staccana_validator_subsidy::ed25519`. The reason we duplicate rather than re-export:
//! keeping the megadrop crate's errors in its own namespace means clients that consume
//! the IDL get a clean `MegadropError::BadEd25519Precompile` rather than a cross-crate
//! `BridgeError::*`, which would muddle the IDL surface.
//!
//! Wire format reference:
//! <https://docs.solanalabs.com/runtime/programs#ed25519-program>

use crate::error::MegadropError;
use anchor_lang::prelude::*;
// Anchor 1.0 no longer re-exports the precompile / instructions-sysvar helpers.
// `solana-sdk-ids` carries the canonical pubkeys (ed25519_program, sysvar::instructions)
// and `solana-instructions-sysvar` carries the `load_*_checked` readers.
use solana_instructions_sysvar::load_instruction_at_checked;
use solana_sdk_ids::ed25519_program;
use solana_sdk_ids::sysvar::instructions::ID as INSTRUCTIONS_SYSVAR_ID;

/// Ed25519 precompile single-signature header layout (offsets in bytes within the
/// precompile ix's `data`).
const ED25519_HEADER_SIZE: usize = 16;
const ED25519_SIG_LEN: usize = 64;
const ED25519_PUBKEY_LEN: usize = 32;
/// `instruction_index == u16::MAX` means "this same instruction." We require all three
/// fields (sig, pubkey, message) to be inlined in the precompile ix's data.
const SAME_INSTRUCTION: u16 = u16::MAX;

/// Single signer + message + signature triple extracted from one ed25519 precompile ix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedEd25519 {
    pub pubkey: [u8; ED25519_PUBKEY_LEN],
    pub signature: [u8; ED25519_SIG_LEN],
    pub message: Vec<u8>,
}

/// Confirm the supplied account is the canonical Instructions sysvar.
pub fn require_instructions_sysvar(account: &AccountInfo) -> Result<()> {
    require_keys_eq!(
        *account.key,
        INSTRUCTIONS_SYSVAR_ID,
        MegadropError::BadInstructionsSysvar
    );
    Ok(())
}

/// Decode a single-signature ed25519 precompile ix from the Instructions sysvar at
/// `index`.
///
/// Rejects:
/// - any non-ed25519-precompile program
/// - `num_signatures != 1`
/// - signature/pubkey/message that aren't all `same_instruction` (we don't support
///   spreading the payload across multiple ixs — the holder MUST inline)
/// - any out-of-bounds offset within the precompile ix data
pub fn parse_ed25519_at(
    sysvar: &AccountInfo,
    index: usize,
) -> std::result::Result<ParsedEd25519, MegadropError> {
    let ix = load_instruction_at_checked(index, sysvar)
        .map_err(|_| MegadropError::BadEd25519Precompile)?;
    if ix.program_id != ed25519_program::ID {
        return Err(MegadropError::BadEd25519Precompile);
    }

    let data = ix.data.as_slice();
    if data.len() < ED25519_HEADER_SIZE {
        return Err(MegadropError::BadEd25519Precompile);
    }

    let num_signatures = data[0];
    if num_signatures != 1 {
        return Err(MegadropError::BadEd25519Precompile);
    }

    let _sig_offset = u16::from_le_bytes([data[2], data[3]]) as usize;
    let sig_ix_index = u16::from_le_bytes([data[4], data[5]]);
    let pk_offset = u16::from_le_bytes([data[6], data[7]]) as usize;
    let pk_ix_index = u16::from_le_bytes([data[8], data[9]]);
    let msg_offset = u16::from_le_bytes([data[10], data[11]]) as usize;
    let msg_size = u16::from_le_bytes([data[12], data[13]]) as usize;
    let msg_ix_index = u16::from_le_bytes([data[14], data[15]]);

    if sig_ix_index != SAME_INSTRUCTION
        || pk_ix_index != SAME_INSTRUCTION
        || msg_ix_index != SAME_INSTRUCTION
    {
        return Err(MegadropError::BadEd25519Precompile);
    }

    let sig_offset = u16::from_le_bytes([data[2], data[3]]) as usize;
    let sig_end = sig_offset
        .checked_add(ED25519_SIG_LEN)
        .ok_or(MegadropError::BadEd25519Precompile)?;
    let pk_end = pk_offset
        .checked_add(ED25519_PUBKEY_LEN)
        .ok_or(MegadropError::BadEd25519Precompile)?;
    let msg_end = msg_offset
        .checked_add(msg_size)
        .ok_or(MegadropError::BadEd25519Precompile)?;
    if sig_end > data.len() || pk_end > data.len() || msg_end > data.len() {
        return Err(MegadropError::BadEd25519Precompile);
    }

    let mut signature = [0u8; ED25519_SIG_LEN];
    signature.copy_from_slice(&data[sig_offset..sig_end]);
    let mut pubkey = [0u8; ED25519_PUBKEY_LEN];
    pubkey.copy_from_slice(&data[pk_offset..pk_end]);
    let message = data[msg_offset..msg_end].to_vec();

    Ok(ParsedEd25519 {
        pubkey,
        signature,
        message,
    })
}

//! Ed25519 precompile inspection for federation attestation verification.
//!
//! Mirrors `staccana_bridge::ed25519` (verbatim copy of the parsing logic). The reason
//! we duplicate rather than re-export from `staccana-bridge`: keeping the subsidy crate's
//! errors in its own namespace means clients that consume the IDL get a clean
//! `SubsidyError::BadEd25519Precompile` rather than a cross-crate `BridgeError::*`,
//! which would muddle the IDL surface.
//!
//! Wire format reference:
//! <https://docs.solanalabs.com/runtime/programs#ed25519-program>

use crate::error::SubsidyError;
use anchor_lang::prelude::*;
// Anchor 1.0 no longer re-exports the precompile / instructions-sysvar helpers we need.
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
        SubsidyError::BadInstructionsSysvar
    );
    Ok(())
}

/// Decode a single-signature ed25519 precompile ix from the Instructions sysvar at
/// `index`. Same semantics as the bridge's parser; see that module's docs for the full
/// rejection list.
pub fn parse_ed25519_at(
    sysvar: &AccountInfo,
    index: usize,
) -> std::result::Result<ParsedEd25519, SubsidyError> {
    let ix = load_instruction_at_checked(index, sysvar)
        .map_err(|_| SubsidyError::BadEd25519Precompile)?;
    if ix.program_id != ed25519_program::ID {
        return Err(SubsidyError::BadEd25519Precompile);
    }

    let data = ix.data.as_slice();
    if data.len() < ED25519_HEADER_SIZE {
        return Err(SubsidyError::BadEd25519Precompile);
    }

    let num_signatures = data[0];
    if num_signatures != 1 {
        return Err(SubsidyError::BadEd25519Precompile);
    }

    let sig_offset = u16::from_le_bytes([data[2], data[3]]) as usize;
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
        return Err(SubsidyError::BadEd25519Precompile);
    }

    let sig_end = sig_offset
        .checked_add(ED25519_SIG_LEN)
        .ok_or(SubsidyError::BadEd25519Precompile)?;
    let pk_end = pk_offset
        .checked_add(ED25519_PUBKEY_LEN)
        .ok_or(SubsidyError::BadEd25519Precompile)?;
    let msg_end = msg_offset
        .checked_add(msg_size)
        .ok_or(SubsidyError::BadEd25519Precompile)?;
    if sig_end > data.len() || pk_end > data.len() || msg_end > data.len() {
        return Err(SubsidyError::BadEd25519Precompile);
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

/// Per-signature offset record inside a batched ed25519 precompile ix.
const PER_SIG_OFFSETS_LEN: usize = 14;

/// Hard cap on the number of signatures a single precompile ix may carry.
/// Matches `state::MAX_FEDERATION_MEMBERS` — federation can't be larger
/// than this, so it's a defensive ceiling.
pub const MAX_BATCH_SIGS: usize = crate::state::MAX_FEDERATION_MEMBERS;

/// Decode a batched ed25519 precompile ix from the Instructions sysvar at
/// `index`. Mirrors `staccana_bridge::ed25519::parse_ed25519_batch_at`.
///
/// Unlike [`parse_ed25519_at`], accepts `num_signatures` in `[1,
/// MAX_BATCH_SIGS]` and returns one [`ParsedEd25519`] per signature. Each
/// entry's `(sig, pubkey, message)` MUST live within this same ix's data
/// (cross-ix references rejected).
///
/// Used by `update_validator_metrics` when it walks back ONE precompile
/// ix carrying M sigs over a shared message — saves ~75 × (M-1) bytes
/// of tx size vs. M individual precompile ixs, which matters at M=5
/// (single-sig path overflows the 1232-byte tx limit at M=5).
pub fn parse_ed25519_batch_at(
    sysvar: &AccountInfo,
    index: usize,
) -> std::result::Result<Vec<ParsedEd25519>, SubsidyError> {
    let ix = load_instruction_at_checked(index, sysvar)
        .map_err(|_| SubsidyError::BadEd25519Precompile)?;
    if ix.program_id != ed25519_program::ID {
        return Err(SubsidyError::BadEd25519Precompile);
    }

    let data = ix.data.as_slice();
    if data.len() < 2 {
        return Err(SubsidyError::BadEd25519Precompile);
    }

    let num_signatures = data[0] as usize;
    if num_signatures == 0 || num_signatures > MAX_BATCH_SIGS {
        return Err(SubsidyError::BadEd25519Precompile);
    }

    let offsets_end = 2usize
        .checked_add(
            num_signatures
                .checked_mul(PER_SIG_OFFSETS_LEN)
                .ok_or(SubsidyError::BadEd25519Precompile)?,
        )
        .ok_or(SubsidyError::BadEd25519Precompile)?;
    if data.len() < offsets_end {
        return Err(SubsidyError::BadEd25519Precompile);
    }

    let mut out = Vec::with_capacity(num_signatures);
    for i in 0..num_signatures {
        let base = 2 + i * PER_SIG_OFFSETS_LEN;
        let sig_offset = u16::from_le_bytes([data[base], data[base + 1]]) as usize;
        let sig_ix_index = u16::from_le_bytes([data[base + 2], data[base + 3]]);
        let pk_offset = u16::from_le_bytes([data[base + 4], data[base + 5]]) as usize;
        let pk_ix_index = u16::from_le_bytes([data[base + 6], data[base + 7]]);
        let msg_offset = u16::from_le_bytes([data[base + 8], data[base + 9]]) as usize;
        let msg_size = u16::from_le_bytes([data[base + 10], data[base + 11]]) as usize;
        let msg_ix_index = u16::from_le_bytes([data[base + 12], data[base + 13]]);

        if sig_ix_index != SAME_INSTRUCTION
            || pk_ix_index != SAME_INSTRUCTION
            || msg_ix_index != SAME_INSTRUCTION
        {
            return Err(SubsidyError::BadEd25519Precompile);
        }

        let sig_end = sig_offset
            .checked_add(ED25519_SIG_LEN)
            .ok_or(SubsidyError::BadEd25519Precompile)?;
        let pk_end = pk_offset
            .checked_add(ED25519_PUBKEY_LEN)
            .ok_or(SubsidyError::BadEd25519Precompile)?;
        let msg_end = msg_offset
            .checked_add(msg_size)
            .ok_or(SubsidyError::BadEd25519Precompile)?;
        if sig_end > data.len() || pk_end > data.len() || msg_end > data.len() {
            return Err(SubsidyError::BadEd25519Precompile);
        }

        let mut signature = [0u8; ED25519_SIG_LEN];
        signature.copy_from_slice(&data[sig_offset..sig_end]);
        let mut pubkey = [0u8; ED25519_PUBKEY_LEN];
        pubkey.copy_from_slice(&data[pk_offset..pk_end]);
        let message = data[msg_offset..msg_end].to_vec();

        out.push(ParsedEd25519 {
            pubkey,
            signature,
            message,
        });
    }
    Ok(out)
}

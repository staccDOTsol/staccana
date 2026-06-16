//! Ed25519 precompile inspection for federation attestation verification.
//!
//! Solana's `Ed25519SigVerify111111111111111111111111111111` precompile is the cheap and
//! correct way to verify ed25519 signatures from on-chain code: pile up the necessary
//! signatures as preceding instructions in the same transaction, then read them back
//! out of the `Instructions` sysvar to confirm what was actually verified.
//!
//! This module mirrors the lazy-claim precompile-reading pattern (see
//! `programs/lazy-claim/src/...`). It is intentionally kept dependency-free of
//! `ed25519-dalek` — runtime cost matters and the precompile is already on-chain.
//!
//! Wire format reference:
//! <https://docs.solanalabs.com/runtime/programs#ed25519-program>
//!
//! Each precompile ix can carry multiple signatures via `num_signatures`. v1 of the
//! bridge accepts both:
//!   - the single-signature path ([`parse_ed25519_at`], kept for back-compat)
//!   - a batched path ([`parse_ed25519_batch_at`]) that pulls M sigs out of one ix
//!
//! Why batched: stacking M one-sig precompile ixs duplicates the 68-byte message
//! preimage M times and pads the tx with M sets of offsets/headers. A 5-sig
//! federation packs into ~620 bytes batched vs ~900 bytes split, which keeps the
//! total tx (mint ix + signatures + accounts) comfortably under the 1232-byte
//! legacy ceiling without needing a v0 + LUT tx.

use crate::error::BridgeError;
use anchor_lang::prelude::*;
// Anchor 1.0 no longer re-exports the precompile / instructions-sysvar helpers we need.
// `solana-sdk-ids` carries the canonical pubkeys (ed25519_program, sysvar::instructions)
// and `solana-instructions-sysvar` carries the `load_*_checked` readers.
use solana_instructions_sysvar::load_instruction_at_checked;
use solana_sdk_ids::ed25519_program;
use solana_sdk_ids::sysvar::instructions::ID as INSTRUCTIONS_SYSVAR_ID;

/// Solana ed25519 precompile single-signature header layout (from the runtime spec):
///
/// | offset | size | field                       |
/// |--------|------|-----------------------------|
/// | 0      | 1    | num_signatures              |
/// | 1      | 1    | padding                     |
/// | 2      | 2    | signature_offset (u16 LE)   |
/// | 4      | 2    | signature_instruction_index |
/// | 6      | 2    | public_key_offset           |
/// | 8      | 2    | public_key_instruction_index|
/// | 10     | 2    | message_data_offset         |
/// | 12     | 2    | message_data_size           |
/// | 14     | 2    | message_instruction_index   |
const ED25519_HEADER_SIZE: usize = 16;
const ED25519_SIG_LEN: usize = 64;
const ED25519_PUBKEY_LEN: usize = 32;
/// `instruction_index == u16::MAX` means "this same instruction" — required for our
/// usage where the pubkey, signature, and message are all packed into the precompile
/// ix data itself rather than scattered across other ixs.
const SAME_INSTRUCTION: u16 = u16::MAX;

/// Single signer + message + signature triple extracted from one ed25519 precompile ix.
///
/// All three are slices into the precompile ix's data; collecting them into owned data
/// lets the caller compare against expected values without re-parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedEd25519 {
    pub pubkey: [u8; ED25519_PUBKEY_LEN],
    pub signature: [u8; ED25519_SIG_LEN],
    pub message: Vec<u8>,
}

/// Confirm the supplied account is the canonical Instructions sysvar. Cheap pubkey
/// comparison; cheaper than letting `load_instruction_at_checked` throw.
pub fn require_instructions_sysvar(account: &AccountInfo) -> Result<()> {
    require_keys_eq!(
        *account.key,
        INSTRUCTIONS_SYSVAR_ID,
        BridgeError::BadInstructionsSysvar
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
///   spreading the payload across multiple ixs — the federation MUST inline)
/// - any out-of-bounds offset within the precompile ix data
pub fn parse_ed25519_at(
    sysvar: &AccountInfo,
    index: usize,
) -> std::result::Result<ParsedEd25519, BridgeError> {
    let ix = load_instruction_at_checked(index, sysvar)
        .map_err(|_| BridgeError::BadEd25519Precompile)?;
    if ix.program_id != ed25519_program::ID {
        return Err(BridgeError::BadEd25519Precompile);
    }

    let data = ix.data.as_slice();
    if data.len() < ED25519_HEADER_SIZE {
        return Err(BridgeError::BadEd25519Precompile);
    }

    let num_signatures = data[0];
    if num_signatures != 1 {
        return Err(BridgeError::BadEd25519Precompile);
    }

    // Bytes 2..16 are five u16-pairs of (offset, instruction_index). Read them strictly
    // to catch any malformed precompile ix without panicking.
    let sig_offset = u16::from_le_bytes([data[2], data[3]]) as usize;
    let sig_ix_index = u16::from_le_bytes([data[4], data[5]]);
    let pk_offset = u16::from_le_bytes([data[6], data[7]]) as usize;
    let pk_ix_index = u16::from_le_bytes([data[8], data[9]]);
    let msg_offset = u16::from_le_bytes([data[10], data[11]]) as usize;
    let msg_size = u16::from_le_bytes([data[12], data[13]]) as usize;
    let msg_ix_index = u16::from_le_bytes([data[14], data[15]]);

    // Demand all three pieces live inside this ix. Cross-ix referencing is a feature
    // of the precompile but a foot-gun for verifiers — every attestation we accept
    // is self-contained.
    if sig_ix_index != SAME_INSTRUCTION
        || pk_ix_index != SAME_INSTRUCTION
        || msg_ix_index != SAME_INSTRUCTION
    {
        return Err(BridgeError::BadEd25519Precompile);
    }

    // Bounds-check each field before slicing.
    let sig_end = sig_offset
        .checked_add(ED25519_SIG_LEN)
        .ok_or(BridgeError::BadEd25519Precompile)?;
    let pk_end = pk_offset
        .checked_add(ED25519_PUBKEY_LEN)
        .ok_or(BridgeError::BadEd25519Precompile)?;
    let msg_end = msg_offset
        .checked_add(msg_size)
        .ok_or(BridgeError::BadEd25519Precompile)?;
    if sig_end > data.len() || pk_end > data.len() || msg_end > data.len() {
        return Err(BridgeError::BadEd25519Precompile);
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
/// Hard cap on the number of signatures a single precompile ix may carry. Mirrors
/// `state::MAX_FEDERATION_MEMBERS` — the on-chain federation can't physically be
/// larger than this so it's a tight, defensive ceiling.
pub const MAX_BATCH_SIGS: usize = crate::state::MAX_FEDERATION_MEMBERS;

/// Decode a batched ed25519 precompile ix from the Instructions sysvar at `index`.
///
/// Unlike [`parse_ed25519_at`], this accepts `num_signatures` between 1 and
/// `MAX_BATCH_SIGS` and returns one [`ParsedEd25519`] per signature. Each entry's
/// `(sig, pubkey, message)` MUST live within this same ix's data — cross-ix
/// references stay rejected.
///
/// For homogeneous batches (every sig signs the same message), the message bytes
/// in the returned entries are clones of the same range. The caller can compare
/// them all to the expected preimage cheaply with `==`.
pub fn parse_ed25519_batch_at(
    sysvar: &AccountInfo,
    index: usize,
) -> std::result::Result<Vec<ParsedEd25519>, BridgeError> {
    let ix = load_instruction_at_checked(index, sysvar)
        .map_err(|_| BridgeError::BadEd25519Precompile)?;
    if ix.program_id != ed25519_program::ID {
        return Err(BridgeError::BadEd25519Precompile);
    }

    let data = ix.data.as_slice();
    if data.len() < 2 {
        return Err(BridgeError::BadEd25519Precompile);
    }

    let num_signatures = data[0] as usize;
    if num_signatures == 0 || num_signatures > MAX_BATCH_SIGS {
        return Err(BridgeError::BadEd25519Precompile);
    }

    // Header (2) + N × 14-byte offset records must all fit in `data`.
    let offsets_end = 2usize
        .checked_add(num_signatures.checked_mul(PER_SIG_OFFSETS_LEN).ok_or(BridgeError::BadEd25519Precompile)?)
        .ok_or(BridgeError::BadEd25519Precompile)?;
    if data.len() < offsets_end {
        return Err(BridgeError::BadEd25519Precompile);
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
            return Err(BridgeError::BadEd25519Precompile);
        }

        let sig_end = sig_offset
            .checked_add(ED25519_SIG_LEN)
            .ok_or(BridgeError::BadEd25519Precompile)?;
        let pk_end = pk_offset
            .checked_add(ED25519_PUBKEY_LEN)
            .ok_or(BridgeError::BadEd25519Precompile)?;
        let msg_end = msg_offset
            .checked_add(msg_size)
            .ok_or(BridgeError::BadEd25519Precompile)?;
        if sig_end > data.len() || pk_end > data.len() || msg_end > data.len() {
            return Err(BridgeError::BadEd25519Precompile);
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

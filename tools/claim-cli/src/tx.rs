//! Transaction encoding for the lazy-claim flow.
//!
//! Two instructions are constructed here:
//!
//! 1. The **ed25519 precompile** instruction — the Solana built-in at program ID
//!    `Ed25519SigVerify111111111111111111111111111` that verifies an ed25519 signature
//!    inline within transaction execution. The lazy-claim program inspects this ix via
//!    the `Instructions` sysvar (per `docs/SPEC.md` §4.3 step 4) to confirm the user
//!    actually signed the claim message with their mainnet keypair.
//!
//! 2. The **claim** instruction targeting the lazy-claim program, carrying the borsh-encoded
//!    [`ClaimArgs`] per `docs/SPEC.md` §4.1.
//!
//! The signed message format (`docs/SPEC.md` §4.2):
//!
//! ```text
//! msg = "STACCANA_CLAIM_V1"
//!    || pubkey                          (32 bytes)
//!    || lamports.to_le_bytes()          ( 8 bytes)
//!    || LAZY_CLAIM_PROGRAM_ID           (32 bytes)
//! ```
//!
//! The claim instruction layout matches `docs/SPEC.md` §4.1:
//!
//! ```text
//! struct ClaimArgs {
//!     pubkey:       [u8; 32],
//!     lamports:     u64,
//!     proof_len:    u16,
//!     proof:        [[u8; 32]; proof_len],
//!     proof_flags:  [u8; (proof_len + 7) / 8],
//! }
//! ```
//!
//! We use borsh for the serialization of the variable-length fields. Borsh encodes the
//! `Vec<[u8; 32]>` and `Vec<u8>` as `u32` length prefix + raw bytes. The spec's literal
//! `proof_len: u16` is preserved as a separate u16 field so that the wire format reads
//! naturally and matches the spec.

use std::convert::TryFrom;

use borsh::BorshSerialize;
use solana_program::hash::Hash;
use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;
use solana_program::sysvar::instructions as sysvar_instructions;
use solana_sdk::ed25519_program;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::system_program;

/// Domain string for the claim message — matches `docs/SPEC.md` §4.2.
pub const STACCANA_CLAIM_DOMAIN: &[u8] = b"STACCANA_CLAIM_V1";

/// Placeholder program ID for the lazy-claim program.
///
/// TODO: replace this constant with the real on-chain program ID once it is assigned at
/// genesis. The CLI will need a release where this constant is updated; downstream tests in
/// this crate hard-code the placeholder so they do not break in the meantime.
///
/// The bytes here come from the ASCII string `LAZY_CLAIM_PROGRAM_PLACEHOLDER111` (32 bytes
/// — chosen so a base58-decoded version is recognizable in logs).
pub const LAZY_CLAIM_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    b'L', b'A', b'Z', b'Y', b'_', b'C', b'L', b'A', b'I', b'M', b'_', b'P', b'R', b'O', b'G', b'R',
    b'A', b'M', b'_', b'P', b'L', b'A', b'C', b'E', b'H', b'O', b'L', b'D', b'E', b'R', b'1', b'1',
]);

/// `ClaimArgs` per `docs/SPEC.md` §4.1.
///
/// Borsh's default `Vec<T>` serialization writes `u32` length, but the spec calls for
/// `proof_len: u16` so we serialize `proof_len` explicitly as a `u16` and write the proof
/// bytes/flag bytes as raw runs. The `BorshSerialize` impl does the manual layout to keep
/// it byte-exact against the spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimArgs {
    pub pubkey: [u8; 32],
    pub lamports: u64,
    pub proof: Vec<[u8; 32]>,
    /// Packed bit flags. Length must equal `(proof.len() + 7) / 8`.
    pub proof_flags: Vec<u8>,
}

impl ClaimArgs {
    pub fn new(pubkey: Pubkey, lamports: u64, proof: Vec<Hash>, proof_flags: Vec<u8>) -> Self {
        let proof_arrays: Vec<[u8; 32]> = proof.iter().map(|h| h.to_bytes()).collect();
        Self {
            pubkey: pubkey.to_bytes(),
            lamports,
            proof: proof_arrays,
            proof_flags,
        }
    }

    /// Serialize the args using the on-chain wire format from `docs/SPEC.md` §4.1.
    ///
    /// Wire layout (concatenated):
    ///
    /// 1. `pubkey` — 32 bytes
    /// 2. `lamports` — 8 bytes LE
    /// 3. `proof_len` — 2 bytes LE
    /// 4. `proof` — `32 * proof_len` bytes
    /// 5. `proof_flags` — `(proof_len + 7) / 8` bytes
    pub fn to_wire_bytes(&self) -> std::io::Result<Vec<u8>> {
        let proof_len = u16::try_from(self.proof.len()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "proof_len does not fit in u16",
            )
        })?;
        let expected_flag_bytes = (self.proof.len() + 7) / 8;
        if self.proof_flags.len() != expected_flag_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "proof_flags length does not match proof_len",
            ));
        }

        let mut out =
            Vec::with_capacity(32 + 8 + 2 + (32 * self.proof.len()) + self.proof_flags.len());
        out.extend_from_slice(&self.pubkey);
        out.extend_from_slice(&self.lamports.to_le_bytes());
        out.extend_from_slice(&proof_len.to_le_bytes());
        for hash in &self.proof {
            out.extend_from_slice(hash);
        }
        out.extend_from_slice(&self.proof_flags);
        Ok(out)
    }
}

// Borsh impl mirrors `to_wire_bytes` exactly so consumers that prefer borsh's trait
// machinery get identical bytes. `borsh::io` is `std::io` when the (default) `std` feature
// is on, so the `Write` / `Result` types here line up with the trait signature.
impl BorshSerialize for ClaimArgs {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        let bytes = self.to_wire_bytes()?;
        writer.write_all(&bytes)
    }
}

/// Build the message that the user's mainnet keypair must sign for the claim.
///
/// Spec §4.2: `"STACCANA_CLAIM_V1" || pubkey || lamports.to_le_bytes() || LAZY_CLAIM_PROGRAM_ID`.
pub fn build_claim_message(pubkey: &Pubkey, lamports: u64) -> Vec<u8> {
    let mut msg = Vec::with_capacity(STACCANA_CLAIM_DOMAIN.len() + 32 + 8 + 32);
    msg.extend_from_slice(STACCANA_CLAIM_DOMAIN);
    msg.extend_from_slice(pubkey.as_ref());
    msg.extend_from_slice(&lamports.to_le_bytes());
    msg.extend_from_slice(LAZY_CLAIM_PROGRAM_ID.as_ref());
    msg
}

/// Layout constants for the ed25519 precompile instruction data.
///
/// Mirrors `solana-ed25519-program`'s constants — duplicated here because the upstream
/// `new_ed25519_instruction` helper requires an `ed25519_dalek::Keypair`, not a Solana
/// `Keypair`, and we don't want to take a dep on `ed25519-dalek` just for the constructor.
/// Instead we sign with the Solana keypair (which already wraps `ed25519-dalek`) and lay
/// out the precompile data ourselves.
const ED25519_PUBKEY_SIZE: usize = 32;
const ED25519_SIGNATURE_SIZE: usize = 64;
const ED25519_OFFSETS_SIZE: usize = 14;
const ED25519_OFFSETS_START: usize = 2;
const ED25519_DATA_START: usize = ED25519_OFFSETS_SIZE + ED25519_OFFSETS_START;

/// Construct the ed25519 precompile instruction that signs `message` with `keypair`.
///
/// Wire format produced (matches `solana-ed25519-program::new_ed25519_instruction`):
///
/// ```text
/// [num_signatures: u8 (=1)] [padding: u8] [offsets: 14 bytes] [pubkey: 32] [signature: 64] [message: variable]
/// ```
///
/// The lazy-claim program reads this back via the `Instructions` sysvar (`docs/SPEC.md`
/// §4.3 step 4) to confirm the user signed the claim message with their mainnet keypair.
pub fn build_ed25519_precompile_instruction(keypair: &Keypair, message: &[u8]) -> Instruction {
    let pubkey_bytes = keypair.pubkey().to_bytes();
    let signature = keypair.sign_message(message);
    let signature_bytes = signature.as_ref();
    assert_eq!(pubkey_bytes.len(), ED25519_PUBKEY_SIZE);
    assert_eq!(signature_bytes.len(), ED25519_SIGNATURE_SIZE);

    let public_key_offset = ED25519_DATA_START;
    let signature_offset = public_key_offset + ED25519_PUBKEY_SIZE;
    let message_data_offset = signature_offset + ED25519_SIGNATURE_SIZE;

    let mut data = Vec::with_capacity(message_data_offset + message.len());
    // [num_signatures, padding]
    data.push(1u8);
    data.push(0u8);
    // offsets struct — 14 bytes, little-endian fields per the precompile layout
    data.extend_from_slice(&(signature_offset as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // signature_instruction_index = self
    data.extend_from_slice(&(public_key_offset as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // public_key_instruction_index = self
    data.extend_from_slice(&(message_data_offset as u16).to_le_bytes());
    data.extend_from_slice(&(message.len() as u16).to_le_bytes());
    data.extend_from_slice(&u16::MAX.to_le_bytes()); // message_instruction_index = self
    debug_assert_eq!(data.len(), public_key_offset);

    data.extend_from_slice(&pubkey_bytes);
    debug_assert_eq!(data.len(), signature_offset);

    data.extend_from_slice(signature_bytes);
    debug_assert_eq!(data.len(), message_data_offset);

    data.extend_from_slice(message);

    Instruction {
        program_id: ed25519_program::ID,
        accounts: vec![],
        data,
    }
}

/// Build the lazy-claim `claim` instruction.
///
/// Account ordering matches `docs/SPEC.md` §4.1:
///
/// | # | Role                  | Description |
/// |---|-----------------------|-------------|
/// | 0 | `[writable]`          | Recipient (system-owned, will be created if absent). Pubkey == claimed pubkey. |
/// | 1 | `[]`                  | Lazy-claim program state account holding embedded `claimable_root`. |
/// | 2 | `[]`                  | Sysvar `Instructions`. |
/// | 3 | `[writable]`          | Treasury PDA (gas sponsor source). |
/// | 4 | `[writable]`          | Per-pubkey claimed-marker PDA `["claimed", pubkey]`. |
/// | 5 | `[writable, signer]`  | Fee payer for the marker PDA's rent-exempt allocation. |
/// | 6 | `[]`                  | System program (used by the marker-init CPI). |
pub fn build_claim_instruction(
    args: &ClaimArgs,
    program_state: Pubkey,
    treasury_pda: Pubkey,
    claimed_marker_pda: Pubkey,
    payer: Pubkey,
) -> std::io::Result<Instruction> {
    let recipient = Pubkey::new_from_array(args.pubkey);
    let body = args.to_wire_bytes()?;
    let mut data = Vec::with_capacity(1 + body.len());
    data.push(0x00);
    data.extend_from_slice(&body);
    let accounts = vec![
        AccountMeta::new(recipient, false),
        AccountMeta::new_readonly(program_state, false),
        AccountMeta::new_readonly(sysvar_instructions::ID, false),
        AccountMeta::new(treasury_pda, false),
        AccountMeta::new(claimed_marker_pda, false),
        AccountMeta::new(payer, true),
        AccountMeta::new_readonly(system_program::ID, false),
    ];
    Ok(Instruction {
        program_id: LAZY_CLAIM_PROGRAM_ID,
        accounts,
        data,
    })
}

/// Derive the per-pubkey claimed-marker PDA at `["claimed", pubkey]`. Matches the spec's
/// description of the PDA passed at account index 4 of the `claim` ix.
pub fn claimed_marker_pda(pubkey: &Pubkey) -> Pubkey {
    let (pda, _bump) =
        Pubkey::find_program_address(&[b"claimed", pubkey.as_ref()], &LAZY_CLAIM_PROGRAM_ID);
    pda
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn claim_message_matches_spec_layout() {
        let pubkey = pk(7);
        let lamports = 0x0123_4567_89ab_cdefu64;

        let msg = build_claim_message(&pubkey, lamports);

        // 17 bytes domain + 32 pubkey + 8 lamports + 32 program id = 89 bytes
        assert_eq!(msg.len(), 17 + 32 + 8 + 32);

        // domain
        assert_eq!(&msg[0..17], b"STACCANA_CLAIM_V1");
        // pubkey
        assert_eq!(&msg[17..49], &[7u8; 32]);
        // lamports LE
        assert_eq!(&msg[49..57], &lamports.to_le_bytes());
        // program id
        assert_eq!(&msg[57..89], LAZY_CLAIM_PROGRAM_ID.as_ref());
    }

    #[test]
    fn claim_message_is_byte_exact() {
        // Locking the message construction to a known sequence so any future drift in field
        // ordering or domain content is caught immediately.
        let pubkey = Pubkey::new_from_array([0u8; 32]);
        let lamports = 1u64;

        let mut expected = Vec::new();
        expected.extend_from_slice(b"STACCANA_CLAIM_V1");
        expected.extend_from_slice(&[0u8; 32]);
        expected.extend_from_slice(&1u64.to_le_bytes());
        expected.extend_from_slice(LAZY_CLAIM_PROGRAM_ID.as_ref());

        assert_eq!(build_claim_message(&pubkey, lamports), expected);
    }

    #[test]
    fn claim_args_wire_bytes_layout() {
        let args = ClaimArgs {
            pubkey: [0xAB; 32],
            lamports: 0x1122_3344_5566_7788,
            proof: vec![[0x11; 32], [0x22; 32], [0x33; 32]],
            proof_flags: vec![0b0000_0101], // 3 bits ⇒ 1 byte
        };
        let wire = args.to_wire_bytes().expect("wire");

        // 32 + 8 + 2 + (32 * 3) + 1 = 139
        assert_eq!(wire.len(), 32 + 8 + 2 + 32 * 3 + 1);

        assert_eq!(&wire[0..32], &[0xABu8; 32]);
        assert_eq!(&wire[32..40], &args.lamports.to_le_bytes());
        assert_eq!(&wire[40..42], &3u16.to_le_bytes());
        assert_eq!(&wire[42..74], &[0x11u8; 32]);
        assert_eq!(&wire[74..106], &[0x22u8; 32]);
        assert_eq!(&wire[106..138], &[0x33u8; 32]);
        assert_eq!(wire[138], 0b0000_0101);
    }

    #[test]
    fn claim_args_borsh_serialize_matches_wire() {
        let args = ClaimArgs {
            pubkey: [0x01; 32],
            lamports: 1234,
            proof: vec![[0xAA; 32], [0xBB; 32]],
            proof_flags: vec![0b0000_0010],
        };
        let wire = args.to_wire_bytes().expect("wire");
        let borsh_bytes = borsh::to_vec(&args).expect("borsh");
        assert_eq!(wire, borsh_bytes);
    }

    #[test]
    fn claim_args_rejects_mismatched_flag_length() {
        // 3 proof entries ⇒ ceil(3/8) = 1 flag byte expected. Provide 2 — error.
        let args = ClaimArgs {
            pubkey: [0u8; 32],
            lamports: 0,
            proof: vec![[0u8; 32]; 3],
            proof_flags: vec![0u8, 0u8],
        };
        assert!(args.to_wire_bytes().is_err());
    }

    #[test]
    fn claim_args_zero_proof_serialization() {
        // Pubkey-only tree (single-leaf root, e.g.) ⇒ proof_len 0, no flag bytes.
        let args = ClaimArgs {
            pubkey: [0xFE; 32],
            lamports: 7,
            proof: vec![],
            proof_flags: vec![],
        };
        let wire = args.to_wire_bytes().expect("wire");
        assert_eq!(wire.len(), 32 + 8 + 2);
        assert_eq!(&wire[0..32], &[0xFEu8; 32]);
        assert_eq!(&wire[32..40], &7u64.to_le_bytes());
        assert_eq!(&wire[40..42], &0u16.to_le_bytes());
    }

    #[test]
    fn claim_instruction_account_layout_matches_spec() {
        let args = ClaimArgs {
            pubkey: pk(1).to_bytes(),
            lamports: 100,
            proof: vec![],
            proof_flags: vec![],
        };
        let program_state = pk(2);
        let treasury_pda = pk(3);
        let claimed_marker = pk(4);
        let payer = pk(5);
        let ix = build_claim_instruction(&args, program_state, treasury_pda, claimed_marker, payer)
            .expect("ix");

        assert_eq!(ix.program_id, LAZY_CLAIM_PROGRAM_ID);
        assert_eq!(ix.data[0], 0x00);
        assert_eq!(&ix.data[1..], args.to_wire_bytes().unwrap().as_slice());
        // SPEC §4.1 (post-discovery in e2e harness): recipient, config, sysvar, treasury,
        // marker, payer, system_program.
        assert_eq!(ix.accounts.len(), 7);

        // 0: recipient, writable, not signer
        assert_eq!(ix.accounts[0].pubkey, pk(1));
        assert!(ix.accounts[0].is_writable);
        assert!(!ix.accounts[0].is_signer);
        // 1: program state, readonly
        assert_eq!(ix.accounts[1].pubkey, program_state);
        assert!(!ix.accounts[1].is_writable);
        // 2: sysvar instructions
        assert_eq!(ix.accounts[2].pubkey, sysvar_instructions::ID);
        assert!(!ix.accounts[2].is_writable);
        // 3: treasury PDA, writable
        assert_eq!(ix.accounts[3].pubkey, treasury_pda);
        assert!(ix.accounts[3].is_writable);
        // 4: claimed marker PDA, writable
        assert_eq!(ix.accounts[4].pubkey, claimed_marker);
        assert!(ix.accounts[4].is_writable);
        // 5: payer, writable + signer (covers marker PDA's rent-exempt allocation)
        assert_eq!(ix.accounts[5].pubkey, payer);
        assert!(ix.accounts[5].is_writable);
        assert!(ix.accounts[5].is_signer);
        // 6: system program, readonly (used by the marker-init CPI)
        assert_eq!(ix.accounts[6].pubkey, system_program::ID);
        assert!(!ix.accounts[6].is_writable);
    }

    #[test]
    fn claimed_marker_pda_is_derived_from_pubkey() {
        let p = pk(7);
        let pda1 = claimed_marker_pda(&p);
        let pda2 = claimed_marker_pda(&p);
        // Deterministic.
        assert_eq!(pda1, pda2);
        // Different pubkey ⇒ different PDA.
        assert_ne!(claimed_marker_pda(&pk(7)), claimed_marker_pda(&pk(8)));
    }

    #[test]
    fn ed25519_precompile_instruction_uses_builtin_program_id() {
        let keypair = Keypair::new();
        let message = build_claim_message(&keypair.pubkey(), 42);
        let ix = build_ed25519_precompile_instruction(&keypair, &message);
        // The ed25519 precompile lives at this well-known program ID.
        assert_eq!(
            ix.program_id,
            solana_sdk::ed25519_program::ID,
            "precompile ix should target the ed25519 program",
        );
        // No accounts on a precompile ix.
        assert!(ix.accounts.is_empty());
    }

    #[test]
    fn ed25519_precompile_data_layout() {
        // Verify the precompile data layout byte-for-byte against the offsets we declare:
        //   [num_signatures=1][padding=0][offsets: 14 bytes][pubkey: 32][signature: 64][message]
        let keypair = Keypair::new();
        let message = build_claim_message(&keypair.pubkey(), 42);
        let ix = build_ed25519_precompile_instruction(&keypair, &message);

        let public_key_offset = ED25519_DATA_START;
        let signature_offset = public_key_offset + ED25519_PUBKEY_SIZE;
        let message_data_offset = signature_offset + ED25519_SIGNATURE_SIZE;

        assert_eq!(
            ix.data.len(),
            message_data_offset + message.len(),
            "total data length"
        );
        assert_eq!(ix.data[0], 1, "num_signatures");
        assert_eq!(ix.data[1], 0, "padding");

        // signature_offset (LE u16)
        assert_eq!(
            u16::from_le_bytes([ix.data[2], ix.data[3]]) as usize,
            signature_offset,
        );
        // public_key_offset (LE u16) — at field index 2 of offsets struct (bytes 6..8)
        assert_eq!(
            u16::from_le_bytes([ix.data[6], ix.data[7]]) as usize,
            public_key_offset,
        );
        // message_data_offset — bytes 10..12
        assert_eq!(
            u16::from_le_bytes([ix.data[10], ix.data[11]]) as usize,
            message_data_offset,
        );
        // message_data_size — bytes 12..14
        assert_eq!(
            u16::from_le_bytes([ix.data[12], ix.data[13]]) as usize,
            message.len(),
        );

        // pubkey
        assert_eq!(
            &ix.data[public_key_offset..public_key_offset + 32],
            keypair.pubkey().as_ref(),
        );
        // message
        assert_eq!(&ix.data[message_data_offset..], message.as_slice());
    }
}

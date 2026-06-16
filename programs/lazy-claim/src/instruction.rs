//! Wire format for the lazy-claim instruction.
//!
//! One instruction (`Claim`), one discriminator byte. Instruction data layout matches
//! SPEC §4.1 exactly:
//!
//! ```text
//! [0..1]                       ix discriminator (0x00 = Claim)
//! [1..33]                      pubkey (32 bytes)
//! [33..41]                     lamports (LE u64)
//! [41..43]                     proof_len (LE u16)
//! [43..43 + proof_len*32]      proof: [[u8; 32]; proof_len]
//! [43 + proof_len*32 ..]       proof_flags: [u8; ceil(proof_len / 8)]
//! ```
//!
//! Borsh is on the dependency list per the workspace plan, but this serializer is hand-
//! rolled for two reasons: (1) the format is fixed and trivial, (2) on-chain programs
//! benefit from avoiding borsh's small per-call overhead. The encoding is bit-compatible
//! with what borsh would produce for the equivalent struct.

use solana_program::program_error::ProgramError;

use crate::error::LazyClaimError;

/// Instruction discriminator — first byte of every ix data payload bound for this program.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LazyClaimInstruction {
    /// Single-tx claim. Inline proof in ix data — fits trees up to ~17 levels deep.
    Claim = 0x00,
    /// Allocate a proof-buffer PDA for staging long proofs across multiple txs.
    InitProofBuffer = 0x01,
    /// Append bytes to a previously initialized proof-buffer PDA.
    WriteProofBuffer = 0x02,
    /// Final claim ix that reads the proof from a staged proof-buffer PDA, runs the
    /// existing claim flow, then closes the buffer (rent → payer).
    ClaimFromBuffer = 0x03,
    /// Privileged: debit lamports from the lazy-claim-owned treasury PDA and
    /// credit them to a recipient. Used as one leg of the multi-ix tx that
    /// fixes the genesis-bake treasury custody bug — see the validator-
    /// subsidy program's `migrate_treasury_owner` for the full dance.
    /// Gated on `ADMIN_AUTHORITY` signer. Wire format: discriminator (0x04)
    /// + amount (LE u64).
    DrainTreasury = 0x04,
    /// Privileged: directly reassign the treasury PDA's owner from this
    /// program to a target program ID. Allowed because Solana lets an
    /// owner-program change `owner` on accounts it owns when `data.len()
    /// == 0`, which is true for the treasury (zero-data PDA). Gated on
    /// `ADMIN_AUTHORITY`. Wire format: discriminator (0x05) + new_owner
    /// (32-byte Pubkey).
    AssignTreasuryOwner = 0x05,
}

impl LazyClaimInstruction {
    pub fn from_byte(b: u8) -> Result<Self, ProgramError> {
        match b {
            0x00 => Ok(Self::Claim),
            0x01 => Ok(Self::InitProofBuffer),
            0x02 => Ok(Self::WriteProofBuffer),
            0x03 => Ok(Self::ClaimFromBuffer),
            0x04 => Ok(Self::DrainTreasury),
            0x05 => Ok(Self::AssignTreasuryOwner),
            _ => Err(LazyClaimError::UnknownInstruction.into()),
        }
    }
}

/// Args for `InitProofBuffer`. Wire format:
///
/// ```text
/// [0..1]  discriminator (0x01)
/// [1..33] pubkey (32 bytes — the claim leaf pubkey, used in the PDA seeds)
/// [33..37] total_len (LE u32) — total bytes that will be written into the buffer
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitProofBufferArgs {
    pub pubkey: [u8; 32],
    pub total_len: u32,
}

impl InitProofBufferArgs {
    pub const WIRE_LEN: usize = 32 + 4;

    pub fn to_ix_data(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + Self::WIRE_LEN);
        out.push(LazyClaimInstruction::InitProofBuffer as u8);
        out.extend_from_slice(&self.pubkey);
        out.extend_from_slice(&self.total_len.to_le_bytes());
        out
    }

    pub fn decode_body(body: &[u8]) -> Result<Self, ProgramError> {
        if body.len() < Self::WIRE_LEN {
            return Err(LazyClaimError::BadInstructionData.into());
        }
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&body[0..32]);
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&body[32..36]);
        Ok(Self {
            pubkey,
            total_len: u32::from_le_bytes(len_bytes),
        })
    }
}

/// Args for `WriteProofBuffer`. Wire format:
///
/// ```text
/// [0..1]   discriminator (0x02)
/// [1..5]   offset (LE u32) — byte offset within the buffer payload
/// [5..7]   chunk_len (LE u16)
/// [7..]    chunk_bytes (chunk_len bytes)
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteProofBufferArgs {
    pub offset: u32,
    pub bytes: Vec<u8>,
}

impl WriteProofBufferArgs {
    pub fn to_ix_data(&self) -> Result<Vec<u8>, ProgramError> {
        let chunk_len = u16::try_from(self.bytes.len())
            .map_err(|_| ProgramError::from(LazyClaimError::BadInstructionData))?;
        let mut out = Vec::with_capacity(1 + 4 + 2 + self.bytes.len());
        out.push(LazyClaimInstruction::WriteProofBuffer as u8);
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&chunk_len.to_le_bytes());
        out.extend_from_slice(&self.bytes);
        Ok(out)
    }

    pub fn decode_body(body: &[u8]) -> Result<Self, ProgramError> {
        if body.len() < 4 + 2 {
            return Err(LazyClaimError::BadInstructionData.into());
        }
        let mut off_bytes = [0u8; 4];
        off_bytes.copy_from_slice(&body[0..4]);
        let offset = u32::from_le_bytes(off_bytes);
        let mut len_bytes = [0u8; 2];
        len_bytes.copy_from_slice(&body[4..6]);
        let chunk_len = u16::from_le_bytes(len_bytes) as usize;
        if body.len() < 6 + chunk_len {
            return Err(LazyClaimError::BadInstructionData.into());
        }
        let bytes = body[6..6 + chunk_len].to_vec();
        Ok(Self { offset, bytes })
    }
}

/// Args for `ClaimFromBuffer`. Same shape as `Claim` minus the inline proof bytes —
/// they're read from the proof-buffer PDA passed at the end of the accounts list.
///
/// Wire format:
///
/// ```text
/// [0..1]   discriminator (0x03)
/// [1..33]  pubkey (32 bytes)
/// [33..41] lamports (LE u64)
/// [41..43] proof_len (LE u16) — sibling count; total proof bytes = proof_len * 32
/// [43..]   proof_flags: ceil(proof_len / 8) bytes
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimFromBufferArgs {
    pub pubkey: [u8; 32],
    pub lamports: u64,
    pub proof_len: u16,
    pub proof_flags: Vec<u8>,
}

impl ClaimFromBufferArgs {
    pub fn to_ix_data(&self) -> Result<Vec<u8>, ProgramError> {
        let expected_flag_bytes = (self.proof_len as usize + 7) / 8;
        if self.proof_flags.len() != expected_flag_bytes {
            return Err(LazyClaimError::ProofLengthMismatch.into());
        }
        let mut out = Vec::with_capacity(1 + 32 + 8 + 2 + self.proof_flags.len());
        out.push(LazyClaimInstruction::ClaimFromBuffer as u8);
        out.extend_from_slice(&self.pubkey);
        out.extend_from_slice(&self.lamports.to_le_bytes());
        out.extend_from_slice(&self.proof_len.to_le_bytes());
        out.extend_from_slice(&self.proof_flags);
        Ok(out)
    }

    pub fn decode_body(body: &[u8]) -> Result<Self, ProgramError> {
        if body.len() < 32 + 8 + 2 {
            return Err(LazyClaimError::BadInstructionData.into());
        }
        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&body[0..32]);
        let mut lamport_bytes = [0u8; 8];
        lamport_bytes.copy_from_slice(&body[32..40]);
        let lamports = u64::from_le_bytes(lamport_bytes);
        let mut len_bytes = [0u8; 2];
        len_bytes.copy_from_slice(&body[40..42]);
        let proof_len = u16::from_le_bytes(len_bytes);
        let flag_bytes = (proof_len as usize + 7) / 8;
        if body.len() < 42 + flag_bytes {
            return Err(LazyClaimError::BadInstructionData.into());
        }
        let proof_flags = body[42..42 + flag_bytes].to_vec();
        Ok(Self {
            pubkey,
            lamports,
            proof_len,
            proof_flags,
        })
    }
}

/// Decoded body of a `Claim` instruction. Owned form — the on-chain handler decodes into
/// this; client-side tooling encodes from it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimArgs {
    pub pubkey: [u8; 32],
    pub lamports: u64,
    pub proof: Vec<[u8; 32]>,
    /// Packed bitfield — bit `i` controls level `i`. See `merkle.rs` doc comment.
    pub proof_flags: Vec<u8>,
}

impl ClaimArgs {
    /// Serialize self into a fresh `Vec`, prefixed with the `Claim` discriminator. Use this
    /// for client-side ix construction.
    pub fn to_ix_data(&self) -> Result<Vec<u8>, ProgramError> {
        let proof_len = u16::try_from(self.proof.len())
            .map_err(|_| ProgramError::from(LazyClaimError::BadInstructionData))?;
        let expected_flag_bytes = (self.proof.len() + 7) / 8;
        if self.proof_flags.len() != expected_flag_bytes {
            return Err(LazyClaimError::ProofLengthMismatch.into());
        }

        let mut out = Vec::with_capacity(
            1 + 32 + 8 + 2 + (self.proof.len() * 32) + self.proof_flags.len(),
        );
        out.push(LazyClaimInstruction::Claim as u8);
        out.extend_from_slice(&self.pubkey);
        out.extend_from_slice(&self.lamports.to_le_bytes());
        out.extend_from_slice(&proof_len.to_le_bytes());
        for sibling in &self.proof {
            out.extend_from_slice(sibling);
        }
        out.extend_from_slice(&self.proof_flags);
        Ok(out)
    }

    /// Decode a full ix data buffer (including the leading discriminator byte). Returns the
    /// parsed [`ClaimArgs`] alongside any trailing bytes (none expected).
    pub fn from_ix_data(data: &[u8]) -> Result<Self, ProgramError> {
        if data.is_empty() {
            return Err(LazyClaimError::BadInstructionData.into());
        }
        match LazyClaimInstruction::from_byte(data[0])? {
            LazyClaimInstruction::Claim => Self::decode_body(&data[1..]),
            _ => Err(LazyClaimError::UnknownInstruction.into()),
        }
    }

    /// Decode just the body (post-discriminator). Used internally and re-exported for tests.
    pub fn decode_body(body: &[u8]) -> Result<Self, ProgramError> {
        // Minimum: pubkey + lamports + proof_len.
        if body.len() < 32 + 8 + 2 {
            return Err(LazyClaimError::BadInstructionData.into());
        }

        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&body[0..32]);

        let mut lamport_bytes = [0u8; 8];
        lamport_bytes.copy_from_slice(&body[32..40]);
        let lamports = u64::from_le_bytes(lamport_bytes);

        let mut len_bytes = [0u8; 2];
        len_bytes.copy_from_slice(&body[40..42]);
        let proof_len = u16::from_le_bytes(len_bytes) as usize;

        let proof_bytes = proof_len
            .checked_mul(32)
            .ok_or(ProgramError::from(LazyClaimError::BadInstructionData))?;
        let flag_bytes = (proof_len + 7) / 8;
        let expected = 32 + 8 + 2 + proof_bytes + flag_bytes;
        if body.len() < expected {
            return Err(LazyClaimError::BadInstructionData.into());
        }

        let mut proof: Vec<[u8; 32]> = Vec::with_capacity(proof_len);
        for i in 0..proof_len {
            let off = 42 + i * 32;
            let mut sibling = [0u8; 32];
            sibling.copy_from_slice(&body[off..off + 32]);
            proof.push(sibling);
        }

        let flags_off = 42 + proof_bytes;
        let proof_flags = body[flags_off..flags_off + flag_bytes].to_vec();

        Ok(Self {
            pubkey,
            lamports,
            proof,
            proof_flags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(proof_len: usize) -> ClaimArgs {
        let proof: Vec<[u8; 32]> = (0..proof_len).map(|i| [i as u8; 32]).collect();
        let flag_bytes = (proof_len + 7) / 8;
        let proof_flags = vec![0xAAu8; flag_bytes];
        ClaimArgs {
            pubkey: [0x11; 32],
            lamports: 1_234_567_890,
            proof,
            proof_flags,
        }
    }

    #[test]
    fn round_trip_zero_proof() {
        let args = sample(0);
        let bytes = args.to_ix_data().unwrap();
        // Discriminator + pubkey + lamports + proof_len = 1 + 32 + 8 + 2 = 43 bytes.
        assert_eq!(bytes.len(), 43);
        assert_eq!(bytes[0], LazyClaimInstruction::Claim as u8);
        let decoded = ClaimArgs::from_ix_data(&bytes).unwrap();
        assert_eq!(args, decoded);
    }

    #[test]
    fn round_trip_small_proof() {
        let args = sample(3);
        let bytes = args.to_ix_data().unwrap();
        // 1 + 32 + 8 + 2 + 3*32 + 1 = 140 bytes.
        assert_eq!(bytes.len(), 140);
        let decoded = ClaimArgs::from_ix_data(&bytes).unwrap();
        assert_eq!(args, decoded);
    }

    #[test]
    fn round_trip_proof_flags_span_multiple_bytes() {
        // 17 levels needs 3 flag bytes (ceil(17/8) = 3).
        let args = sample(17);
        let bytes = args.to_ix_data().unwrap();
        let decoded = ClaimArgs::from_ix_data(&bytes).unwrap();
        assert_eq!(args, decoded);
        assert_eq!(decoded.proof_flags.len(), 3);
    }

    #[test]
    fn rejects_unknown_discriminator() {
        let mut bytes = sample(0).to_ix_data().unwrap();
        bytes[0] = 0xEE;
        assert!(ClaimArgs::from_ix_data(&bytes).is_err());
    }

    #[test]
    fn rejects_truncated_payload() {
        let bytes = sample(2).to_ix_data().unwrap();
        // Drop the trailing flag byte.
        let truncated = &bytes[..bytes.len() - 1];
        assert!(ClaimArgs::from_ix_data(truncated).is_err());
    }

    #[test]
    fn rejects_empty_payload() {
        assert!(ClaimArgs::from_ix_data(&[]).is_err());
    }

    #[test]
    fn to_ix_data_validates_flag_byte_count() {
        let mut args = sample(8);
        args.proof_flags = vec![0u8]; // 8 levels need 1 byte; force 1 then bump proof to 9.
        args.proof.push([0u8; 32]); // now 9 levels but still 1 flag byte (need 2).
        assert!(args.to_ix_data().is_err());
    }
}

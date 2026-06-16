//! Publish a signed `R` update to the staccana bridge program.
//!
//! Per SPEC §5.3, the federation publishes the attestation by submitting an
//! `update_ratio` instruction to the bridge program. The on-chain handler:
//!
//! 1. Recomputes the [`build_attestation_message`](crate::sign::build_attestation_message)
//!    bytes from the supplied args.
//! 2. Verifies M ed25519 signatures against the registered federation set (preceding
//!    ed25519 precompile ix in the same transaction).
//! 3. Asserts `slot >= last_published_slot[asset_id] + R_PUBLISH_INTERVAL_SLOTS`.
//! 4. Recomputes `R_q64 = (vault_value << 64) / mint_supply`.
//! 5. Updates the `RatioState` PDA.
//!
//! ## v0 status
//!
//! - [`build_update_ratio_ix`] constructs the on-chain ix in spec-conformant form. **Pure
//!   and unit-testable.**
//! - [`publish_attestation`] is **STUBBED**: it logs the action and returns
//!   [`PublishError::NotImplemented`]. Wiring up a real RPC submit needs the bridge
//!   program id (TBD per SPEC §2.1) and the per-asset PDA bumps. See the function-level
//!   doc comment for the integration points.

use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use crate::sign::{AttestationInputs, SignedAttestation};
#[cfg(test)]
use crate::sign::{ATTESTATION_DOMAIN, ATTESTATION_LEN};

/// Bridge program ix discriminator for `update_ratio`.
///
/// Anchor derives this as `sha256("global:update_ratio")[0..8]`. The bytes below were
/// computed independently and verified against the Anchor convention; if the bridge program
/// renames the ix, regenerate via:
///
/// ```bash
/// python3 -c "import hashlib; print(hashlib.sha256(b'global:update_ratio').digest()[:8].hex())"
/// # → 09d85d8a268aafc1
/// ```
///
/// Long-term the integrator can replace this constant with a direct re-export of
/// `staccana_bridge::instruction::UpdateRatio::DISCRIMINATOR` once the federation-attestor
/// crate takes a path-dep on the bridge crate. For v0 we keep the constant inline so the
/// dep graph stays light and the value is testable byte-for-byte.
pub const UPDATE_RATIO_DISCRIMINATOR: [u8; 8] =
    [0x09, 0xd8, 0x5d, 0x8a, 0x26, 0x8a, 0xaf, 0xc1];

/// Errors the publisher can surface.
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("publishing not yet implemented (v0 stub) — would have submitted to {staccana_rpc} (sig count = {sig_count})")]
    NotImplemented {
        staccana_rpc: String,
        sig_count: usize,
    },

    #[error("signature aggregation incomplete: have {have}, need M = {need_m}")]
    InsufficientSignatures { have: usize, need_m: usize },

    #[error("attestations disagree on inputs (cannot aggregate)")]
    DisagreeingAttestations,

    #[error("rpc / submit error: {0}")]
    Rpc(String),
}

/// Args struct mirroring the on-chain `update_ratio` ix layout.
///
/// Wire format (little-endian throughout):
///
/// ```text
/// [0..8]              discriminator (anchor 8-byte selector)
/// [8..12]             asset_id           (u32 LE)
/// [12..20]            vault_value        (u64 LE)
/// [20..28]            mint_supply        (u64 LE)
/// [28..36]            slot               (u64 LE)
/// [36..44]            nonce              (u64 LE)
/// [44..45]            sig_count          (u8)
/// [45..45 + 64*K]     federation_signatures (K * 64 bytes)
/// [...+ K]            federation_indices    (K bytes)
/// ```
///
/// Note: the on-chain handler ALSO requires the M ed25519 precompile instructions to
/// appear immediately before this ix in the transaction. The args here name *which*
/// federation members signed; the precompile ixes carry the actual signature material.
/// Both are kept in the args struct here for completeness — `to_ix_data()` serializes
/// them in case a future on-chain layout pulls signatures into ix data instead.
#[derive(Clone, Debug)]
pub struct UpdateRatioArgs {
    pub inputs: AttestationInputs,
    /// Indices into the registered `FederationSet.members` array. Length == M.
    pub federation_indices: Vec<u8>,
    /// Raw 64-byte signatures, one per index. Length == M.
    pub federation_signatures: Vec<[u8; 64]>,
}

impl UpdateRatioArgs {
    /// Serialize the ix data per the layout in this module's doc comment.
    pub fn to_ix_data(&self) -> Vec<u8> {
        let m = self.federation_indices.len();
        debug_assert_eq!(
            m,
            self.federation_signatures.len(),
            "indices and signatures must be 1:1"
        );

        // 8 (disc) + 4 + 8 + 8 + 8 + 8 (inputs) + 1 (sig_count) + m*64 (sigs) + m (idxs)
        let mut out = Vec::with_capacity(8 + 4 + 8 + 8 + 8 + 8 + 1 + m * 64 + m);
        out.extend_from_slice(&UPDATE_RATIO_DISCRIMINATOR);
        out.extend_from_slice(&self.inputs.asset_id.to_le_bytes());
        out.extend_from_slice(&self.inputs.vault_value.to_le_bytes());
        out.extend_from_slice(&self.inputs.mint_supply.to_le_bytes());
        out.extend_from_slice(&self.inputs.slot.to_le_bytes());
        out.extend_from_slice(&self.inputs.nonce.to_le_bytes());
        out.push(m as u8);
        for sig in &self.federation_signatures {
            out.extend_from_slice(sig);
        }
        out.extend_from_slice(&self.federation_indices);
        out
    }
}

/// Build the staccana `update_ratio` `Instruction` ready to drop into a `Transaction`.
///
/// `bridge_program_id` is the on-chain bridge program (TBD per SPEC §2.1); the integrator
/// passes the resolved id at runtime.
///
/// Account list mirrors what the on-chain handler will require:
///
/// | # | Role | Description |
/// |---|---|---|
/// | 0 | `[writable]` | Bridge program state |
/// | 1 | `[writable]` | Asset ratio PDA `["ratio", asset_id]` |
/// | 2 | `[]`         | Asset config PDA `["asset", asset_id]` |
/// | 3 | `[]`         | Federation pubkey set PDA `["federation"]` |
/// | 4 | `[]`         | `Instructions` sysvar (so the handler can inspect preceding ed25519 precompile ixes) |
///
/// PDAs are passed in by the caller — derivation lives in the bridge crate, not here, so
/// this tool stays decoupled from Anchor.
#[allow(clippy::too_many_arguments)]
pub fn build_update_ratio_ix(
    bridge_program_id: Pubkey,
    bridge_state: Pubkey,
    ratio_pda: Pubkey,
    asset_config_pda: Pubkey,
    federation_set_pda: Pubkey,
    instructions_sysvar: Pubkey,
    args: &UpdateRatioArgs,
) -> Instruction {
    Instruction {
        program_id: bridge_program_id,
        accounts: vec![
            AccountMeta::new(bridge_state, false),
            AccountMeta::new(ratio_pda, false),
            AccountMeta::new_readonly(asset_config_pda, false),
            AccountMeta::new_readonly(federation_set_pda, false),
            AccountMeta::new_readonly(instructions_sysvar, false),
        ],
        data: args.to_ix_data(),
    }
}

/// Aggregate M signed attestations from federation members into an [`UpdateRatioArgs`]
/// ready for [`build_update_ratio_ix`].
///
/// All `attestations` MUST cover the *same* `AttestationInputs`; they differ only in
/// `signer` / `signature`. `member_indices[i]` is the on-chain set index for
/// `attestations[i]`.
pub fn aggregate_attestations(
    inputs: AttestationInputs,
    attestations: &[SignedAttestation],
    member_indices: &[u8],
    m: usize,
) -> Result<UpdateRatioArgs, PublishError> {
    if attestations.len() < m {
        return Err(PublishError::InsufficientSignatures {
            have: attestations.len(),
            need_m: m,
        });
    }
    if attestations.len() != member_indices.len() {
        return Err(PublishError::DisagreeingAttestations);
    }

    let expected_msg = crate::sign::build_attestation_message(inputs);
    for att in attestations {
        if att.message != expected_msg {
            return Err(PublishError::DisagreeingAttestations);
        }
    }

    let federation_signatures: Vec<[u8; 64]> = attestations
        .iter()
        .map(|a| {
            let bytes = a.signature.as_ref();
            // ed25519 signatures are exactly 64 bytes; this is a sanity assert.
            debug_assert_eq!(bytes.len(), 64);
            let mut out = [0u8; 64];
            out.copy_from_slice(bytes);
            out
        })
        .collect();

    Ok(UpdateRatioArgs {
        inputs,
        federation_indices: member_indices.to_vec(),
        federation_signatures,
    })
}

/// Submit the aggregated attestation to staccana.
///
/// **v0 STUB.** Real impl needs:
///
/// 1. A `solana_client::rpc_client::RpcClient` connected to `staccana_rpc`.
/// 2. A fee-payer `Keypair` (the federation member's key, or a dedicated relayer key).
/// 3. The bridge program id and the resolved PDAs (see [`build_update_ratio_ix`] doc).
/// 4. M `Instruction::new_with_bytes(ed25519_program::id(), …)` precompile ixes
///    immediately preceding the `update_ratio` ix.
/// 5. `RpcClient::send_and_confirm_transaction_with_spinner_and_commitment(...,
///    CommitmentConfig::confirmed())`.
///
/// The current implementation logs and returns
/// [`PublishError::NotImplemented`] so the daemon main loop can still exercise the path.
pub fn publish_attestation(
    staccana_rpc: &str,
    args: &UpdateRatioArgs,
) -> Result<(), PublishError> {
    eprintln!(
        "[federation-attestor] publish stub: would submit update_ratio to {staccana_rpc} \
         (asset_id={}, slot={}, nonce={}, sig_count={})",
        args.inputs.asset_id,
        args.inputs.slot,
        args.inputs.nonce,
        args.federation_signatures.len(),
    );
    Err(PublishError::NotImplemented {
        staccana_rpc: staccana_rpc.to_string(),
        sig_count: args.federation_signatures.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::sign_attestation;
    use solana_sdk::signature::Keypair;

    fn sample_inputs() -> AttestationInputs {
        AttestationInputs {
            asset_id: 7,
            vault_value: 1_000_000_000,
            mint_supply: 999_500_000,
            slot: 12_345_000,
            nonce: 42,
        }
    }

    fn sample_args(m: usize) -> UpdateRatioArgs {
        let inputs = sample_inputs();
        let attestations: Vec<_> = (0..m)
            .map(|_| sign_attestation(inputs, &Keypair::new()))
            .collect();
        let indices: Vec<u8> = (0..m as u8).collect();
        aggregate_attestations(inputs, &attestations, &indices, m).unwrap()
    }

    #[test]
    fn ix_data_layout_is_byte_exact_for_zero_sigs() {
        // Edge case: no signatures attached. Lets the layout be inspected without sig
        // noise. (Real txs always carry M sigs; this is a lower-bound sanity test.)
        let args = UpdateRatioArgs {
            inputs: sample_inputs(),
            federation_indices: vec![],
            federation_signatures: vec![],
        };
        let data = args.to_ix_data();

        // 8 disc + 4 + 8 + 8 + 8 + 8 + 1 (sig_count) = 45
        assert_eq!(data.len(), 45);
        assert_eq!(&data[..8], &UPDATE_RATIO_DISCRIMINATOR);
        assert_eq!(&data[8..12], &7u32.to_le_bytes());
        assert_eq!(&data[12..20], &1_000_000_000u64.to_le_bytes());
        assert_eq!(&data[20..28], &999_500_000u64.to_le_bytes());
        assert_eq!(&data[28..36], &12_345_000u64.to_le_bytes());
        assert_eq!(&data[36..44], &42u64.to_le_bytes());
        assert_eq!(data[44], 0); // sig_count
    }

    #[test]
    fn ix_data_includes_signatures_and_indices() {
        let args = sample_args(5);
        let data = args.to_ix_data();
        // 45 (header) + 5*64 (sigs) + 5 (idxs) = 370
        assert_eq!(data.len(), 45 + 5 * 64 + 5);
        assert_eq!(data[44], 5); // sig_count
        // Last 5 bytes are the indices [0,1,2,3,4]
        let tail = &data[data.len() - 5..];
        assert_eq!(tail, &[0u8, 1, 2, 3, 4]);
    }

    #[test]
    fn build_ix_carries_correct_program_and_accounts() {
        let bridge = Pubkey::new_unique();
        let state = Pubkey::new_unique();
        let ratio = Pubkey::new_unique();
        let cfg = Pubkey::new_unique();
        let fed = Pubkey::new_unique();
        let sysvar = Pubkey::new_unique();
        let args = sample_args(3);
        let ix = build_update_ratio_ix(bridge, state, ratio, cfg, fed, sysvar, &args);
        assert_eq!(ix.program_id, bridge);
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(ix.accounts[0].pubkey, state);
        assert!(ix.accounts[0].is_writable);
        assert_eq!(ix.accounts[1].pubkey, ratio);
        assert!(ix.accounts[1].is_writable);
        assert_eq!(ix.accounts[2].pubkey, cfg);
        assert!(!ix.accounts[2].is_writable);
        assert_eq!(ix.accounts[3].pubkey, fed);
        assert!(!ix.accounts[3].is_writable);
        assert_eq!(ix.accounts[4].pubkey, sysvar);
        assert!(!ix.accounts[4].is_writable);
        assert_eq!(ix.data, args.to_ix_data());
    }

    #[test]
    fn aggregate_rejects_too_few_signatures() {
        let inputs = sample_inputs();
        let one = sign_attestation(inputs, &Keypair::new());
        let err = aggregate_attestations(inputs, &[one], &[0], 5).unwrap_err();
        assert!(matches!(
            err,
            PublishError::InsufficientSignatures { have: 1, need_m: 5 }
        ));
    }

    #[test]
    fn aggregate_rejects_disagreeing_inputs() {
        let inputs_a = sample_inputs();
        let mut inputs_b = inputs_a;
        inputs_b.nonce += 1;
        let a = sign_attestation(inputs_a, &Keypair::new());
        let b = sign_attestation(inputs_b, &Keypair::new());
        let err = aggregate_attestations(inputs_a, &[a, b], &[0, 1], 2).unwrap_err();
        assert!(matches!(err, PublishError::DisagreeingAttestations));
    }

    #[test]
    fn aggregate_rejects_index_count_mismatch() {
        let inputs = sample_inputs();
        let a = sign_attestation(inputs, &Keypair::new());
        let b = sign_attestation(inputs, &Keypair::new());
        // Two attestations, one index → mismatch.
        let err = aggregate_attestations(inputs, &[a, b], &[0], 2).unwrap_err();
        assert!(matches!(err, PublishError::DisagreeingAttestations));
    }

    #[test]
    fn publish_stub_returns_not_implemented() {
        let args = sample_args(5);
        let err = publish_attestation("https://rpc.staccana.network", &args).unwrap_err();
        assert!(matches!(err, PublishError::NotImplemented { .. }));
    }

    #[test]
    fn ix_data_header_layout_independent_of_sig_count() {
        // The first 45 bytes (header) MUST be identical regardless of how many sigs
        // follow. Pin this so a refactor can't accidentally interleave sig data into the
        // header region.
        let args0 = UpdateRatioArgs {
            inputs: sample_inputs(),
            federation_indices: vec![],
            federation_signatures: vec![],
        };
        let args5 = sample_args(5);
        // Mask out sig_count byte (offset 44) since it differs intentionally.
        let mut h0 = args0.to_ix_data()[..45].to_vec();
        let mut h5 = args5.to_ix_data()[..45].to_vec();
        h0[44] = 0;
        h5[44] = 0;
        assert_eq!(h0, h5);
    }

    /// Belt-and-suspenders: assert the constants the on-chain side will rely on are what
    /// SPEC §5.3 says they are.
    #[test]
    fn spec_constants_unchanged() {
        assert_eq!(ATTESTATION_DOMAIN, b"STACCANA_RATIO_V1");
        assert_eq!(ATTESTATION_LEN, 53);
    }
}

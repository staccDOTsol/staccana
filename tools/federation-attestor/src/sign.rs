//! Attestation message construction and signing per SPEC §5.3.
//!
//! The byte layout of an attestation is **wire-format-locked**:
//!
//! ```text
//! attestation =
//!     b"STACCANA_RATIO_V1"           (17 bytes — domain separator)
//!  || asset_id.to_le_bytes()         ( 4 bytes — u32 LE)
//!  || vault_value.to_le_bytes()      ( 8 bytes — u64 LE, value in underlying)
//!  || mint_supply.to_le_bytes()      ( 8 bytes — u64 LE, current mint supply)
//!  || slot.to_le_bytes()             ( 8 bytes — u64 LE, observation slot)
//!  || nonce.to_le_bytes()            ( 8 bytes — u64 LE, monotonic per (asset, direction))
//! ```
//!
//! Total: **53 bytes**.
//!
//! M-of-N federation members each sign the *exact same byte string* with their ed25519
//! signing key. The on-chain bridge program reconstructs the bytes from the
//! `update_ratio` ix args and verifies signatures via the ed25519 precompile.
//!
//! This module is pure (no I/O, no async). The unit tests below pin the byte layout: any
//! change here that doesn't break those tests is safe; any change that breaks them is a
//! consensus break.

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;

/// Domain-separator prefix per SPEC §5.3. Exactly 17 bytes (no trailing NUL).
pub const ATTESTATION_DOMAIN: &[u8; 17] = b"STACCANA_RATIO_V1";

/// Total length in bytes of a serialized attestation message.
///
/// Sum: 17 (domain) + 4 (asset_id) + 8 (vault_value) + 8 (mint_supply) + 8 (slot)
/// + 8 (nonce) = **53**.
pub const ATTESTATION_LEN: usize = 17 + 4 + 8 + 8 + 8 + 8;

/// All scalar inputs for one attestation. Cheap to copy; passed around by value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttestationInputs {
    /// Per-asset id from `AssetConfig.asset_id`.
    pub asset_id: u32,
    /// Vault value denominated in the underlying (lamports for stSOL, USDC base units for
    /// ssUSDC, etc). Federation members observe this from the mainnet vault account.
    pub vault_value: u64,
    /// Current staccana mint supply for this asset. Observed from the staccana mint
    /// account.
    pub mint_supply: u64,
    /// The slot at which the observation was made. Used by the bridge program to enforce
    /// `R_PUBLISH_INTERVAL_SLOTS` spacing.
    pub slot: u64,
    /// Monotonic per-(asset_id, direction) nonce; the bridge program rejects replays.
    pub nonce: u64,
}

/// One member's signed attestation. The wire-format `message` bytes are kept alongside the
/// signature so consumers don't have to reconstruct them — useful for inter-member gossip
/// and for the publisher when assembling the M-of-N batch.
#[derive(Clone, Debug)]
pub struct SignedAttestation {
    /// The 53-byte canonical message bytes per SPEC §5.3.
    pub message: [u8; ATTESTATION_LEN],
    /// The signer's pubkey. The on-chain set lookup happens by `federation_index`, but
    /// keeping the pubkey here lets a relayer match index ↔ key without a side table.
    pub signer: Pubkey,
    /// ed25519 signature over `message`.
    pub signature: Signature,
}

/// Construct the canonical attestation byte string per SPEC §5.3.
///
/// The output is exactly [`ATTESTATION_LEN`] bytes. Callers MUST use this function rather
/// than rolling their own concatenation — the byte layout is consensus-relevant.
pub fn build_attestation_message(inputs: AttestationInputs) -> [u8; ATTESTATION_LEN] {
    let mut out = [0u8; ATTESTATION_LEN];
    let mut off = 0;

    // Domain: 17 bytes.
    out[off..off + ATTESTATION_DOMAIN.len()].copy_from_slice(ATTESTATION_DOMAIN);
    off += ATTESTATION_DOMAIN.len();

    // asset_id: 4 bytes LE.
    out[off..off + 4].copy_from_slice(&inputs.asset_id.to_le_bytes());
    off += 4;

    // vault_value: 8 bytes LE.
    out[off..off + 8].copy_from_slice(&inputs.vault_value.to_le_bytes());
    off += 8;

    // mint_supply: 8 bytes LE.
    out[off..off + 8].copy_from_slice(&inputs.mint_supply.to_le_bytes());
    off += 8;

    // slot: 8 bytes LE.
    out[off..off + 8].copy_from_slice(&inputs.slot.to_le_bytes());
    off += 8;

    // nonce: 8 bytes LE.
    out[off..off + 8].copy_from_slice(&inputs.nonce.to_le_bytes());
    off += 8;

    debug_assert_eq!(off, ATTESTATION_LEN);
    out
}

/// Build the attestation message from `inputs` and sign it with `keypair`.
///
/// `keypair` should be the federation member's signing key (the same key that appears at
/// `federation_index` in the on-chain `FederationSet.members` array). This function does
/// not enforce that linkage — the daemon's config layer is responsible.
pub fn sign_attestation(inputs: AttestationInputs, keypair: &Keypair) -> SignedAttestation {
    let message = build_attestation_message(inputs);
    let signature = keypair.sign_message(&message);
    SignedAttestation {
        message,
        signer: keypair.pubkey(),
        signature,
    }
}

/// Verify a previously signed attestation. Returns `true` iff the signature is valid for
/// the carried `message` under `signer`.
///
/// Off-chain helper; the on-chain bridge does this via the ed25519 precompile, not via
/// this function.
pub fn verify_attestation(att: &SignedAttestation) -> bool {
    att.signature.verify(att.signer.as_ref(), &att.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample inputs used by the layout-pinning tests. Values chosen to make the LE
    /// encoding visually distinct in a hex dump.
    fn sample_inputs() -> AttestationInputs {
        AttestationInputs {
            asset_id: 0x0102_0304,
            vault_value: 0x1112_1314_1516_1718,
            mint_supply: 0x2122_2324_2526_2728,
            slot: 0x3132_3334_3536_3738,
            nonce: 0x4142_4344_4546_4748,
        }
    }

    #[test]
    fn message_length_matches_spec() {
        let msg = build_attestation_message(sample_inputs());
        assert_eq!(msg.len(), ATTESTATION_LEN);
        assert_eq!(ATTESTATION_LEN, 53);
    }

    #[test]
    fn message_layout_is_byte_exact() {
        // This test is the wire-format lock for SPEC §5.3. Edit only when bumping the
        // domain separator (which would also rev the protocol).
        let msg = build_attestation_message(sample_inputs());

        // Expected = b"STACCANA_RATIO_V1" || LE(asset_id) || LE(vault_value)
        //          || LE(mint_supply) || LE(slot) || LE(nonce)
        let mut expected = Vec::with_capacity(ATTESTATION_LEN);
        expected.extend_from_slice(b"STACCANA_RATIO_V1");
        expected.extend_from_slice(&0x0102_0304u32.to_le_bytes());
        expected.extend_from_slice(&0x1112_1314_1516_1718u64.to_le_bytes());
        expected.extend_from_slice(&0x2122_2324_2526_2728u64.to_le_bytes());
        expected.extend_from_slice(&0x3132_3334_3536_3738u64.to_le_bytes());
        expected.extend_from_slice(&0x4142_4344_4546_4748u64.to_le_bytes());

        assert_eq!(&msg[..], expected.as_slice());
    }

    #[test]
    fn message_domain_prefix_is_exact() {
        // The first 17 bytes MUST be the literal ASCII string "STACCANA_RATIO_V1" with no
        // trailing NUL. A NUL at byte 17 would shift every numeric field by one and break
        // verification.
        let msg = build_attestation_message(sample_inputs());
        assert_eq!(&msg[..17], b"STACCANA_RATIO_V1");
        // First byte after the domain is the LSB of asset_id (0x04 in our sample).
        assert_eq!(msg[17], 0x04);
    }

    #[test]
    fn message_changes_when_any_input_changes() {
        // Property: every field actually contributes to the bytes. Catches bugs where a
        // field is silently dropped or a slice offset is miscomputed.
        let base = build_attestation_message(sample_inputs());

        let mut bumped_asset = sample_inputs();
        bumped_asset.asset_id = bumped_asset.asset_id.wrapping_add(1);
        assert_ne!(build_attestation_message(bumped_asset), base);

        let mut bumped_value = sample_inputs();
        bumped_value.vault_value = bumped_value.vault_value.wrapping_add(1);
        assert_ne!(build_attestation_message(bumped_value), base);

        let mut bumped_supply = sample_inputs();
        bumped_supply.mint_supply = bumped_supply.mint_supply.wrapping_add(1);
        assert_ne!(build_attestation_message(bumped_supply), base);

        let mut bumped_slot = sample_inputs();
        bumped_slot.slot = bumped_slot.slot.wrapping_add(1);
        assert_ne!(build_attestation_message(bumped_slot), base);

        let mut bumped_nonce = sample_inputs();
        bumped_nonce.nonce = bumped_nonce.nonce.wrapping_add(1);
        assert_ne!(build_attestation_message(bumped_nonce), base);
    }

    #[test]
    fn message_is_zeroed_for_zero_inputs() {
        // Sanity: zero inputs ⇒ domain prefix followed by 36 zero bytes. Confirms there's
        // no padding or stray-byte injection in the encoder.
        let inputs = AttestationInputs {
            asset_id: 0,
            vault_value: 0,
            mint_supply: 0,
            slot: 0,
            nonce: 0,
        };
        let msg = build_attestation_message(inputs);
        assert_eq!(&msg[..17], b"STACCANA_RATIO_V1");
        assert!(msg[17..].iter().all(|&b| b == 0));
    }

    #[test]
    fn signing_round_trip_verifies() {
        // Generate a fresh keypair, sign, and verify. This is the daemon's hot path
        // distilled to its core: any failure here breaks the whole publish loop.
        let keypair = Keypair::new();
        let inputs = sample_inputs();
        let signed = sign_attestation(inputs, &keypair);

        assert_eq!(signed.signer, keypair.pubkey());
        assert_eq!(signed.message, build_attestation_message(inputs));
        assert!(verify_attestation(&signed));
    }

    #[test]
    fn signing_is_deterministic_for_same_inputs() {
        // ed25519 deterministic-K means: same key, same message ⇒ same signature.
        // Important for relayers de-duplicating gossip.
        let keypair = Keypair::new();
        let inputs = sample_inputs();
        let a = sign_attestation(inputs, &keypair);
        let b = sign_attestation(inputs, &keypair);
        assert_eq!(a.signature, b.signature);
    }

    #[test]
    fn verify_rejects_signature_under_wrong_key() {
        // A signature from key X must not verify under key Y, even on the same message.
        let alice = Keypair::new();
        let bob = Keypair::new();
        let inputs = sample_inputs();
        let mut signed = sign_attestation(inputs, &alice);
        signed.signer = bob.pubkey();
        assert!(!verify_attestation(&signed));
    }

    #[test]
    fn verify_rejects_tampered_message() {
        // Flipping any byte of the message must break verification.
        let keypair = Keypair::new();
        let mut signed = sign_attestation(sample_inputs(), &keypair);
        signed.message[20] ^= 0x01;
        assert!(!verify_attestation(&signed));
    }
}

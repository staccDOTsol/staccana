//! Cross-chain attestation message construction for the deposit/burn → mint/release flow.
//!
//! Two message kinds, each domain-separated so a signature for one can never replay as
//! the other:
//!
//! - **`STACCANA_MINT_V1`** (68 bytes) — federation-signed, consumed staccana-side by
//!   `bridge::mint`. Triggered by a `Deposit` event on the mainnet/devnet bridge-vault.
//!   Layout: `b"STACCANA_MINT_V1" || asset_id_le || value_after_fee_le || recipient ||
//!   nonce_le`.
//!
//! - **`MAINNET_RELEASE_V1`** (70 bytes) — federation-signed, consumed mainnet-side by
//!   `bridge-vault::release_with_attestation`. Triggered by a `Burn` event on the
//!   staccana bridge program. Layout: `b"MAINNET_RELEASE_V1" || asset_id_le ||
//!   release_amount_le || recipient || nonce_le`.
//!
//! These bytes are wire-format-locked: the on-chain handler in
//! `programs/bridge/src/attestation.rs::build_mint_message` and
//! `programs/bridge-vault/src/attestation.rs::build_release_message` build the
//! identical preimage. The unit tests below pin the exact bytes.

use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;

/// Domain-separation prefix for inbound (mainnet → staccana) mint attestations. Must
/// match `programs/bridge/src/attestation.rs::MINT_DOMAIN` byte-for-byte.
pub const MINT_DOMAIN: &[u8; 16] = b"STACCANA_MINT_V1";

/// Total length in bytes of a serialized mint-attestation message:
/// 16 (domain) + 4 (asset_id) + 8 (value_after_fee) + 32 (recipient) + 8 (nonce) = 68.
pub const MINT_MSG_LEN: usize = 16 + 4 + 8 + 32 + 8;

/// Domain-separation prefix for outbound (staccana → mainnet) release attestations.
/// Must match `programs/bridge-vault/src/attestation.rs::RELEASE_DOMAIN` byte-for-byte.
pub const RELEASE_DOMAIN: &[u8; 18] = b"MAINNET_RELEASE_V1";

/// Total length in bytes of a serialized release-attestation message:
/// 18 (domain) + 4 (asset_id) + 8 (release_amount) + 32 (recipient) + 8 (nonce) = 70.
pub const RELEASE_MSG_LEN: usize = 18 + 4 + 8 + 32 + 8;

/// Build the canonical mint-attestation byte string.
///
/// Inputs come from a mainnet/devnet `DepositEvent`: `asset_id`, `amount_after_fee`
/// (the post-vault-fee credited amount), `dest` (staccana-side recipient pubkey), and
/// `nonce` (per-asset deposit-direction monotonic counter from the vault).
pub fn build_mint_message(
    asset_id: u32,
    value_after_fee: u64,
    recipient: &[u8; 32],
    nonce: u64,
) -> [u8; MINT_MSG_LEN] {
    let mut out = [0u8; MINT_MSG_LEN];
    let mut off = 0;
    out[off..off + 16].copy_from_slice(MINT_DOMAIN);
    off += 16;
    out[off..off + 4].copy_from_slice(&asset_id.to_le_bytes());
    off += 4;
    out[off..off + 8].copy_from_slice(&value_after_fee.to_le_bytes());
    off += 8;
    out[off..off + 32].copy_from_slice(recipient);
    off += 32;
    out[off..off + 8].copy_from_slice(&nonce.to_le_bytes());
    off += 8;
    debug_assert_eq!(off, MINT_MSG_LEN);
    out
}

/// Build the canonical release-attestation byte string.
///
/// Inputs come from a staccana `BurnEvent`: `asset_id`, `gross_release` (release
/// amount before mainnet fee), `mainnet_dest` (recipient pubkey on mainnet/devnet),
/// `nonce_out` (per-asset burn-direction monotonic counter from the staccana bridge).
pub fn build_release_message(
    asset_id: u32,
    release_amount: u64,
    recipient: &[u8; 32],
    nonce: u64,
) -> [u8; RELEASE_MSG_LEN] {
    let mut out = [0u8; RELEASE_MSG_LEN];
    let mut off = 0;
    out[off..off + 18].copy_from_slice(RELEASE_DOMAIN);
    off += 18;
    out[off..off + 4].copy_from_slice(&asset_id.to_le_bytes());
    off += 4;
    out[off..off + 8].copy_from_slice(&release_amount.to_le_bytes());
    off += 8;
    out[off..off + 32].copy_from_slice(recipient);
    off += 32;
    out[off..off + 8].copy_from_slice(&nonce.to_le_bytes());
    off += 8;
    debug_assert_eq!(off, RELEASE_MSG_LEN);
    out
}

/// Sign a mint message with the federation member's keypair. Returns the 64-byte
/// ed25519 signature alongside the original preimage so the caller (or peer-gossip
/// layer) doesn't have to reconstruct the bytes.
pub fn sign_mint(
    asset_id: u32,
    value_after_fee: u64,
    recipient: &[u8; 32],
    nonce: u64,
    keypair: &Keypair,
) -> ([u8; MINT_MSG_LEN], Signature) {
    let msg = build_mint_message(asset_id, value_after_fee, recipient, nonce);
    let sig = keypair.sign_message(&msg);
    (msg, sig)
}

/// Sign a release message with the federation member's keypair. Same shape as
/// [`sign_mint`]; the only difference is the domain prefix and total length.
pub fn sign_release(
    asset_id: u32,
    release_amount: u64,
    recipient: &[u8; 32],
    nonce: u64,
    keypair: &Keypair,
) -> ([u8; RELEASE_MSG_LEN], Signature) {
    let msg = build_release_message(asset_id, release_amount, recipient, nonce);
    let sig = keypair.sign_message(&msg);
    (msg, sig)
}

/// Verify an ed25519 signature over a mint message. Off-chain helper; on-chain the
/// bridge program does this via the ed25519 precompile sysvar inspection.
pub fn verify_mint(
    msg: &[u8; MINT_MSG_LEN],
    signer: &solana_sdk::pubkey::Pubkey,
    sig: &Signature,
) -> bool {
    sig.verify(signer.as_ref(), msg)
}

/// Verify an ed25519 signature over a release message.
pub fn verify_release(
    msg: &[u8; RELEASE_MSG_LEN],
    signer: &solana_sdk::pubkey::Pubkey,
    sig: &Signature,
) -> bool {
    sig.verify(signer.as_ref(), msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layout-pinning test for `STACCANA_MINT_V1`. The on-chain bridge handler
    /// reconstructs these exact bytes; any encoder change here that doesn't break this
    /// test is safe, anything that breaks it is a consensus break.
    #[test]
    fn mint_message_layout_byte_exact() {
        let asset_id: u32 = 0xDEAD_BEEF;
        let value: u64 = 0x0102_0304_0506_0708;
        let recipient = [0x42u8; 32];
        let nonce: u64 = 0x1112_1314_1516_1718;

        let msg = build_mint_message(asset_id, value, &recipient, nonce);
        assert_eq!(msg.len(), MINT_MSG_LEN);
        assert_eq!(MINT_MSG_LEN, 68);
        assert_eq!(&msg[0..16], MINT_DOMAIN);
        assert_eq!(&msg[16..20], &asset_id.to_le_bytes());
        assert_eq!(&msg[20..28], &value.to_le_bytes());
        assert_eq!(&msg[28..60], &recipient);
        assert_eq!(&msg[60..68], &nonce.to_le_bytes());
    }

    /// Mirror of the layout-pinning test for `MAINNET_RELEASE_V1`.
    #[test]
    fn release_message_layout_byte_exact() {
        let asset_id: u32 = 0xDEAD_BEEF;
        let amount: u64 = 0x0102_0304_0506_0708;
        let recipient = [0x77u8; 32];
        let nonce: u64 = 0x9999_AAAA_BBBB_CCCC;

        let msg = build_release_message(asset_id, amount, &recipient, nonce);
        assert_eq!(msg.len(), RELEASE_MSG_LEN);
        assert_eq!(RELEASE_MSG_LEN, 70);
        assert_eq!(&msg[0..18], RELEASE_DOMAIN);
        assert_eq!(&msg[18..22], &asset_id.to_le_bytes());
        assert_eq!(&msg[22..30], &amount.to_le_bytes());
        assert_eq!(&msg[30..62], &recipient);
        assert_eq!(&msg[62..70], &nonce.to_le_bytes());
    }

    #[test]
    fn domains_are_distinct_lengths() {
        // Catastrophic if a release attestation could replay as a mint attestation;
        // the lengths alone make a literal byte-equal collision impossible.
        assert_ne!(MINT_DOMAIN.len(), RELEASE_DOMAIN.len());
        assert_eq!(MINT_DOMAIN, b"STACCANA_MINT_V1");
        assert_eq!(RELEASE_DOMAIN, b"MAINNET_RELEASE_V1");
    }

    /// Cross-chain byte-equality smoke test: confirm our preimage matches exactly
    /// what the on-chain `programs/bridge/src/attestation.rs::build_mint_message`
    /// would build (which we reimplement inline here for the test's independence).
    #[test]
    fn mint_message_matches_on_chain_layout() {
        let asset_id: u32 = 7;
        let value: u64 = 1_000_000;
        let recipient = [42u8; 32];
        let nonce: u64 = 99;

        let ours = build_mint_message(asset_id, value, &recipient, nonce);

        // Hand-build the same bytes per `programs/bridge/src/attestation.rs`.
        let mut on_chain = Vec::with_capacity(16 + 4 + 8 + 32 + 8);
        on_chain.extend_from_slice(b"STACCANA_MINT_V1");
        on_chain.extend_from_slice(&asset_id.to_le_bytes());
        on_chain.extend_from_slice(&value.to_le_bytes());
        on_chain.extend_from_slice(&recipient);
        on_chain.extend_from_slice(&nonce.to_le_bytes());

        assert_eq!(&ours[..], on_chain.as_slice());
    }

    /// Mirror — confirm the release preimage matches
    /// `programs/bridge-vault/src/attestation.rs::build_release_message`.
    #[test]
    fn release_message_matches_on_chain_layout() {
        let asset_id: u32 = 7;
        let amount: u64 = 1_000_000;
        let recipient = [99u8; 32];
        let nonce: u64 = 42;

        let ours = build_release_message(asset_id, amount, &recipient, nonce);

        let mut on_chain = Vec::with_capacity(18 + 4 + 8 + 32 + 8);
        on_chain.extend_from_slice(b"MAINNET_RELEASE_V1");
        on_chain.extend_from_slice(&asset_id.to_le_bytes());
        on_chain.extend_from_slice(&amount.to_le_bytes());
        on_chain.extend_from_slice(&recipient);
        on_chain.extend_from_slice(&nonce.to_le_bytes());

        assert_eq!(&ours[..], on_chain.as_slice());
    }

    #[test]
    fn mint_signing_round_trip_verifies() {
        let kp = Keypair::new();
        let recipient = [3u8; 32];
        let (msg, sig) = sign_mint(1, 1_000, &recipient, 7, &kp);
        assert!(verify_mint(&msg, &kp.pubkey(), &sig));
    }

    #[test]
    fn release_signing_round_trip_verifies() {
        let kp = Keypair::new();
        let recipient = [4u8; 32];
        let (msg, sig) = sign_release(1, 1_000, &recipient, 7, &kp);
        assert!(verify_release(&msg, &kp.pubkey(), &sig));
    }

    #[test]
    fn mint_verify_rejects_tampered_message() {
        let kp = Keypair::new();
        let recipient = [3u8; 32];
        let (mut msg, sig) = sign_mint(1, 1_000, &recipient, 7, &kp);
        msg[20] ^= 0x01;
        assert!(!verify_mint(&msg, &kp.pubkey(), &sig));
    }

    #[test]
    fn mint_verify_rejects_wrong_signer() {
        let kp = Keypair::new();
        let stranger = Keypair::new();
        let recipient = [3u8; 32];
        let (msg, sig) = sign_mint(1, 1_000, &recipient, 7, &kp);
        assert!(!verify_mint(&msg, &stranger.pubkey(), &sig));
    }

    /// Property test: every field is committed to. Flipping any field changes the
    /// bytes, so the signature would no longer verify against a swapped-input
    /// reconstruction.
    #[test]
    fn mint_message_changes_for_each_field() {
        let recipient = [1u8; 32];
        let other_recipient = [2u8; 32];
        let base = build_mint_message(1, 2, &recipient, 5);
        assert_ne!(build_mint_message(99, 2, &recipient, 5), base);
        assert_ne!(build_mint_message(1, 99, &recipient, 5), base);
        assert_ne!(build_mint_message(1, 2, &other_recipient, 5), base);
        assert_ne!(build_mint_message(1, 2, &recipient, 99), base);
    }

    #[test]
    fn release_message_changes_for_each_field() {
        let recipient = [1u8; 32];
        let other_recipient = [2u8; 32];
        let base = build_release_message(1, 2, &recipient, 5);
        assert_ne!(build_release_message(99, 2, &recipient, 5), base);
        assert_ne!(build_release_message(1, 99, &recipient, 5), base);
        assert_ne!(build_release_message(1, 2, &other_recipient, 5), base);
        assert_ne!(build_release_message(1, 2, &recipient, 99), base);
    }

    /// Replay-protection check: a mint signature for `(asset, value, recipient, nonce)`
    /// must not verify as a release signature for the same tuple. The two domains
    /// differ in length so the preimages can't even share a byte representation —
    /// catching the "domain prefix collision" class of bug.
    #[test]
    fn mint_signature_does_not_verify_as_release() {
        let kp = Keypair::new();
        let recipient = [9u8; 32];
        let (_mint_msg, mint_sig) = sign_mint(1, 1_000, &recipient, 7, &kp);

        // Build the corresponding release preimage and try to verify the mint sig
        // against it. Must fail.
        let release_msg = build_release_message(1, 1_000, &recipient, 7);
        assert!(!verify_release(&release_msg, &kp.pubkey(), &mint_sig));
    }
}

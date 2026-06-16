//! Pure helpers for federation release-attestation message construction.
//!
//! Factored out so they can be unit-tested without spinning up a local validator. Anchor
//! handler code calls into here for stateless work — message bytes, dedup checks, fee math.
//!
//! ## Wire format
//!
//! The release attestation mirrors the staccana-side mint attestation
//! (`STACCANA_MINT_V1`) but with a distinct domain prefix so a release attestation can
//! never replay as a mint attestation and vice-versa. See SPEC §"Replay protection".
//!
//! Layout: `b"MAINNET_RELEASE_V1" || asset_id_le || release_amount_le || recipient ||
//! nonce_le`. Total length is 18 + 4 + 8 + 32 + 8 = 70 bytes.
//!
//! Fields mirror the staccana-side `BurnEvent`:
//! - `asset_id` — must agree across chains
//! - `release_amount` — `gross_release` from the staccana burn (R has been applied
//!   already on the staccana side); the mainnet vault deducts its own `release_fee_bps`
//!   on top before transferring.
//! - `recipient` — staccana-side `mainnet_dest`, copied through verbatim
//! - `nonce` — staccana-side `nonce_out` (per-asset monotonic burn counter)

use crate::error::VaultError;

/// Domain-separation prefix for release attestations (staccana → mainnet). Must be
/// distinct from the staccana-side `STACCANA_MINT_V1` and `STACCANA_RATIO_V1` prefixes
/// so signed messages can never cross-replay.
pub const RELEASE_DOMAIN: &[u8] = b"MAINNET_RELEASE_V1";

/// `chain_id` discriminator emitted in `DepositEvent` so off-chain consumers can route
/// the event to the correct staccana-side bridge program. Matches the staccana-side
/// constant `CHAIN_ID_STACCANA` (ASCII "stac" little-endian).
pub const CHAIN_ID_STACCANA: u32 = 0x6361_7473; // ASCII "stac" (LE bytes: 73 74 61 63)

/// Build the release-attestation message that the federation signs.
///
/// Layout: `RELEASE_DOMAIN || asset_id_le || release_amount_le || recipient || nonce_le`.
pub fn build_release_message(
    asset_id: u32,
    release_amount: u64,
    recipient: &[u8; 32],
    nonce: u64,
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(RELEASE_DOMAIN.len() + 4 + 8 + 32 + 8);
    msg.extend_from_slice(RELEASE_DOMAIN);
    msg.extend_from_slice(&asset_id.to_le_bytes());
    msg.extend_from_slice(&release_amount.to_le_bytes());
    msg.extend_from_slice(recipient);
    msg.extend_from_slice(&nonce.to_le_bytes());
    msg
}

/// Apply a bps fee (deducted): `gross * (10_000 - fee_bps) / 10_000`.
///
/// Returns the post-fee amount. `fee_bps` is capped at 10_000; the spec defaults are
/// 10 (0.1%).
pub fn apply_bps_fee(gross: u64, fee_bps: u16) -> u64 {
    debug_assert!(fee_bps <= 10_000, "fee_bps capped at 10000 = 100%");
    let bps = fee_bps.min(10_000) as u128;
    let net = ((gross as u128) * (10_000 - bps)) / 10_000;
    net as u64
}

/// Reject duplicate signer indices within one attestation. `indices` is the M-element
/// slice the user passes in; valid attestations must have M distinct values, all in
/// `[0, n)`.
pub fn check_unique_indices(indices: &[u8], n: u8) -> Result<(), VaultError> {
    // n caps at 32 per `MAX_FEDERATION_MEMBERS`, so a 32-bit bitset is plenty.
    let mut seen: u64 = 0;
    for &idx in indices {
        if idx >= n {
            return Err(VaultError::FederationIndexOutOfRange);
        }
        let bit = 1u64 << idx;
        if seen & bit != 0 {
            return Err(VaultError::DuplicateFederationSigner);
        }
        seen |= bit;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_message_layout_matches_spec() {
        let asset_id: u32 = 0xDEAD_BEEF;
        let release_amount: u64 = 0x0102_0304_0506_0708;
        let recipient = [0x42u8; 32];
        let nonce: u64 = 0x1112_1314_1516_1718;

        let msg = build_release_message(asset_id, release_amount, &recipient, nonce);

        assert_eq!(msg.len(), 18 + 4 + 8 + 32 + 8);
        assert_eq!(&msg[0..18], RELEASE_DOMAIN);
        assert_eq!(&msg[18..22], &asset_id.to_le_bytes());
        assert_eq!(&msg[22..30], &release_amount.to_le_bytes());
        assert_eq!(&msg[30..62], &recipient);
        assert_eq!(&msg[62..70], &nonce.to_le_bytes());
    }

    #[test]
    fn release_message_domain_is_exact_ascii() {
        // Catch any "MAINNET_RELEASE" / "_V2" / casing slip with a hard-coded byte check.
        assert_eq!(RELEASE_DOMAIN, b"MAINNET_RELEASE_V1");
        assert_eq!(RELEASE_DOMAIN.len(), 18);
    }

    #[test]
    fn release_message_changes_for_each_field() {
        let recipient = [1u8; 32];
        let other_recipient = [2u8; 32];
        let base = build_release_message(1, 2, &recipient, 5);
        assert_ne!(base, build_release_message(99, 2, &recipient, 5));
        assert_ne!(base, build_release_message(1, 99, &recipient, 5));
        assert_ne!(base, build_release_message(1, 2, &other_recipient, 5));
        assert_ne!(base, build_release_message(1, 2, &recipient, 99));
    }

    #[test]
    fn release_domain_distinct_from_staccana_mint_domain() {
        // Catastrophic if a mint attestation could replay as a release attestation:
        // a federation signature on `STACCANA_MINT_V1 || ...` must not match
        // `MAINNET_RELEASE_V1 || ...`. Verify the prefixes literally cannot collide.
        const STACCANA_MINT_DOMAIN: &[u8] = b"STACCANA_MINT_V1";
        assert_ne!(RELEASE_DOMAIN, STACCANA_MINT_DOMAIN);
        assert_ne!(RELEASE_DOMAIN.len(), STACCANA_MINT_DOMAIN.len());
    }

    #[test]
    fn bps_fee_zero_is_passthrough() {
        assert_eq!(apply_bps_fee(1_000_000, 0), 1_000_000);
    }

    #[test]
    fn bps_fee_default_10_bps() {
        assert_eq!(apply_bps_fee(1_000_000, 10), 999_000);
    }

    #[test]
    fn bps_fee_handles_max_u64_no_overflow() {
        let result = apply_bps_fee(u64::MAX, 10);
        let expected = ((u64::MAX as u128) * 9_990 / 10_000) as u64;
        assert_eq!(result, expected);
    }

    #[test]
    fn bps_fee_full_take() {
        assert_eq!(apply_bps_fee(1_000_000, 10_000), 0);
    }

    #[test]
    fn unique_indices_accepts_distinct() {
        check_unique_indices(&[0, 1, 2, 3, 4], 9).unwrap();
    }

    #[test]
    fn unique_indices_rejects_duplicate() {
        let err = check_unique_indices(&[0, 1, 2, 1, 4], 9).unwrap_err();
        assert!(matches!(err, VaultError::DuplicateFederationSigner));
    }

    #[test]
    fn unique_indices_rejects_out_of_range() {
        let err = check_unique_indices(&[0, 1, 9, 3, 4], 9).unwrap_err();
        assert!(matches!(err, VaultError::FederationIndexOutOfRange));
    }

    #[test]
    fn unique_indices_empty_is_ok() {
        check_unique_indices(&[], 9).unwrap();
    }

    #[test]
    fn unique_indices_handles_full_population() {
        let all: Vec<u8> = (0u8..32).collect();
        check_unique_indices(&all, 32).unwrap();
    }

    #[test]
    fn chain_id_staccana_decodes_to_ascii_stac() {
        // CHAIN_ID_STACCANA = ASCII "stac" interpreted as u32 LE — i.e. bytes
        // 73 74 61 63 ('s' 't' 'a' 'c'). Verify the constant matches the documented
        // wire interpretation, since off-chain consumers pattern-match on this.
        let bytes = CHAIN_ID_STACCANA.to_le_bytes();
        assert_eq!(&bytes, b"stac");
    }

    // -- handler-equivalent verification simulations --------------------------
    //
    // The on-chain handler verifies, for each of M ed25519 precompile ixs:
    //   1. parsed.message == build_release_message(args)
    //   2. parsed.pubkey  == federation_set.members[indices[i]]
    //   3. indices have no dup, all in [0, n)
    //
    // These tests simulate steps (1) + (2) + (3) at the helper level, since (1) and
    // (2) are pure-data comparisons that can be exercised without a sysvar.

    /// Helper: produce M `(pubkey, message)` tuples as the federation precompile ixs
    /// would report them, for a given attestation.
    fn simulate_signed_messages(
        members: &[[u8; 32]],
        indices: &[u8],
        asset_id: u32,
        release_amount: u64,
        recipient: &[u8; 32],
        nonce: u64,
    ) -> Vec<([u8; 32], Vec<u8>)> {
        let msg = build_release_message(asset_id, release_amount, recipient, nonce);
        indices
            .iter()
            .map(|&i| (members[i as usize], msg.clone()))
            .collect()
    }

    /// Mirror of the per-precompile loop body in the handler.
    fn verify_attestation(
        members: &[[u8; 32]],
        n: u8,
        m: u8,
        indices: &[u8],
        asset_id: u32,
        release_amount: u64,
        recipient: &[u8; 32],
        nonce: u64,
        signed: &[([u8; 32], Vec<u8>)],
    ) -> Result<(), VaultError> {
        if indices.len() != m as usize {
            return Err(VaultError::InsufficientFederationSignatures);
        }
        check_unique_indices(indices, n)?;
        let expected = build_release_message(asset_id, release_amount, recipient, nonce);
        if signed.len() != m as usize {
            return Err(VaultError::InsufficientFederationSignatures);
        }
        for (i, &member_idx) in indices.iter().enumerate() {
            let (pk, msg) = &signed[i];
            if msg != &expected {
                return Err(VaultError::BadAttestationMessage);
            }
            if pk != &members[member_idx as usize] {
                return Err(VaultError::BadFederationSigner);
            }
        }
        Ok(())
    }

    /// Toy 5-of-9 federation pubkey set for the simulations.
    fn toy_federation() -> Vec<[u8; 32]> {
        (0u8..9).map(|i| [i; 32]).collect()
    }

    #[test]
    fn release_with_valid_m_of_n_sigs_succeeds() {
        // Happy path: 5 distinct members sign the canonical release message → verify
        // returns Ok(()). Mirrors the on-chain handler success path.
        let members = toy_federation();
        let recipient = [7u8; 32];
        let indices = [0u8, 1, 2, 3, 4];
        let signed = simulate_signed_messages(&members, &indices, 1, 1_000_000, &recipient, 42);
        verify_attestation(&members, 9, 5, &indices, 1, 1_000_000, &recipient, 42, &signed)
            .expect("5-of-9 verify must succeed");
    }

    #[test]
    fn release_with_insufficient_sigs_fails() {
        // Only 4 signers, threshold is 5 → InsufficientFederationSignatures. This is
        // the M-mismatch rejection the handler does at args parse time.
        let members = toy_federation();
        let recipient = [7u8; 32];
        let indices = [0u8, 1, 2, 3]; // 4, not 5
        let signed = simulate_signed_messages(&members, &indices, 1, 1_000_000, &recipient, 42);
        let err = verify_attestation(
            &members, 9, 5, &indices, 1, 1_000_000, &recipient, 42, &signed,
        )
        .unwrap_err();
        assert!(matches!(err, VaultError::InsufficientFederationSignatures));
    }

    #[test]
    fn release_with_unknown_signer_fails() {
        // Member at index 4 swapped out with a foreign pubkey → BadFederationSigner.
        // Models the case where a relayer attaches a precompile signed by a key that
        // is *not* in the registered federation set.
        let members = toy_federation();
        let recipient = [7u8; 32];
        let indices = [0u8, 1, 2, 3, 4];
        let mut signed =
            simulate_signed_messages(&members, &indices, 1, 1_000_000, &recipient, 42);
        signed[4].0 = [0xFFu8; 32]; // not in federation
        let err = verify_attestation(
            &members, 9, 5, &indices, 1, 1_000_000, &recipient, 42, &signed,
        )
        .unwrap_err();
        assert!(matches!(err, VaultError::BadFederationSigner));
    }

    #[test]
    fn release_with_wrong_asset_attestation_rejected() {
        // Federation signed an attestation for asset_id=2, but the relayer submitted
        // it claiming asset_id=1. The expected message changes → BadAttestationMessage.
        // This is the cross-asset replay protection: a stSOL release attestation can't
        // drain a wSOL vault and vice-versa.
        let members = toy_federation();
        let recipient = [7u8; 32];
        let indices = [0u8, 1, 2, 3, 4];

        // Simulate sigs over asset_id=2 ...
        let signed_for_asset_2 =
            simulate_signed_messages(&members, &indices, 2, 1_000_000, &recipient, 42);

        // ... but verify against asset_id=1.
        let err = verify_attestation(
            &members,
            9,
            5,
            &indices,
            1, // different asset!
            1_000_000,
            &recipient,
            42,
            &signed_for_asset_2,
        )
        .unwrap_err();
        assert!(matches!(err, VaultError::BadAttestationMessage));
    }

    #[test]
    fn release_with_wrong_recipient_rejected() {
        // Federation signed for one recipient; relayer swaps in a different one →
        // BadAttestationMessage. This is the man-in-the-middle protection on the
        // release destination.
        let members = toy_federation();
        let signed_recipient = [7u8; 32];
        let attacker_recipient = [9u8; 32];
        let indices = [0u8, 1, 2, 3, 4];
        let signed = simulate_signed_messages(
            &members,
            &indices,
            1,
            1_000_000,
            &signed_recipient,
            42,
        );
        let err = verify_attestation(
            &members,
            9,
            5,
            &indices,
            1,
            1_000_000,
            &attacker_recipient,
            42,
            &signed,
        )
        .unwrap_err();
        assert!(matches!(err, VaultError::BadAttestationMessage));
    }

    #[test]
    fn release_with_duplicate_signer_rejected() {
        // Same member signs twice — even with otherwise-valid signatures, the dedup
        // check rejects with DuplicateFederationSigner.
        let members = toy_federation();
        let recipient = [7u8; 32];
        let indices = [0u8, 1, 2, 3, 1]; // index 1 used twice
        let signed = simulate_signed_messages(&members, &indices, 1, 1_000_000, &recipient, 42);
        let err = verify_attestation(
            &members, 9, 5, &indices, 1, 1_000_000, &recipient, 42, &signed,
        )
        .unwrap_err();
        assert!(matches!(err, VaultError::DuplicateFederationSigner));
    }

    // -- replay attack simulation --------------------------------------------

    /// Simulates the on-chain `nonce_out` PDA — once a (asset_id, nonce) pair is
    /// consumed, it cannot be consumed again. The on-chain enforcement is via Anchor
    /// `init` failing on PDA collision; this test mirrors that semantic at the
    /// data-structure level.
    #[derive(Default)]
    struct ConsumedNonces(std::collections::HashSet<(u32, u64)>);

    impl ConsumedNonces {
        fn try_consume(&mut self, asset_id: u32, nonce: u64) -> Result<(), VaultError> {
            if !self.0.insert((asset_id, nonce)) {
                return Err(VaultError::NonceAlreadyConsumed);
            }
            Ok(())
        }
    }

    #[test]
    fn replay_attack_same_nonce_twice_rejected() {
        // First release with (asset_id, nonce) = (1, 42) consumes the marker.
        // Second release with the same tuple, even with valid sigs, must fail:
        // NonceAlreadyConsumed. Models the Anchor `init` revert.
        let mut consumed = ConsumedNonces::default();
        consumed.try_consume(1, 42).expect("first consume ok");
        let err = consumed.try_consume(1, 42).unwrap_err();
        assert!(matches!(err, VaultError::NonceAlreadyConsumed));
    }

    #[test]
    fn distinct_nonces_for_same_asset_both_consume() {
        // Two distinct nonces for the same asset should each consume independently —
        // sanity-check that the PDA derivation per-(asset, nonce) doesn't collapse.
        let mut consumed = ConsumedNonces::default();
        consumed.try_consume(1, 42).expect("nonce 42 ok");
        consumed.try_consume(1, 43).expect("nonce 43 ok");
    }

    #[test]
    fn same_nonce_distinct_assets_both_consume() {
        // Same nonce value for different assets must NOT collide — each (asset, nonce)
        // tuple has its own marker PDA (seeds: ["nonce_out", asset_id_le, nonce_le]).
        let mut consumed = ConsumedNonces::default();
        consumed.try_consume(1, 42).expect("(1,42) ok");
        consumed.try_consume(2, 42).expect("(2,42) ok");
    }

    // -- deposit-side nonce monotonicity --------------------------------------

    /// Mirrors the per-asset deposit nonce counter in `NonceInCounter`. The handler
    /// reads-then-increments; this test confirms the intended semantic (each deposit
    /// emits a strictly-increasing nonce, starting at 0).
    #[derive(Default)]
    struct DepositCounter {
        next: u64,
    }

    impl DepositCounter {
        fn allocate(&mut self) -> Result<u64, VaultError> {
            let assigned = self.next;
            self.next = self
                .next
                .checked_add(1)
                .ok_or(VaultError::BadInstructionData)?;
            Ok(assigned)
        }
    }

    #[test]
    fn deposit_increments_nonce_starting_at_zero() {
        // First deposit must emit nonce=0; second nonce=1; etc. The staccana-side
        // mint expects this monotone sequence to chain attestations.
        let mut counter = DepositCounter::default();
        assert_eq!(counter.allocate().unwrap(), 0);
        assert_eq!(counter.allocate().unwrap(), 1);
        assert_eq!(counter.allocate().unwrap(), 2);
        assert_eq!(counter.next, 3);
    }
}

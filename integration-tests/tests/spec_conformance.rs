//! SPEC.md byte-format conformance tests.
//!
//! Each test below pins ONE wire format from `docs/SPEC.md` against a fixture with known
//! expected bytes. Failing any of these tests means a consensus break — every validator
//! must agree byte-for-byte on these formats.
//!
//! Coverage:
//!
//! * §4.1 — `ClaimArgs` instruction body layout
//! * §4.2 — claim message signed by the mainnet keypair
//! * §5.3 — federation ratio attestation
//! * §6.1 — `SwapIntent` canonical encoding
//!
//! The existing per-crate unit tests (in `claim-cli`, `lazy-claim`, `federation-attestor`,
//! and `matcher`) cover layout pinning within their own crate. These tests cross the
//! boundary: the bytes a producer crate emits must equal the bytes the consumer crate
//! expects, and both must equal the literal bytes the spec describes.

use solana_program::pubkey::Pubkey;
use staccana_claim_cli::tx::{
    build_claim_message as cli_build_claim_message, ClaimArgs as CliClaimArgs,
    LAZY_CLAIM_PROGRAM_ID, STACCANA_CLAIM_DOMAIN,
};
use staccana_federation_attestor::sign::{
    build_attestation_message, AttestationInputs, ATTESTATION_DOMAIN, ATTESTATION_LEN,
};
use staccana_lazy_claim::{build_claim_message as program_build_claim_message, ClaimArgs};
use staccana_matcher::SwapIntent;

// ──────────────────────────────────────────────────────────────────────────────
// §4.2 — Claim message
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn claim_message_matches_spec_4_2_byte_for_byte() {
    // Fixture from the task spec: pubkey [0x01; 32], lamports 1000, program_id [0x02; 32].
    let pubkey = [0x01u8; 32];
    let lamports: u64 = 1000;
    let program_id = Pubkey::new_from_array([0x02u8; 32]);

    let mut expected = Vec::with_capacity(17 + 32 + 8 + 32);
    expected.extend_from_slice(b"STACCANA_CLAIM_V1");
    expected.extend_from_slice(&pubkey);
    expected.extend_from_slice(&lamports.to_le_bytes());
    expected.extend_from_slice(program_id.as_ref());

    let from_program = program_build_claim_message(&pubkey, lamports, &program_id);
    assert_eq!(
        from_program, expected,
        "lazy-claim message disagrees with spec"
    );
}

#[test]
fn claim_message_matches_between_lazy_claim_and_claim_cli_for_canonical_program_id() {
    // The claim-cli builds the message against `LAZY_CLAIM_PROGRAM_ID` (currently a
    // placeholder). The lazy-claim program builds the message against whatever program_id
    // the validator passes in. For the placeholder, both must agree.
    let pubkey = Pubkey::new_from_array([0x42u8; 32]);
    let lamports: u64 = 1_234_567_890;

    let cli_msg = cli_build_claim_message(&pubkey, lamports);
    let program_msg =
        program_build_claim_message(&pubkey.to_bytes(), lamports, &LAZY_CLAIM_PROGRAM_ID);
    assert_eq!(cli_msg, program_msg);
}

#[test]
fn claim_message_domain_is_exactly_seventeen_bytes_no_nul() {
    // Pinning the domain length and content prevents accidental NUL-termination drift
    // (which would shift every numeric field by one byte).
    assert_eq!(STACCANA_CLAIM_DOMAIN, b"STACCANA_CLAIM_V1");
    assert_eq!(STACCANA_CLAIM_DOMAIN.len(), 17);
}

// ──────────────────────────────────────────────────────────────────────────────
// §4.1 — ClaimArgs instruction body
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn claim_args_layout_matches_spec_4_1() {
    // Fixture: known proof_len=2, two distinct sibling hashes, proof_flags = 0b10
    // (level 0 sibling on left, level 1 sibling on right).
    let pubkey = [0xAAu8; 32];
    let lamports: u64 = 0x0102_0304_0506_0708;
    let proof_0 = [0x11u8; 32];
    let proof_1 = [0x22u8; 32];
    let proof_flags: Vec<u8> = vec![0b0000_0010];

    let args = ClaimArgs {
        pubkey,
        lamports,
        proof: vec![proof_0, proof_1],
        proof_flags: proof_flags.clone(),
    };
    let serialized = args.to_ix_data().expect("serialize");

    // Layout per SPEC §4.1 (with the ix discriminator prefix that the on-chain handler
    // requires):
    //   [0]            discriminator = 0x00
    //   [1..33]        pubkey (32 bytes)
    //   [33..41]       lamports (LE u64)
    //   [41..43]       proof_len (LE u16) = 2
    //   [43..75]       proof[0]
    //   [75..107]      proof[1]
    //   [107..108]     proof_flags = 0b0000_0010
    let mut expected = Vec::new();
    expected.push(0x00);
    expected.extend_from_slice(&pubkey);
    expected.extend_from_slice(&lamports.to_le_bytes());
    expected.extend_from_slice(&2u16.to_le_bytes());
    expected.extend_from_slice(&proof_0);
    expected.extend_from_slice(&proof_1);
    expected.extend_from_slice(&proof_flags);

    assert_eq!(
        serialized, expected,
        "lazy-claim ClaimArgs does not match spec"
    );
    assert_eq!(serialized.len(), 1 + 32 + 8 + 2 + 64 + 1);
}

#[test]
fn claim_args_zero_proof_layout_is_minimal() {
    // The spec leaves room for a single-leaf tree where the proof is empty. The encoded
    // body must be exactly pubkey || lamports || proof_len(=0).
    let args = ClaimArgs {
        pubkey: [0xFEu8; 32],
        lamports: 7,
        proof: vec![],
        proof_flags: vec![],
    };
    let serialized = args.to_ix_data().expect("serialize");
    let mut expected = Vec::new();
    expected.push(0x00);
    expected.extend_from_slice(&[0xFEu8; 32]);
    expected.extend_from_slice(&7u64.to_le_bytes());
    expected.extend_from_slice(&0u16.to_le_bytes());
    assert_eq!(serialized, expected);
    assert_eq!(serialized.len(), 1 + 32 + 8 + 2);
}

#[test]
fn claim_args_round_trip_via_lazy_claim_decoder() {
    // Encoding with one crate and decoding with another (here: same crate, but going
    // through the to_ix_data → from_ix_data path) catches asymmetries between encoder
    // and decoder. The claim-cli's `tx::ClaimArgs` produces wire bytes that the
    // lazy-claim's `ClaimArgs::from_ix_data` must accept.
    let cli_args = CliClaimArgs {
        pubkey: [0x33u8; 32],
        lamports: 9_876_543_210,
        proof: vec![[0x44u8; 32]; 5],
        proof_flags: vec![0b0001_0101], // 5 bits: ⌈5/8⌉ = 1 byte
    };
    let mut data_with_disc = Vec::with_capacity(1 + cli_args.to_wire_bytes().unwrap().len());
    data_with_disc.push(0x00); // ix discriminator
    data_with_disc.extend_from_slice(&cli_args.to_wire_bytes().unwrap());

    let decoded = ClaimArgs::from_ix_data(&data_with_disc).expect("decode");
    assert_eq!(decoded.pubkey, cli_args.pubkey);
    assert_eq!(decoded.lamports, cli_args.lamports);
    assert_eq!(decoded.proof, cli_args.proof);
    assert_eq!(decoded.proof_flags, cli_args.proof_flags);
}

// ──────────────────────────────────────────────────────────────────────────────
// §5.3 — Ratio attestation
// ──────────────────────────────────────────────────────────────────────────────

#[test]
fn ratio_attestation_matches_spec_5_3_byte_for_byte() {
    // Fixture from the task spec: asset_id 1, vault_value 1_000_000, mint_supply
    // 1_000_000, slot 100, nonce 5.
    let inputs = AttestationInputs {
        asset_id: 1,
        vault_value: 1_000_000,
        mint_supply: 1_000_000,
        slot: 100,
        nonce: 5,
    };

    let mut expected = Vec::with_capacity(ATTESTATION_LEN);
    expected.extend_from_slice(b"STACCANA_RATIO_V1");
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.extend_from_slice(&1_000_000u64.to_le_bytes());
    expected.extend_from_slice(&1_000_000u64.to_le_bytes());
    expected.extend_from_slice(&100u64.to_le_bytes());
    expected.extend_from_slice(&5u64.to_le_bytes());

    let actual = build_attestation_message(inputs);
    assert_eq!(&actual[..], expected.as_slice());
    assert_eq!(actual.len(), 53);
}

#[test]
fn ratio_attestation_domain_constant_matches_spec_literal() {
    assert_eq!(ATTESTATION_DOMAIN, b"STACCANA_RATIO_V1");
    assert_eq!(ATTESTATION_DOMAIN.len(), 17);
    assert_eq!(ATTESTATION_LEN, 53);
}

#[test]
fn ratio_attestation_field_offsets_pinned() {
    // Independent fixture pinning offsets within the 53-byte message. Catches a
    // field-reorder bug that the basic byte-for-byte test would also catch but with a
    // less actionable diff.
    let inputs = AttestationInputs {
        asset_id: 0xDEAD_BEEFu32,
        vault_value: 0x0102_0304_0506_0708u64,
        mint_supply: 0x1112_1314_1516_1718u64,
        slot: 0x2122_2324_2526_2728u64,
        nonce: 0x3132_3334_3536_3738u64,
    };
    let msg = build_attestation_message(inputs);
    assert_eq!(&msg[..17], b"STACCANA_RATIO_V1");
    assert_eq!(&msg[17..21], &0xDEAD_BEEFu32.to_le_bytes());
    assert_eq!(&msg[21..29], &0x0102_0304_0506_0708u64.to_le_bytes());
    assert_eq!(&msg[29..37], &0x1112_1314_1516_1718u64.to_le_bytes());
    assert_eq!(&msg[37..45], &0x2122_2324_2526_2728u64.to_le_bytes());
    assert_eq!(&msg[45..53], &0x3132_3334_3536_3738u64.to_le_bytes());
}

// ──────────────────────────────────────────────────────────────────────────────
// §6.1 — SwapIntent canonical encoding
// ──────────────────────────────────────────────────────────────────────────────

/// SPEC §6.1 canonical encoding: `signer || in_mint || in_amount_le || out_mint ||
/// min_out_le || nonce_le`.
///
/// The matcher crate doesn't expose this canonical encoder (it only consumes
/// `SwapIntent`s by value), so we rebuild the canonical bytes here from the published
/// field order. The point of this test is to lock the *layout* — if the matcher ever
/// derives or exposes a `to_canonical_bytes`, this test should swap to call it directly
/// and the fixture remains the lock.
fn canonical_encode(intent: &SwapIntent) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + 32 + 8 + 32 + 8 + 8);
    out.extend_from_slice(intent.signer.as_ref());
    out.extend_from_slice(intent.in_mint.as_ref());
    out.extend_from_slice(&intent.in_amount.to_le_bytes());
    out.extend_from_slice(intent.out_mint.as_ref());
    out.extend_from_slice(&intent.min_out.to_le_bytes());
    out.extend_from_slice(&intent.nonce.to_le_bytes());
    out
}

#[test]
fn swap_intent_canonical_encoding_matches_spec_6_1() {
    // Fixture: every field set to a recognizable byte pattern so the encoder's offsets
    // are easy to read in a hex dump.
    let intent = SwapIntent {
        signer: Pubkey::new_from_array([0x10u8; 32]),
        in_mint: Pubkey::new_from_array([0x20u8; 32]),
        in_amount: 0x0102_0304_0506_0708u64,
        out_mint: Pubkey::new_from_array([0x30u8; 32]),
        min_out: 0x1112_1314_1516_1718u64,
        nonce: 0x2122_2324_2526_2728u64,
    };
    let bytes = canonical_encode(&intent);
    assert_eq!(bytes.len(), 32 + 32 + 8 + 32 + 8 + 8);
    assert_eq!(&bytes[0..32], &[0x10u8; 32]);
    assert_eq!(&bytes[32..64], &[0x20u8; 32]);
    assert_eq!(&bytes[64..72], &0x0102_0304_0506_0708u64.to_le_bytes());
    assert_eq!(&bytes[72..104], &[0x30u8; 32]);
    assert_eq!(&bytes[104..112], &0x1112_1314_1516_1718u64.to_le_bytes());
    assert_eq!(&bytes[112..120], &0x2122_2324_2526_2728u64.to_le_bytes());
}

#[test]
fn swap_intent_zero_fields_encode_as_all_zeros() {
    // A zero-field intent should encode to exactly 120 zero bytes — confirms there is no
    // hidden discriminator, length prefix, or padding inserted by the canonical encoder.
    let intent = SwapIntent {
        signer: Pubkey::default(),
        in_mint: Pubkey::default(),
        in_amount: 0,
        out_mint: Pubkey::default(),
        min_out: 0,
        nonce: 0,
    };
    let bytes = canonical_encode(&intent);
    assert_eq!(bytes.len(), 120);
    assert!(bytes.iter().all(|&b| b == 0));
}

#[test]
fn swap_intent_each_field_contributes_to_encoding() {
    // Catch field-drop bugs: bumping each field by one must change the bytes.
    let base = SwapIntent {
        signer: Pubkey::new_from_array([1u8; 32]),
        in_mint: Pubkey::new_from_array([2u8; 32]),
        in_amount: 100,
        out_mint: Pubkey::new_from_array([3u8; 32]),
        min_out: 50,
        nonce: 1,
    };
    let base_bytes = canonical_encode(&base);

    let mut bumped = base.clone();
    bumped.in_amount += 1;
    assert_ne!(canonical_encode(&bumped), base_bytes);

    let mut bumped = base.clone();
    bumped.min_out += 1;
    assert_ne!(canonical_encode(&bumped), base_bytes);

    let mut bumped = base.clone();
    bumped.nonce += 1;
    assert_ne!(canonical_encode(&bumped), base_bytes);

    let mut bumped = base.clone();
    bumped.signer = Pubkey::new_from_array([99u8; 32]);
    assert_ne!(canonical_encode(&bumped), base_bytes);
}

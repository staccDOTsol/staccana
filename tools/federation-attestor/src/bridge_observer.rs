//! Cross-chain event observer for the deposit/burn → mint/release bridge flow.
//!
//! Polls each chain's RPC for new transactions touching the bridge program / vault
//! program, parses Anchor `emit!` log lines (`Program data: <base64>`) for the relevant
//! event discriminators, and yields strongly-typed [`DepositEvent`] / [`BurnEvent`]
//! values for the daemon to attest.
//!
//! ## Why polling, not WebSocket
//!
//! For v1 we deliberately use `getSignaturesForAddress` polling instead of
//! `logsSubscribe`: WebSocket subscriptions drop on validator restart and silently lose
//! events, requiring a backfill path anyway. With polling, a single restart-safe loop
//! handles both steady-state and recovery — the `last_seen_signature` cursor is the
//! only state. Tradeoff: ~5s minimum latency vs sub-second; for a federation attestor
//! this is well within the human-operator-visible budget.
//!
//! ## Anchor event log format
//!
//! When an Anchor program calls `emit!(MyEvent { ... })`, the runtime writes one log
//! line of the form `Program data: <base64>` whose decoded body is:
//!
//! ```text
//! [0..8]  event discriminator = sha256("event:<EventName>")[..8]
//! [8..]   borsh-serialized event struct
//! ```
//!
//! We hardcode the discriminators for the two events we care about
//! ([`DEPOSIT_EVENT_DISCRIMINATOR`], [`BURN_EVENT_DISCRIMINATOR`]) so this crate
//! doesn't need a heavy dep on the bridge / vault crates' IDLs at runtime.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use solana_sdk::pubkey::Pubkey;

/// Anchor event discriminator for `bridge_vault::DepositEvent`. Computed as
/// `sha256("event:DepositEvent")[..8]`. Verified by the unit tests in this module.
pub const DEPOSIT_EVENT_DISCRIMINATOR: [u8; 8] =
    [120, 248, 61, 83, 31, 142, 107, 144];

/// Anchor event discriminator for `bridge::BurnEvent`. Computed as
/// `sha256("event:BurnEvent")[..8]`. Verified by the unit tests in this module.
pub const BURN_EVENT_DISCRIMINATOR: [u8; 8] =
    [33, 89, 47, 117, 82, 124, 238, 250];

/// `Deposit` event observed on the mainnet/devnet bridge-vault. Mirrors
/// `programs/bridge-vault/src/instructions/deposit.rs::DepositEvent` field-by-field
/// (matching borsh encoding order).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepositEvent {
    pub asset_id: u32,
    pub user: Pubkey,
    pub amount: u64,
    pub amount_after_fee: u64,
    pub dest: [u8; 32],
    pub nonce: u64,
    pub chain_id: u32,
    /// The transaction signature this event was extracted from. Used by the daemon
    /// to advance the `last_seen_signature` cursor without re-parsing.
    pub source_signature: String,
}

/// `Burn` event observed on the staccana bridge program. Mirrors
/// `programs/bridge/src/instructions/burn.rs::BurnEvent`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BurnEvent {
    pub asset_id: u32,
    pub user: Pubkey,
    pub amount: u64,
    pub gross_release: u64,
    pub net_release: u64,
    pub r_q64: u128,
    pub mainnet_dest: [u8; 32],
    pub nonce_out: u64,
    pub chain_id: u32,
    pub source_signature: String,
}

/// Errors surfacing from log parsing. RPC-level errors are not mapped here — the
/// daemon's RPC poll loop wraps them in `anyhow::Error` directly.
#[derive(Debug, thiserror::Error)]
pub enum LogParseError {
    #[error("base64 decode failed: {0}")]
    Base64(String),

    #[error("event payload too short ({len} bytes, need >= 8 for discriminator)")]
    PayloadTooShort { len: usize },

    #[error("borsh decode failed: {0}")]
    Borsh(String),
}

/// Decode a single `Program data: <base64>` log line into the raw event bytes
/// (discriminator + body). Returns `Ok(None)` if the line isn't an Anchor event line.
///
/// Solana validator log lines come through `solana_client::rpc_response::RpcLogsResponse`
/// (or the `logs` field of `getTransaction`) without the `Program log: ` /
/// `Program data: ` prefix mapping into a structured field — the prefix is part of the
/// line text. We pattern-match on `"Program data: "` and base64-decode the rest.
pub fn decode_program_data_line(line: &str) -> Result<Option<Vec<u8>>, LogParseError> {
    const PREFIX: &str = "Program data: ";
    let Some(b64) = line.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    let bytes = base64_decode(b64.trim()).map_err(|e| LogParseError::Base64(e.to_string()))?;
    Ok(Some(bytes))
}

/// Try to parse a `DepositEvent` from a raw Anchor event payload (discriminator + body).
/// Returns `Ok(None)` if the discriminator doesn't match. `Err` only on a malformed
/// body — i.e. the discriminator matched but borsh decoding failed.
pub fn try_parse_deposit_event(
    payload: &[u8],
    source_signature: &str,
) -> Result<Option<DepositEvent>, LogParseError> {
    if payload.len() < 8 {
        return Err(LogParseError::PayloadTooShort { len: payload.len() });
    }
    if payload[..8] != DEPOSIT_EVENT_DISCRIMINATOR {
        return Ok(None);
    }
    let body = &payload[8..];
    // Borsh layout: u32 || Pubkey(32) || u64 || u64 || [u8;32] || u64 || u32 = 96 bytes
    let expected_len = 4 + 32 + 8 + 8 + 32 + 8 + 4;
    if body.len() < expected_len {
        return Err(LogParseError::Borsh(format!(
            "deposit body too short: got {}, need {}",
            body.len(),
            expected_len
        )));
    }
    let mut off = 0;
    let asset_id = u32::from_le_bytes(read_array::<4>(body, &mut off));
    let user = Pubkey::new_from_array(read_array::<32>(body, &mut off));
    let amount = u64::from_le_bytes(read_array::<8>(body, &mut off));
    let amount_after_fee = u64::from_le_bytes(read_array::<8>(body, &mut off));
    let dest = read_array::<32>(body, &mut off);
    let nonce = u64::from_le_bytes(read_array::<8>(body, &mut off));
    let chain_id = u32::from_le_bytes(read_array::<4>(body, &mut off));
    debug_assert_eq!(off, expected_len);
    Ok(Some(DepositEvent {
        asset_id,
        user,
        amount,
        amount_after_fee,
        dest,
        nonce,
        chain_id,
        source_signature: source_signature.to_string(),
    }))
}

/// Try to parse a `BurnEvent` from a raw Anchor event payload.
pub fn try_parse_burn_event(
    payload: &[u8],
    source_signature: &str,
) -> Result<Option<BurnEvent>, LogParseError> {
    if payload.len() < 8 {
        return Err(LogParseError::PayloadTooShort { len: payload.len() });
    }
    if payload[..8] != BURN_EVENT_DISCRIMINATOR {
        return Ok(None);
    }
    let body = &payload[8..];
    // Borsh layout: u32 || Pubkey(32) || u64 || u64 || u64 || u128 || [u8;32] || u64 || u32
    let expected_len = 4 + 32 + 8 + 8 + 8 + 16 + 32 + 8 + 4;
    if body.len() < expected_len {
        return Err(LogParseError::Borsh(format!(
            "burn body too short: got {}, need {}",
            body.len(),
            expected_len
        )));
    }
    let mut off = 0;
    let asset_id = u32::from_le_bytes(read_array::<4>(body, &mut off));
    let user = Pubkey::new_from_array(read_array::<32>(body, &mut off));
    let amount = u64::from_le_bytes(read_array::<8>(body, &mut off));
    let gross_release = u64::from_le_bytes(read_array::<8>(body, &mut off));
    let net_release = u64::from_le_bytes(read_array::<8>(body, &mut off));
    let r_q64 = u128::from_le_bytes(read_array::<16>(body, &mut off));
    let mainnet_dest = read_array::<32>(body, &mut off);
    let nonce_out = u64::from_le_bytes(read_array::<8>(body, &mut off));
    let chain_id = u32::from_le_bytes(read_array::<4>(body, &mut off));
    debug_assert_eq!(off, expected_len);
    Ok(Some(BurnEvent {
        asset_id,
        user,
        amount,
        gross_release,
        net_release,
        r_q64,
        mainnet_dest,
        nonce_out,
        chain_id,
        source_signature: source_signature.to_string(),
    }))
}

/// Walk a transaction's full log lines and yield every `DepositEvent` it contains.
/// A single tx can carry multiple deposit events if a CPI batched them; the daemon
/// processes each independently.
pub fn extract_deposit_events(logs: &[String], source_signature: &str) -> Vec<DepositEvent> {
    let mut out = Vec::new();
    for line in logs {
        match decode_program_data_line(line) {
            Ok(Some(payload)) => match try_parse_deposit_event(&payload, source_signature) {
                Ok(Some(ev)) => out.push(ev),
                Ok(None) => {} // discriminator didn't match; not a deposit
                Err(_) => {}   // malformed body; skip silently
            },
            Ok(None) => {} // not a `Program data:` line
            Err(_) => {}    // malformed base64; skip
        }
    }
    out
}

/// Mirror of [`extract_deposit_events`] for `BurnEvent`.
pub fn extract_burn_events(logs: &[String], source_signature: &str) -> Vec<BurnEvent> {
    let mut out = Vec::new();
    for line in logs {
        if let Ok(Some(payload)) = decode_program_data_line(line) {
            if let Ok(Some(ev)) = try_parse_burn_event(&payload, source_signature) {
                out.push(ev);
            }
        }
    }
    out
}

// --- internal helpers ------------------------------------------------------

fn read_array<const N: usize>(buf: &[u8], off: &mut usize) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(&buf[*off..*off + N]);
    *off += N;
    out
}

/// Pure-Rust base64 decode (RFC 4648, no padding tolerance — Solana log lines always
/// pad). Avoids pulling in the `base64` crate just for this one call site.
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    // Standard alphabet table.
    const TABLE: [i8; 256] = {
        let mut t = [-1i8; 256];
        let mut i = 0u8;
        while i < 26 {
            t[(b'A' + i) as usize] = i as i8;
            t[(b'a' + i) as usize] = (i + 26) as i8;
            i += 1;
        }
        let mut j = 0u8;
        while j < 10 {
            t[(b'0' + j) as usize] = (j + 52) as i8;
            j += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };

    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut nbits: u32 = 0;
    for &c in bytes {
        if c == b'=' {
            break;
        }
        let v = TABLE[c as usize];
        if v < 0 {
            return Err(anyhow!("invalid base64 character: {:?}", c as char));
        }
        buf = (buf << 6) | (v as u32);
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push(((buf >> nbits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

// --- RPC plumbing ----------------------------------------------------------

/// Minimal JSON-RPC client trait — abstracts over `solana_client::rpc_client::RpcClient`
/// so the daemon's polling loop is testable without spinning up a validator.
///
/// The daemon implementation backs onto `RpcClient::get_signatures_for_address` +
/// `RpcClient::get_transaction_with_config(... UiTransactionEncoding::Json)` and pulls
/// the `meta.log_messages` field. For the v1 ship we keep this trait + a thin wrapper
/// so unit tests can plug in a mock; the real daemon uses [`SolanaRpcClient`].
pub trait BridgeRpcClient {
    /// Fetch up to `limit` signatures for `address` newer than `until` (the cursor).
    /// Result MUST be returned oldest-first so the daemon processes them in causal
    /// order. RPC's native `getSignaturesForAddress` returns newest-first; the impl
    /// reverses the slice.
    fn signatures_for_address_since(
        &self,
        address: &Pubkey,
        until: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>>;

    /// Fetch the `log_messages` slice for one transaction. Returns `None` if the
    /// transaction has no logs (failed before any program ran) or doesn't exist.
    fn transaction_logs(&self, signature: &str) -> Result<Option<Vec<String>>>;
}

/// Thin `solana_client::rpc_client::RpcClient` adapter implementing
/// [`BridgeRpcClient`]. Constructed from an HTTP RPC URL.
///
/// The daemon doesn't reach for `solana-pubsub-client` (WebSocket) intentionally —
/// see the module-level doc comment.
pub struct SolanaRpcClient {
    inner: solana_client::rpc_client::RpcClient,
}

impl SolanaRpcClient {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            inner: solana_client::rpc_client::RpcClient::new(url.into()),
        }
    }
}

impl BridgeRpcClient for SolanaRpcClient {
    fn signatures_for_address_since(
        &self,
        address: &Pubkey,
        until: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
        let cfg = GetConfirmedSignaturesForAddress2Config {
            before: None,
            until: until.map(|s| s.parse()).transpose().context("parse until-sig")?,
            limit: Some(limit),
            commitment: Some(solana_sdk::commitment_config::CommitmentConfig::finalized()),
        };
        let mut sigs = self
            .inner
            .get_signatures_for_address_with_config(address, cfg)
            .context("get_signatures_for_address")?;
        // RPC returns newest-first; daemon wants oldest-first.
        sigs.reverse();
        Ok(sigs.into_iter().map(|s| s.signature).collect())
    }

    fn transaction_logs(&self, signature: &str) -> Result<Option<Vec<String>>> {
        use solana_transaction_status_client_types::UiTransactionEncoding;
        let sig = signature.parse().context("parse tx signature")?;
        let tx = self
            .inner
            .get_transaction(&sig, UiTransactionEncoding::Json)
            .context("get_transaction")?;
        Ok(tx.transaction.meta.and_then(|m| {
            // Anchor 1.x logs land in `log_messages`. The Option wrapping comes from
            // `OptionSerializer<Vec<String>>` in `solana-transaction-status`; we
            // collapse `Skip / None / Some(empty)` all to None for the caller.
            let opt: Option<Vec<String>> = m.log_messages.into();
            opt.filter(|l| !l.is_empty())
        }))
    }
}

/// Minimal JSON-on-disk shape for parsing log lines from a `getTransaction` response —
/// useful for offline replay / log-file ingestion in test environments.
#[derive(Debug, Deserialize)]
pub struct OfflineTxLogs {
    pub signature: String,
    pub log_messages: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recompute the discriminator from `event:<Name>` and confirm our hard-coded
    /// constants match. This pins the on-chain-equivalent identifier so an Anchor
    /// emit-name change is caught immediately.
    fn anchor_event_disc(event_name: &str) -> [u8; 8] {
        // sha256(b"event:<Name>")[..8] — the Anchor convention (see anchor-attribute-event).
        use solana_program::hash::hashv;
        let preimage = format!("event:{event_name}");
        let h = hashv(&[preimage.as_bytes()]);
        let mut out = [0u8; 8];
        out.copy_from_slice(&h.as_ref()[..8]);
        out
    }

    #[test]
    fn deposit_discriminator_matches_anchor_convention() {
        assert_eq!(
            DEPOSIT_EVENT_DISCRIMINATOR,
            anchor_event_disc("DepositEvent")
        );
    }

    #[test]
    fn burn_discriminator_matches_anchor_convention() {
        assert_eq!(BURN_EVENT_DISCRIMINATOR, anchor_event_disc("BurnEvent"));
    }

    /// Encode a tiny synthetic `DepositEvent` payload exactly as Anchor's borsh
    /// emitter would, then confirm the parser reconstructs the original.
    #[test]
    fn deposit_event_round_trip_through_borsh() {
        let asset_id: u32 = 1;
        let user = Pubkey::new_from_array([0xAAu8; 32]);
        let amount: u64 = 1_000_000;
        let amount_after_fee: u64 = 999_000;
        let dest = [0xBBu8; 32];
        let nonce: u64 = 7;
        let chain_id: u32 = 0x6361_7473;

        let mut payload = Vec::new();
        payload.extend_from_slice(&DEPOSIT_EVENT_DISCRIMINATOR);
        payload.extend_from_slice(&asset_id.to_le_bytes());
        payload.extend_from_slice(user.as_ref());
        payload.extend_from_slice(&amount.to_le_bytes());
        payload.extend_from_slice(&amount_after_fee.to_le_bytes());
        payload.extend_from_slice(&dest);
        payload.extend_from_slice(&nonce.to_le_bytes());
        payload.extend_from_slice(&chain_id.to_le_bytes());

        let parsed = try_parse_deposit_event(&payload, "sig123")
            .unwrap()
            .expect("discriminator should match");
        assert_eq!(parsed.asset_id, asset_id);
        assert_eq!(parsed.user, user);
        assert_eq!(parsed.amount, amount);
        assert_eq!(parsed.amount_after_fee, amount_after_fee);
        assert_eq!(parsed.dest, dest);
        assert_eq!(parsed.nonce, nonce);
        assert_eq!(parsed.chain_id, chain_id);
        assert_eq!(parsed.source_signature, "sig123");
    }

    #[test]
    fn burn_event_round_trip_through_borsh() {
        let asset_id: u32 = 7;
        let user = Pubkey::new_from_array([1u8; 32]);
        let amount: u64 = 500_000;
        let gross_release: u64 = 750_000;
        let net_release: u64 = 749_250;
        let r_q64: u128 = 1u128 << 64;
        let mainnet_dest = [9u8; 32];
        let nonce_out: u64 = 42;
        let chain_id: u32 = 0x6D61_696E;

        let mut payload = Vec::new();
        payload.extend_from_slice(&BURN_EVENT_DISCRIMINATOR);
        payload.extend_from_slice(&asset_id.to_le_bytes());
        payload.extend_from_slice(user.as_ref());
        payload.extend_from_slice(&amount.to_le_bytes());
        payload.extend_from_slice(&gross_release.to_le_bytes());
        payload.extend_from_slice(&net_release.to_le_bytes());
        payload.extend_from_slice(&r_q64.to_le_bytes());
        payload.extend_from_slice(&mainnet_dest);
        payload.extend_from_slice(&nonce_out.to_le_bytes());
        payload.extend_from_slice(&chain_id.to_le_bytes());

        let parsed = try_parse_burn_event(&payload, "sig777")
            .unwrap()
            .expect("discriminator should match");
        assert_eq!(parsed.asset_id, asset_id);
        assert_eq!(parsed.amount, amount);
        assert_eq!(parsed.gross_release, gross_release);
        assert_eq!(parsed.net_release, net_release);
        assert_eq!(parsed.r_q64, r_q64);
        assert_eq!(parsed.mainnet_dest, mainnet_dest);
        assert_eq!(parsed.nonce_out, nonce_out);
        assert_eq!(parsed.chain_id, chain_id);
    }

    #[test]
    fn parse_returns_none_for_wrong_discriminator() {
        let mut payload = vec![0u8; 100];
        payload[..8].copy_from_slice(&[0xFFu8; 8]);
        let r = try_parse_deposit_event(&payload, "sig").unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn parse_errors_on_too_short_payload() {
        let payload = vec![0u8; 4];
        let err = try_parse_deposit_event(&payload, "sig").unwrap_err();
        assert!(matches!(err, LogParseError::PayloadTooShort { len: 4 }));
    }

    #[test]
    fn parse_errors_on_truncated_body() {
        // Discriminator matches but body is too short for the event struct.
        let mut payload = Vec::new();
        payload.extend_from_slice(&DEPOSIT_EVENT_DISCRIMINATOR);
        payload.extend_from_slice(&[0u8; 10]); // way short of 96 bytes
        let err = try_parse_deposit_event(&payload, "sig").unwrap_err();
        assert!(matches!(err, LogParseError::Borsh(_)));
    }

    #[test]
    fn decode_program_data_line_strips_prefix() {
        // base64 of "hello" = aGVsbG8=
        let line = "Program data: aGVsbG8=";
        let bytes = decode_program_data_line(line).unwrap().unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn decode_program_data_line_rejects_non_event_lines() {
        let line = "Program log: hello";
        assert!(decode_program_data_line(line).unwrap().is_none());
    }

    #[test]
    fn extract_deposit_events_handles_mixed_logs() {
        // Build a synthetic log slice containing one deposit event and a bunch of
        // unrelated lines. Confirm we extract exactly the one event.
        let asset_id: u32 = 5;
        let user = Pubkey::new_from_array([3u8; 32]);
        let amount: u64 = 2_000_000;
        let amount_after_fee: u64 = 1_999_800;
        let dest = [4u8; 32];
        let nonce: u64 = 99;
        let chain_id: u32 = 0x6361_7473;

        let mut payload = Vec::new();
        payload.extend_from_slice(&DEPOSIT_EVENT_DISCRIMINATOR);
        payload.extend_from_slice(&asset_id.to_le_bytes());
        payload.extend_from_slice(user.as_ref());
        payload.extend_from_slice(&amount.to_le_bytes());
        payload.extend_from_slice(&amount_after_fee.to_le_bytes());
        payload.extend_from_slice(&dest);
        payload.extend_from_slice(&nonce.to_le_bytes());
        payload.extend_from_slice(&chain_id.to_le_bytes());
        let event_line = format!("Program data: {}", base64_encode_test(&payload));

        let logs = vec![
            "Program log: invoking deposit".to_string(),
            "Program 11111111111111111111111111111111 invoke [2]".to_string(),
            event_line,
            "Program log: success".to_string(),
        ];
        let events = extract_deposit_events(&logs, "txABC");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].asset_id, asset_id);
        assert_eq!(events[0].nonce, nonce);
        assert_eq!(events[0].source_signature, "txABC");
    }

    /// Test-only base64 encoder so we can construct valid `Program data:` lines
    /// without pulling in the `base64` crate.
    fn base64_encode_test(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            out.push(ALPHABET[(b0 >> 2) as usize] as char);
            out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            if chunk.len() >= 2 {
                out.push(ALPHABET[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() == 3 {
                out.push(ALPHABET[(b2 & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    #[test]
    fn base64_decode_round_trip() {
        for input in [
            b"".to_vec(),
            b"f".to_vec(),
            b"fo".to_vec(),
            b"foo".to_vec(),
            b"foob".to_vec(),
            b"fooba".to_vec(),
            b"foobar".to_vec(),
        ] {
            let encoded = base64_encode_test(&input);
            let decoded = base64_decode(&encoded).unwrap();
            assert_eq!(decoded, input, "round trip failed for {input:?}");
        }
    }
}

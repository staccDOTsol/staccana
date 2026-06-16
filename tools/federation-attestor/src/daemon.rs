//! Long-running federation-attestor daemon: polls both chains, signs attestations,
//! persists state.
//!
//! The daemon is structured as one tick function ([`Daemon::tick`]) that does a single
//! poll-cycle worth of work. The main loop in `main.rs` calls it on a fixed interval.
//! This shape keeps the integration tests trivial: drive ticks manually and inspect
//! the recorded attestations.
//!
//! ## Per-tick algorithm
//!
//! ```text
//! For each chain (solana-devnet vault, staccana bridge):
//!   sigs = rpc.signatures_for_address_since(program_id, last_seen_signature, LIMIT)
//!   for sig in sigs:
//!     logs = rpc.transaction_logs(sig)
//!     for event in extract_events(logs, sig):
//!       attestation = sign(event, signer_keypair)
//!       record(attestation)               // hand off to relayer / publish stub
//!     state.last_seen_signature = sig
//!   state.save()
//! ```
//!
//! Recording / publishing is intentionally a **callback** (the [`Sink`] trait below)
//! so the daemon stays testable: production wires it to a publisher that builds the
//! `mint` / `release_with_attestation` ix bytes and submits via RPC; tests wire it to
//! an in-memory `Vec<SignedAttestation>` collector.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signature};
use solana_sdk::signer::Signer;

use crate::bridge_msg::{sign_mint, sign_release, MINT_MSG_LEN, RELEASE_MSG_LEN};
use crate::bridge_observer::{
    extract_burn_events, extract_deposit_events, BridgeRpcClient, BurnEvent, DepositEvent,
};
use crate::state_store::AttestorState;

/// Maximum signatures to fetch per address per tick. RPCs typically cap at 1000;
/// 100 keeps the per-tick latency bounded while still draining a backlog quickly.
pub const SIGNATURES_PER_TICK: usize = 100;

/// One signed mint attestation, ready to be aggregated with M-1 peers' sigs and
/// submitted to the staccana bridge `mint` ix.
#[derive(Clone, Debug)]
pub struct SignedMintAttestation {
    pub event: DepositEvent,
    pub message: [u8; MINT_MSG_LEN],
    pub signer: Pubkey,
    pub signature: Signature,
}

/// Mirror, for staccana → mainnet release attestations.
#[derive(Clone, Debug)]
pub struct SignedReleaseAttestation {
    pub event: BurnEvent,
    pub message: [u8; RELEASE_MSG_LEN],
    pub signer: Pubkey,
    pub signature: Signature,
}

/// Pluggable destination for newly-signed attestations. The daemon hands each
/// attestation to one of these methods; the implementation decides what to do
/// (persist, gossip to peers, submit to RPC, etc).
pub trait Sink: Send + Sync {
    fn record_mint(&self, att: SignedMintAttestation) -> Result<()>;
    fn record_release(&self, att: SignedReleaseAttestation) -> Result<()>;
}

/// Default sink that just logs the attestation to stderr. Useful for first-launch
/// validation: operator can `journalctl -u staccana-federation-attestor@1` and see
/// every signature flow through.
pub struct StderrSink;

impl Sink for StderrSink {
    fn record_mint(&self, att: SignedMintAttestation) -> Result<()> {
        eprintln!(
            "[federation-attestor] signed mint attestation: asset_id={} nonce={} \
             value_after_fee={} dest={} signer={} sig={}",
            att.event.asset_id,
            att.event.nonce,
            att.event.amount_after_fee,
            bs58::encode(att.event.dest).into_string(),
            att.signer,
            att.signature,
        );
        Ok(())
    }

    fn record_release(&self, att: SignedReleaseAttestation) -> Result<()> {
        eprintln!(
            "[federation-attestor] signed release attestation: asset_id={} nonce_out={} \
             gross_release={} mainnet_dest={} signer={} sig={}",
            att.event.asset_id,
            att.event.nonce_out,
            att.event.gross_release,
            bs58::encode(att.event.mainnet_dest).into_string(),
            att.signer,
            att.signature,
        );
        Ok(())
    }
}

/// Inputs to a single daemon tick. Held by reference so `tick` can be cheap to call
/// in a loop.
pub struct DaemonCtx<'a> {
    pub signer: &'a Keypair,
    pub state_path: &'a PathBuf,
    pub solana_rpc: &'a dyn BridgeRpcClient,
    pub staccana_rpc: &'a dyn BridgeRpcClient,
    /// Mainnet/devnet bridge-vault program id (the `--bridge-vault` CLI flag).
    pub bridge_vault_program: Pubkey,
    /// Staccana bridge program id (the `--staccana-bridge` CLI flag).
    pub staccana_bridge_program: Pubkey,
    pub sink: Arc<dyn Sink>,
}

/// Run one tick: poll both chains, sign every new event, persist cursor.
///
/// Returns the count of `(deposit_events_signed, burn_events_signed)` so the caller
/// can log progress / drive tests.
pub fn tick(ctx: &DaemonCtx<'_>) -> Result<(usize, usize)> {
    let mut state = AttestorState::load(ctx.state_path)
        .with_context(|| format!("load state from {}", ctx.state_path.display()))?;

    // --- Solana side: deposit events --------------------------------------
    let mut deposit_count = 0;
    let solana_sigs = ctx
        .solana_rpc
        .signatures_for_address_since(
            &ctx.bridge_vault_program,
            state.last_seen_solana_signature.as_deref(),
            SIGNATURES_PER_TICK,
        )
        .context("poll solana bridge-vault signatures")?;

    for sig in solana_sigs {
        let logs = match ctx
            .solana_rpc
            .transaction_logs(&sig)
            .with_context(|| format!("fetch logs for sig {sig}"))?
        {
            Some(l) => l,
            None => {
                // No logs (failed tx) — still advance the cursor to avoid re-fetching.
                state.last_seen_solana_signature = Some(sig);
                continue;
            }
        };
        let events = extract_deposit_events(&logs, &sig);
        for event in events {
            let (msg, signature) = sign_mint(
                event.asset_id,
                event.amount_after_fee,
                &event.dest,
                event.nonce,
                ctx.signer,
            );
            ctx.sink.record_mint(SignedMintAttestation {
                event,
                message: msg,
                signer: ctx.signer.pubkey(),
                signature,
            })?;
            deposit_count += 1;
        }
        state.last_seen_solana_signature = Some(sig);
    }

    // --- Staccana side: burn events ---------------------------------------
    let mut burn_count = 0;
    let staccana_sigs = ctx
        .staccana_rpc
        .signatures_for_address_since(
            &ctx.staccana_bridge_program,
            state.last_seen_staccana_signature.as_deref(),
            SIGNATURES_PER_TICK,
        )
        .context("poll staccana bridge signatures")?;

    for sig in staccana_sigs {
        let logs = match ctx
            .staccana_rpc
            .transaction_logs(&sig)
            .with_context(|| format!("fetch logs for sig {sig}"))?
        {
            Some(l) => l,
            None => {
                state.last_seen_staccana_signature = Some(sig);
                continue;
            }
        };
        let events = extract_burn_events(&logs, &sig);
        for event in events {
            let (msg, signature) = sign_release(
                event.asset_id,
                event.gross_release,
                &event.mainnet_dest,
                event.nonce_out,
                ctx.signer,
            );
            ctx.sink.record_release(SignedReleaseAttestation {
                event,
                message: msg,
                signer: ctx.signer.pubkey(),
                signature,
            })?;
            burn_count += 1;
        }
        state.last_seen_staccana_signature = Some(sig);
    }

    state.touch_now();
    state
        .save(ctx.state_path)
        .with_context(|| format!("save state to {}", ctx.state_path.display()))?;

    Ok((deposit_count, burn_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_msg::{verify_mint, verify_release, MINT_DOMAIN, RELEASE_DOMAIN};
    use crate::bridge_observer::{BURN_EVENT_DISCRIMINATOR, DEPOSIT_EVENT_DISCRIMINATOR};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory mock RPC: hand-crafted signatures + per-signature log slices.
    struct MockRpc {
        sigs: Mutex<Vec<String>>, // oldest-first
        logs: Mutex<HashMap<String, Vec<String>>>,
    }

    impl MockRpc {
        fn new(sigs: Vec<String>, logs: HashMap<String, Vec<String>>) -> Self {
            Self {
                sigs: Mutex::new(sigs),
                logs: Mutex::new(logs),
            }
        }
    }

    impl BridgeRpcClient for MockRpc {
        fn signatures_for_address_since(
            &self,
            _address: &Pubkey,
            until: Option<&str>,
            _limit: usize,
        ) -> Result<Vec<String>> {
            let all = self.sigs.lock().unwrap().clone();
            // Mirror the contract: return oldest-first, sliced to "newer than `until`".
            if let Some(cursor) = until {
                if let Some(idx) = all.iter().position(|s| s == cursor) {
                    return Ok(all.into_iter().skip(idx + 1).collect());
                }
            }
            Ok(all)
        }

        fn transaction_logs(&self, signature: &str) -> Result<Option<Vec<String>>> {
            Ok(self.logs.lock().unwrap().get(signature).cloned())
        }
    }

    /// Test-only base64 encoder (matches the helper in `bridge_observer.rs::tests`).
    fn b64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
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

    fn make_deposit_log_line(
        asset_id: u32,
        user: &Pubkey,
        amount: u64,
        amount_after_fee: u64,
        dest: &[u8; 32],
        nonce: u64,
        chain_id: u32,
    ) -> String {
        let mut payload = Vec::new();
        payload.extend_from_slice(&DEPOSIT_EVENT_DISCRIMINATOR);
        payload.extend_from_slice(&asset_id.to_le_bytes());
        payload.extend_from_slice(user.as_ref());
        payload.extend_from_slice(&amount.to_le_bytes());
        payload.extend_from_slice(&amount_after_fee.to_le_bytes());
        payload.extend_from_slice(dest);
        payload.extend_from_slice(&nonce.to_le_bytes());
        payload.extend_from_slice(&chain_id.to_le_bytes());
        format!("Program data: {}", b64(&payload))
    }

    fn make_burn_log_line(
        asset_id: u32,
        user: &Pubkey,
        amount: u64,
        gross: u64,
        net: u64,
        r_q64: u128,
        dest: &[u8; 32],
        nonce_out: u64,
        chain_id: u32,
    ) -> String {
        let mut payload = Vec::new();
        payload.extend_from_slice(&BURN_EVENT_DISCRIMINATOR);
        payload.extend_from_slice(&asset_id.to_le_bytes());
        payload.extend_from_slice(user.as_ref());
        payload.extend_from_slice(&amount.to_le_bytes());
        payload.extend_from_slice(&gross.to_le_bytes());
        payload.extend_from_slice(&net.to_le_bytes());
        payload.extend_from_slice(&r_q64.to_le_bytes());
        payload.extend_from_slice(dest);
        payload.extend_from_slice(&nonce_out.to_le_bytes());
        payload.extend_from_slice(&chain_id.to_le_bytes());
        format!("Program data: {}", b64(&payload))
    }

    #[derive(Default)]
    struct CollectingSink {
        mints: Mutex<Vec<SignedMintAttestation>>,
        releases: Mutex<Vec<SignedReleaseAttestation>>,
    }

    impl Sink for CollectingSink {
        fn record_mint(&self, att: SignedMintAttestation) -> Result<()> {
            self.mints.lock().unwrap().push(att);
            Ok(())
        }
        fn record_release(&self, att: SignedReleaseAttestation) -> Result<()> {
            self.releases.lock().unwrap().push(att);
            Ok(())
        }
    }

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "staccana-attestor-daemon-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn tick_signs_each_deposit_event_with_correct_message_bytes() {
        // Two deposits across two transactions. After one tick, sink should have two
        // signed mint attestations whose messages match the canonical preimage.
        let signer = Keypair::new();
        let user1 = Pubkey::new_unique();
        let user2 = Pubkey::new_unique();
        let dest1 = [11u8; 32];
        let dest2 = [22u8; 32];

        let mut logs = HashMap::new();
        logs.insert(
            "sigA".into(),
            vec![make_deposit_log_line(
                1,
                &user1,
                1_000_000,
                999_000,
                &dest1,
                0,
                0x6361_7473,
            )],
        );
        logs.insert(
            "sigB".into(),
            vec![make_deposit_log_line(
                1,
                &user2,
                500_000,
                499_500,
                &dest2,
                1,
                0x6361_7473,
            )],
        );
        let solana = MockRpc::new(vec!["sigA".into(), "sigB".into()], logs);
        let staccana = MockRpc::new(vec![], HashMap::new());

        let dir = tempdir();
        let state_path = AttestorState::path_for(&dir, &signer.pubkey());
        let sink: Arc<CollectingSink> = Arc::new(CollectingSink::default());
        let sink_ref: Arc<dyn Sink> = sink.clone();
        let ctx = DaemonCtx {
            signer: &signer,
            state_path: &state_path,
            solana_rpc: &solana,
            staccana_rpc: &staccana,
            bridge_vault_program: Pubkey::new_unique(),
            staccana_bridge_program: Pubkey::new_unique(),
            sink: sink_ref,
        };

        let (mints, burns) = tick(&ctx).unwrap();
        assert_eq!(mints, 2);
        assert_eq!(burns, 0);

        let mints = sink.mints.lock().unwrap();
        assert_eq!(mints.len(), 2);
        // First attestation: nonce=0, dest=dest1.
        assert_eq!(mints[0].event.nonce, 0);
        assert_eq!(mints[0].event.dest, dest1);
        assert_eq!(&mints[0].message[0..16], MINT_DOMAIN);
        assert!(verify_mint(
            &mints[0].message,
            &mints[0].signer,
            &mints[0].signature
        ));
        // Second attestation: nonce=1, dest=dest2.
        assert_eq!(mints[1].event.nonce, 1);
        assert_eq!(mints[1].event.dest, dest2);
        assert!(verify_mint(
            &mints[1].message,
            &mints[1].signer,
            &mints[1].signature
        ));
    }

    #[test]
    fn tick_signs_each_burn_event_with_correct_message_bytes() {
        let signer = Keypair::new();
        let user = Pubkey::new_unique();
        let dest = [33u8; 32];

        let mut logs = HashMap::new();
        logs.insert(
            "burnSig".into(),
            vec![make_burn_log_line(
                7,
                &user,
                500_000,
                500_000,
                499_500,
                1u128 << 64,
                &dest,
                0,
                0x6D61_696E,
            )],
        );
        let solana = MockRpc::new(vec![], HashMap::new());
        let staccana = MockRpc::new(vec!["burnSig".into()], logs);

        let dir = tempdir();
        let state_path = AttestorState::path_for(&dir, &signer.pubkey());
        let sink: Arc<CollectingSink> = Arc::new(CollectingSink::default());
        let sink_ref: Arc<dyn Sink> = sink.clone();
        let ctx = DaemonCtx {
            signer: &signer,
            state_path: &state_path,
            solana_rpc: &solana,
            staccana_rpc: &staccana,
            bridge_vault_program: Pubkey::new_unique(),
            staccana_bridge_program: Pubkey::new_unique(),
            sink: sink_ref,
        };

        let (mints, burns) = tick(&ctx).unwrap();
        assert_eq!(mints, 0);
        assert_eq!(burns, 1);

        let releases = sink.releases.lock().unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].event.nonce_out, 0);
        assert_eq!(releases[0].event.gross_release, 500_000);
        assert_eq!(&releases[0].message[0..18], RELEASE_DOMAIN);
        assert!(verify_release(
            &releases[0].message,
            &releases[0].signer,
            &releases[0].signature
        ));
    }

    #[test]
    fn replay_protection_skips_already_seen_signatures() {
        // First tick processes "sigA" + "sigB". Second tick (with the same RPC)
        // should re-read the cursor from disk, ask `since(sigB)`, and get nothing.
        let signer = Keypair::new();
        let user = Pubkey::new_unique();
        let dest = [11u8; 32];

        let mut logs = HashMap::new();
        logs.insert(
            "sigA".into(),
            vec![make_deposit_log_line(
                1,
                &user,
                1_000_000,
                999_000,
                &dest,
                0,
                0x6361_7473,
            )],
        );
        logs.insert(
            "sigB".into(),
            vec![make_deposit_log_line(
                1,
                &user,
                500_000,
                499_500,
                &dest,
                1,
                0x6361_7473,
            )],
        );
        let solana = MockRpc::new(vec!["sigA".into(), "sigB".into()], logs);
        let staccana = MockRpc::new(vec![], HashMap::new());

        let dir = tempdir();
        let state_path = AttestorState::path_for(&dir, &signer.pubkey());
        let sink: Arc<CollectingSink> = Arc::new(CollectingSink::default());
        let sink_ref: Arc<dyn Sink> = sink.clone();
        let ctx = DaemonCtx {
            signer: &signer,
            state_path: &state_path,
            solana_rpc: &solana,
            staccana_rpc: &staccana,
            bridge_vault_program: Pubkey::new_unique(),
            staccana_bridge_program: Pubkey::new_unique(),
            sink: sink_ref,
        };

        let (mints1, _) = tick(&ctx).unwrap();
        assert_eq!(mints1, 2);

        // Second tick on the same MockRpc: state cursor is now `sigB`, so the mock
        // returns no signatures newer than that.
        let (mints2, _) = tick(&ctx).unwrap();
        assert_eq!(mints2, 0, "replay protection: second tick must process zero events");

        // Sink still has only the original two attestations.
        assert_eq!(sink.mints.lock().unwrap().len(), 2);
    }

    #[test]
    fn tick_advances_cursor_even_for_tx_with_no_events() {
        // Tx with logs but no `Program data` lines (e.g. a `register_asset` ix).
        // Cursor must still advance so we don't re-poll it next tick.
        let signer = Keypair::new();

        let mut logs = HashMap::new();
        logs.insert(
            "boringSig".into(),
            vec!["Program log: register_asset called".to_string()],
        );
        let solana = MockRpc::new(vec!["boringSig".into()], logs);
        let staccana = MockRpc::new(vec![], HashMap::new());

        let dir = tempdir();
        let state_path = AttestorState::path_for(&dir, &signer.pubkey());
        let sink: Arc<CollectingSink> = Arc::new(CollectingSink::default());
        let sink_ref: Arc<dyn Sink> = sink.clone();
        let ctx = DaemonCtx {
            signer: &signer,
            state_path: &state_path,
            solana_rpc: &solana,
            staccana_rpc: &staccana,
            bridge_vault_program: Pubkey::new_unique(),
            staccana_bridge_program: Pubkey::new_unique(),
            sink: sink_ref,
        };

        let (mints, _) = tick(&ctx).unwrap();
        assert_eq!(mints, 0);

        // State should now have `boringSig` as the cursor.
        let state = AttestorState::load(&state_path).unwrap();
        assert_eq!(
            state.last_seen_solana_signature.as_deref(),
            Some("boringSig")
        );
    }

    #[test]
    fn signing_is_deterministic_across_ticks() {
        // ed25519 deterministic-K means same key + same message ⇒ same signature.
        // Mirror the property at the daemon level: re-process the same event with
        // the same keypair and confirm the recorded signature byte-matches.
        let signer = Keypair::new();
        let user = Pubkey::new_unique();
        let dest = [55u8; 32];

        let mut logs = HashMap::new();
        logs.insert(
            "sig1".into(),
            vec![make_deposit_log_line(
                3,
                &user,
                100,
                99,
                &dest,
                42,
                0x6361_7473,
            )],
        );

        let solana1 = MockRpc::new(vec!["sig1".into()], logs.clone());
        let staccana = MockRpc::new(vec![], HashMap::new());
        let dir1 = tempdir();
        let state_path1 = AttestorState::path_for(&dir1, &signer.pubkey());
        let sink1: Arc<CollectingSink> = Arc::new(CollectingSink::default());
        let sink_ref1: Arc<dyn Sink> = sink1.clone();
        let bvp = Pubkey::new_unique();
        let sbp = Pubkey::new_unique();
        let ctx1 = DaemonCtx {
            signer: &signer,
            state_path: &state_path1,
            solana_rpc: &solana1,
            staccana_rpc: &staccana,
            bridge_vault_program: bvp,
            staccana_bridge_program: sbp,
            sink: sink_ref1,
        };
        tick(&ctx1).unwrap();

        // Run a second daemon instance with the same keypair, fresh state — it
        // re-processes the same event and produces the same sig.
        let solana2 = MockRpc::new(vec!["sig1".into()], logs);
        let dir2 = tempdir();
        let state_path2 = AttestorState::path_for(&dir2, &signer.pubkey());
        let sink2: Arc<CollectingSink> = Arc::new(CollectingSink::default());
        let sink_ref2: Arc<dyn Sink> = sink2.clone();
        let ctx2 = DaemonCtx {
            signer: &signer,
            state_path: &state_path2,
            solana_rpc: &solana2,
            staccana_rpc: &staccana,
            bridge_vault_program: bvp,
            staccana_bridge_program: sbp,
            sink: sink_ref2,
        };
        tick(&ctx2).unwrap();

        let s1 = sink1.mints.lock().unwrap();
        let s2 = sink2.mints.lock().unwrap();
        assert_eq!(s1[0].signature, s2[0].signature);
        assert_eq!(s1[0].message, s2[0].message);
    }
}

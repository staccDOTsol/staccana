//! Binary entrypoint for the staccana federation-attestor daemon.
//!
//! Two operation modes, picked by which CLI flags are set:
//!
//! 1. **Bridge attestor (v1, the production flow)** — the operator passes
//!    `--signer-keypair`, `--solana-rpc`, `--staccana-rpc`, `--bridge-vault`,
//!    `--staccana-bridge`. The daemon polls each chain's bridge program for new
//!    deposits / burns, signs the canonical mint / release attestations, and hands
//!    them to a [`Sink`] (default: stderr-log). Persists `last_seen_signature` per
//!    direction under `--state-dir/attestor-state-<signer-pubkey>.json`.
//!
//! 2. **Ratio attestor (v0, legacy SPEC §5.3 flow)** — the operator passes
//!    `--config /path/to/attestor.toml`. The daemon loads the TOML config and runs
//!    the original ratio-attestation tick (currently stubbed; observer + publish are
//!    v0 placeholders). Kept available so the existing systemd unit
//!    `infra/systemd/staccana-attestor.service` keeps working.
//!
//! Per-instance keypair selection (federation member 1..9) lives in the systemd
//! template unit `infra/systemd/staccana-federation-attestor@.service`:
//! `--signer-keypair /etc/staccana/federation/signer-%i.json`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::read_keypair_file;
use solana_sdk::signer::Signer;

use staccana_federation_attestor::{
    config::AttestorConfig,
    daemon::{tick, DaemonCtx, StderrSink},
    bridge_observer::SolanaRpcClient,
    observer::{Observer, StubObserver},
    state_store::AttestorState,
};

#[derive(Parser, Debug)]
#[command(
    name = "staccana-federation-attestor",
    about = "Federation member daemon: signs cross-chain attestations for the staccana ↔ Solana bridge.",
    version
)]
struct Cli {
    // --- v1 (bridge attestor) flags -------------------------------------

    /// Path to this federation member's ed25519 signing keypair (Solana JSON
    /// keypair format). Triggers v1 (bridge attestor) mode.
    #[arg(long, conflicts_with = "config")]
    signer_keypair: Option<PathBuf>,

    /// Staccana cluster RPC URL. Defaults to the staccana validator on box 1.
    #[arg(long, default_value = "http://localhost:8899")]
    staccana_rpc: String,

    /// Solana cluster RPC URL (devnet for the v1 bridge). Defaults to public devnet.
    #[arg(long, default_value = "https://api.devnet.solana.com")]
    solana_rpc: String,

    /// Staccana bridge program id (the wrapper-mint program). Default points at the
    /// 2026-05-02 deploy `LA7h3...`.
    #[arg(long, default_value = "LA7h3hjvD62MeTtdeE4h2vq3EGxbU1oqzHtewp4xb9b")]
    staccana_bridge: String,

    /// Bridge-vault program id (the mainnet/devnet custody program). Default points
    /// at the 2026-05-02 devnet deploy `F2Ayp...`.
    #[arg(long, default_value = "F2AypZ8FDWnR5bdyLHzo4idof9YrBpdBmbgLwLBjLfVU")]
    bridge_vault: String,

    /// Polling interval in seconds. Each tick fetches up to
    /// `daemon::SIGNATURES_PER_TICK` new signatures per chain.
    #[arg(long, default_value_t = 5)]
    poll_interval: u64,

    /// Directory for persistent cursor state. The daemon writes one file per signer
    /// pubkey here (`attestor-state-<pubkey>.json`).
    #[arg(long, default_value = "/var/lib/staccana/attestor")]
    state_dir: PathBuf,

    // --- v0 (legacy ratio attestor) flag --------------------------------

    /// Path to a TOML config file for the legacy ratio-attestor mode (SPEC §5.3).
    /// Mutually exclusive with `--signer-keypair`.
    #[arg(long, short)]
    config: Option<PathBuf>,

    // --- shared --------------------------------------------------------

    /// Run a single tick and exit. Useful for systemd OneShot health checks and for
    /// driving manual replays from a one-liner.
    #[arg(long, default_value_t = false)]
    once: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.signer_keypair.is_some() {
        run_bridge_attestor(cli).await
    } else if cli.config.is_some() {
        run_ratio_attestor(cli).await
    } else {
        Err(anyhow!(
            "must pass either --signer-keypair (v1 bridge attestor) or --config (v0 ratio attestor)"
        ))
    }
}

/// v1 path: poll Solana devnet bridge-vault for `Deposit`s and staccana bridge for
/// `Burn`s, sign attestations, persist cursor.
async fn run_bridge_attestor(cli: Cli) -> Result<()> {
    let signer_path = cli.signer_keypair.expect("checked by caller");
    let signer = read_keypair_file(&signer_path).map_err(|e| {
        anyhow!(
            "failed to load signer keypair from {:?}: {}",
            signer_path,
            e
        )
    })?;
    let bridge_vault: Pubkey = cli
        .bridge_vault
        .parse()
        .with_context(|| format!("parse --bridge-vault {:?}", cli.bridge_vault))?;
    let staccana_bridge: Pubkey = cli
        .staccana_bridge
        .parse()
        .with_context(|| format!("parse --staccana-bridge {:?}", cli.staccana_bridge))?;

    let solana_rpc = SolanaRpcClient::new(cli.solana_rpc.clone());
    let staccana_rpc = SolanaRpcClient::new(cli.staccana_rpc.clone());

    let state_path = AttestorState::path_for(&cli.state_dir, &signer.pubkey());

    eprintln!(
        "[federation-attestor] bridge mode: signer={} state={} solana_rpc={} staccana_rpc={} \
         bridge_vault={} staccana_bridge={} poll_interval={}s",
        signer.pubkey(),
        state_path.display(),
        cli.solana_rpc,
        cli.staccana_rpc,
        bridge_vault,
        staccana_bridge,
        cli.poll_interval,
    );

    let sink: Arc<dyn staccana_federation_attestor::Sink> = Arc::new(StderrSink);

    loop {
        let ctx = DaemonCtx {
            signer: &signer,
            state_path: &state_path,
            solana_rpc: &solana_rpc,
            staccana_rpc: &staccana_rpc,
            bridge_vault_program: bridge_vault,
            staccana_bridge_program: staccana_bridge,
            sink: sink.clone(),
        };
        match tick(&ctx) {
            Ok((m, b)) => {
                if m + b > 0 {
                    eprintln!(
                        "[federation-attestor] tick: signed {m} mint, {b} release attestations"
                    );
                }
            }
            Err(e) => {
                eprintln!("[federation-attestor] tick error (will retry): {e:#}");
            }
        }
        if cli.once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(cli.poll_interval)).await;
    }
}

/// v0 path: legacy ratio-attestor that reads a TOML config and runs a stubbed
/// observer/publish loop. Preserved so the previously-shipped systemd unit doesn't
/// break.
async fn run_ratio_attestor(cli: Cli) -> Result<()> {
    let config_path = cli.config.expect("checked by caller");
    let cfg = AttestorConfig::load_and_validate(&config_path)
        .with_context(|| format!("loading config from {:?}", config_path))?;
    eprintln!(
        "[federation-attestor] ratio mode: loaded config for member_index={} (set size {}, peers {})",
        cfg.member_index,
        cfg.federation_pubkeys.len(),
        cfg.peers.len()
    );

    let _keypair = cfg
        .load_keypair()
        .with_context(|| format!("loading signing keypair from {:?}", cfg.member_key_path))?;

    let mut observer = StubObserver::new();
    // Half of the ~60s `R_PUBLISH_INTERVAL_SLOTS` per SPEC §2.3.
    let tick_dur = Duration::from_secs(30);
    loop {
        match observer.poll_deposit() {
            Ok(Some(d)) => eprintln!("[federation-attestor] (ratio) saw deposit: {d:?}"),
            Ok(None) => {}
            Err(e) => eprintln!("[federation-attestor] (ratio) deposit poll error: {e}"),
        }
        match observer.poll_burn() {
            Ok(Some(b)) => eprintln!("[federation-attestor] (ratio) saw burn: {b:?}"),
            Ok(None) => {}
            Err(e) => eprintln!("[federation-attestor] (ratio) burn poll error: {e}"),
        }
        if cli.once {
            return Ok(());
        }
        tokio::time::sleep(tick_dur).await;
    }
}

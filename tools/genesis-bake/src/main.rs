//! `staccana-genesis-bake` CLI binary.
//!
//! Drives the library: reads a `composed-genesis.json`, the four bootstrap keypairs,
//! and (optionally) up to five program `.so` paths, then writes a complete bootable
//! ledger directory at the requested path. See the crate's `lib.rs` doc comment for
//! the end-to-end pipeline.
//!
//! Usage:
//!
//! ```text
//! staccana-genesis-bake \
//!   --composed-genesis /var/lib/staccana/genesis/composed-genesis.json \
//!   --identity-keypair /etc/staccana/keys/identity.json \
//!   --vote-keypair    /etc/staccana/keys/vote.json \
//!   --stake-keypair   /etc/staccana/keys/stake.json \
//!   --faucet-keypair  /etc/staccana/keys/faucet.json \
//!   --cluster-type   development \
//!   --lazy-claim-so          target/deploy/staccana_lazy_claim.so \
//!   --secret-pump-so         programs/secret-pump/target/deploy/staccana_secret_pump.so \
//!   --megadrop-so            programs/megadrop/target/deploy/staccana_megadrop.so \
//!   --output-ledger-dir /var/lib/staccana/ledger
//! ```
//!
//! The output is a *directory*, not a file. Inside it you'll find:
//!   - `genesis.bin`     — the bincode-serialized GenesisConfig
//!   - `genesis.tar.bz2` — tar+bzip2 of genesis.bin + the rocksdb dir (snapshot
//!     bootstrappers fetch this)
//!   - `rocksdb/`        — the blockstore, pre-seeded with slot 0 PoH ticks so the
//!     validator can `process_bank_0` without panicking with
//!     `InvalidBlock(Incomplete)`

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Result};
use clap::{Parser, ValueEnum};
use solana_cluster_type::ClusterType;
use solana_pubkey::Pubkey;

/// Parse a base58-encoded pubkey from a CLI string. Used by clap value parsers.
fn parse_pubkey(s: &str) -> Result<Pubkey, String> {
    Pubkey::from_str(s).map_err(|e| format!("invalid base58 pubkey {s:?}: {e}"))
}

use staccana_genesis_bake::{
    bake, emit::log_bake_summary, emit::write_ledger, load_inputs_from_paths,
};

/// CLI mirror of `solana_cluster_type::ClusterType` so clap can derive `ValueEnum`.
/// `solana-cluster-type` doesn't implement `ValueEnum` itself, so we own a thin
/// shim that converts to the real enum.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ClusterTypeArg {
    Development,
    Devnet,
    Testnet,
    MainnetBeta,
}

impl From<ClusterTypeArg> for ClusterType {
    fn from(v: ClusterTypeArg) -> Self {
        match v {
            ClusterTypeArg::Development => ClusterType::Development,
            ClusterTypeArg::Devnet => ClusterType::Devnet,
            ClusterTypeArg::Testnet => ClusterType::Testnet,
            ClusterTypeArg::MainnetBeta => ClusterType::MainnetBeta,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "staccana-genesis-bake",
    about = "Bake a complete bootable Staccana ledger directory from a ComposedGenesis JSON.",
    version
)]
struct Cli {
    /// Path to the `composed-genesis.json` written by `staccana-genesis-emit` (step
    /// 20 of the deploy pipeline).
    #[arg(long)]
    composed_genesis: PathBuf,

    /// Bootstrap validator identity keypair (standard `solana-keygen` JSON format).
    #[arg(long)]
    identity_keypair: PathBuf,

    /// Bootstrap validator vote keypair.
    #[arg(long)]
    vote_keypair: PathBuf,

    /// Bootstrap validator stake keypair.
    #[arg(long)]
    stake_keypair: PathBuf,

    /// Faucet keypair. Even though staccana doesn't run a faucet on mainnet, the
    /// keypair-and-account is generated and present at slot 0 to keep tooling that
    /// expects a faucet pubkey from crashing in dev.
    #[arg(long)]
    faucet_keypair: PathBuf,

    /// Cluster type baked into the GenesisConfig. Default is `development` so the
    /// nightly devnet shake-out doesn't get a genesis labeled `MainnetBeta`. For
    /// the real mainnet-sigma launch this MUST be set to `mainnet-beta`.
    #[arg(long, value_enum, default_value_t = ClusterTypeArg::Development)]
    cluster_type: ClusterTypeArg,

    /// `.so` path for the lazy-claim program. If omitted the program is skipped (the
    /// chain still boots; lazy-claim must then be deployed post-boot via
    /// `solana program deploy`).
    #[arg(long)]
    lazy_claim_so: Option<PathBuf>,

    /// `.so` path for the secret-pump program. Optional — see `--lazy-claim-so` for the
    /// "skipped" semantics.
    #[arg(long)]
    secret_pump_so: Option<PathBuf>,

    /// `.so` path for the megadrop program. Optional.
    #[arg(long)]
    megadrop_so: Option<PathBuf>,

    /// `.so` path for SPL Token v3 (canonical mainnet pubkey
    /// `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`). Genesis-baking it here
    /// avoids the post-boot deploy needing the canonical upgrade-authority
    /// keypair (which we don't have).
    #[arg(long)]
    spl_token_so: Option<PathBuf>,

    /// `.so` path for SPL Token-2022 v8 (canonical
    /// `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`). Required for any
    /// downstream consumer that uses Anchor's `Program<'info, Token2022>` or
    /// `Interface<'info, TokenInterface>` (which both hardcode-check the
    /// canonical address).
    #[arg(long)]
    spl_token_2022_so: Option<PathBuf>,

    /// `.so` path for SPL Associated Token Account (canonical
    /// `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`).
    #[arg(long)]
    spl_associated_token_so: Option<PathBuf>,

    /// `.so` path for SPL Memo v3 (canonical
    /// `MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr`).
    #[arg(long)]
    spl_memo_so: Option<PathBuf>,

    /// `.so` path for the AddressLookupTable core-BPF program (deployed at
    /// `AddressLookupTab1e1111111111111111111111111`). Required for any v0
    /// transaction that references a LUT — without it, every page that
    /// hit the legacy 1232-byte ceiling pre-flight-rejects with
    /// `ProgramAccountNotFound`. Source ships with `solana-program-test`
    /// as `core_bpf_address_lookup_table-3.0.0.so`.
    #[arg(long)]
    address_lookup_table_so: Option<PathBuf>,

    /// Upgrade authority pubkey (base58) baked into the staccana programs
    /// at slot 0. Without this flag the staccana programs (lazy-claim,
    /// bridge, secret-pump, validator-subsidy, megadrop) are immutable
    /// from genesis — any future on-chain bug means another full rebake.
    /// With this flag set to a pubkey the operator controls, future
    /// patches can ship via `solana program deploy --upgrade-authority`
    /// without touching genesis.
    ///
    /// SPL programs (token, token-2022, ATA, memo) always bake immutable
    /// regardless of this flag — they're upstream canonical and we never
    /// upgrade them out from under live txs.
    #[arg(long, value_parser = parse_pubkey)]
    staccana_program_upgrade_authority: Option<Pubkey>,

    /// Additional bootstrap validator keypair triplets. Each occurrence takes a
    /// comma-separated `identity.json,vote.json,stake.json` triplet. May be passed
    /// multiple times to add multiple validators. Each one will be materialized
    /// in genesis with a vote+stake account fully delegated and active from slot
    /// 0 — same shape as the primary `--identity-keypair / --vote-keypair /
    /// --stake-keypair` triplet.
    ///
    /// **Why**: agave 2.0.x's tower-BFT threshold check has a single-validator
    /// bootstrap deadlock — solo validators can never land their first vote
    /// because the threshold check rejects every attempt against the bank's
    /// (empty) on-chain vote-account state. With ≥2 validators in genesis, both
    /// can clear the "tower not deep enough" escape and converge once their vote
    /// txs reach each other via gossip.
    ///
    /// Example: `--additional-validator /etc/staccana/keys-2/identity.json,/etc/staccana/keys-2/vote.json,/etc/staccana/keys-2/stake.json`
    #[arg(long, value_delimiter = '\0')]
    additional_validator: Vec<String>,

    /// Output **directory** for the bootable ledger. Will contain `genesis.bin`,
    /// `genesis.tar.bz2`, and a `rocksdb/` blockstore. Existing contents at this
    /// path are destroyed and replaced (`Blockstore::destroy` is idempotent and
    /// safe to re-run).
    ///
    /// Backwards compat: if `--output-genesis` is passed instead, its parent
    /// directory is treated as `--output-ledger-dir`. The old form prints a
    /// deprecation warning.
    #[arg(long)]
    output_ledger_dir: Option<PathBuf>,

    /// **Deprecated.** Use `--output-ledger-dir` instead. If set, its parent
    /// directory is used as the ledger dir and a warning is printed. Kept so
    /// existing scripts don't break mid-deploy.
    #[arg(long, hide = true)]
    output_genesis: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let ledger_dir: PathBuf = match (cli.output_ledger_dir.as_ref(), cli.output_genesis.as_ref()) {
        (Some(d), None) => d.clone(),
        (None, Some(g)) => {
            eprintln!(
                "[bake] WARNING: --output-genesis is deprecated; use --output-ledger-dir. \
                 Treating its parent directory ({}) as the ledger dir.",
                g.parent().map(|p| p.display().to_string()).unwrap_or_else(|| ".".to_string())
            );
            g.parent()
                .map(|p| p.to_path_buf())
                .ok_or_else(|| anyhow!("--output-genesis path has no parent directory"))?
        }
        (Some(_), Some(_)) => {
            return Err(anyhow!(
                "pass either --output-ledger-dir or --output-genesis, not both"
            ));
        }
        (None, None) => {
            return Err(anyhow!(
                "missing required argument: --output-ledger-dir <DIR>"
            ));
        }
    };

    let cluster_type: ClusterType = cli.cluster_type.into();

    // Parse `id.json,vote.json,stake.json` triplets, one per --additional-validator
    // occurrence. Each triplet becomes a fully-staked second bootstrap validator
    // in genesis, which is what breaks agave 2.0.x's tower-BFT solo deadlock.
    let mut additional_validators: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::new();
    for raw in &cli.additional_validator {
        let parts: Vec<&str> = raw.split(',').collect();
        if parts.len() != 3 {
            return Err(anyhow!(
                "--additional-validator expects exactly 3 comma-separated keypair paths \
                 (identity.json,vote.json,stake.json); got: {raw}"
            ));
        }
        additional_validators.push((
            PathBuf::from(parts[0].trim()),
            PathBuf::from(parts[1].trim()),
            PathBuf::from(parts[2].trim()),
        ));
    }

    let inputs = load_inputs_from_paths(
        &cli.composed_genesis,
        &cli.identity_keypair,
        &cli.vote_keypair,
        &cli.stake_keypair,
        &cli.faucet_keypair,
        cluster_type,
        additional_validators,
        cli.lazy_claim_so,
        cli.secret_pump_so,
        cli.megadrop_so,
        cli.spl_token_so,
        cli.spl_token_2022_so,
        cli.spl_associated_token_so,
        cli.spl_memo_so,
        cli.address_lookup_table_so,
        cli.staccana_program_upgrade_authority,
    )?;

    let (config, summary) = bake(&inputs)?;
    let hash = write_ledger(&config, &ledger_dir)?;
    log_bake_summary(&summary, &hash, cluster_type);
    eprintln!("[bake] ledger directory ready: {}", ledger_dir.display());
    eprintln!("[bake]   - {}/genesis.bin", ledger_dir.display());
    eprintln!("[bake]   - {}/genesis.tar.bz2", ledger_dir.display());
    eprintln!("[bake]   - {}/rocksdb/   (slot 0 ticks pre-seeded)", ledger_dir.display());

    Ok(())
}

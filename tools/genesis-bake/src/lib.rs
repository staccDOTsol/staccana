//! `staccana-genesis-bake` — bake the real `genesis.bin` for staccana mainnet-sigma.
//!
//! ## What this crate replaces
//!
//! Vanilla `solana-genesis` produces a working ledger with system + faucet + bootstrap
//! validator accounts and not much else (ours weighs in at ~3.22 SOL / 212 accounts —
//! enough to boot, nowhere near enough to *be* staccana). This crate consumes the
//! `ComposedGenesis` JSON written by `tools/genesis-emit/` (step 20 of the deploy
//! pipeline) and produces a `genesis.bin` with everything pre-installed at slot 0:
//!
//! - The bootstrap validator + vote + stake + faucet accounts (1 SOL each, same as
//!   vanilla).
//! - The **treasury PDA** at `["treasury"] / VALIDATOR_SUBSIDY_PROGRAM_ID`, pre-credited
//!   with the 485M-SOL figure from the composed genesis. Owner: validator-subsidy
//!   program — so the validator-subsidy CPIs that debit it can sign.
//! - The **lazy-claim Config account** at `["config"] / LAZY_CLAIM_PROGRAM_ID`, owned by
//!   the lazy-claim program, carrying the embedded Merkle root. The on-chain claim
//!   handler reads its `claimable_root` directly from this account at runtime.
//! - The **five staccana programs** (lazy-claim, bridge, secret-pump,
//!   validator-subsidy, megadrop) registered as upgradeable BPF programs at their
//!   well-known program IDs. The `.so` byte payloads are read from disk paths supplied
//!   on the command line.
//! - The **four CTE feature gates** (the ZK ElGamal Proof set from
//!   `staccana_genesis::CTE_FEATURE_GATES_AT_GENESIS`) flipped on at slot 0 — these are
//!   the gates Token-22 Confidential Transfer Extension needs but that mainnet hasn't
//!   activated yet. Combined with the `solana_zk_elgamal_proof_program` builtin (also
//!   wired here) this is what gives staccana confidential transfers from the moment
//!   the chain boots.
//! - The classic-v1 **fixed-fee governor** (0.027 SOL, 50% burn) and **disabled
//!   inflation**, both inherited from `ComposedGenesis`.
//! - **Cluster type** = `MainnetBeta` (staccana's mainnet, not a devnet).
//!
//! ## Module layout
//!
//! - [`config`] — assembles the [`solana_genesis_config::GenesisConfig`] from a
//!   [`BakeInputs`] bundle. Pure function.
//! - [`accounts`] — the bootstrap-validator / vote / stake / faucet / treasury PDA /
//!   lazy-claim Config account constructors. Each is independently unit-tested.
//! - [`programs`] — reads `.so` files and lays them out as `bpf_loader_upgradeable`
//!   `Program` + `ProgramData` account pairs at the well-known program IDs.
//! - [`features`] — turns each entry of `CTE_FEATURE_GATES_AT_GENESIS` into a
//!   `Feature { activated_at: Some(0) }` account at the gate's pubkey.
//! - [`emit`] — serializes the assembled `GenesisConfig` to `genesis.bin` and surfaces
//!   the resulting genesis hash + capitalization summary.
//! - [`pdas`] — well-known PDA + program-ID constants (treasury, lazy-claim config,
//!   the five program IDs). All match what the on-chain programs actually expect, so
//!   the genesis-side derivations and the program-side derivations agree.
//!
//! ## Pipeline
//!
//! ```text
//!     composed-genesis.json (step 20)         identity/vote/stake/faucet keypairs
//!                  │                                          │
//!                  ▼                                          ▼
//!              load_composed                              load_keypairs
//!                  │                                          │
//!                  └──────────────────┬──────────────────────┘
//!                                     ▼
//!                                bake(BakeInputs)
//!                                     │
//!                                     ▼
//!                          GenesisConfig (in memory)
//!                                     │
//!                                     ▼
//!                              emit::write_bin
//!                                     │
//!                                     ▼
//!                                genesis.bin (step 30)
//! ```

pub mod accounts;
pub mod config;
pub mod emit;
pub mod features;
pub mod mints;
pub mod pdas;
pub mod programs;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use solana_cluster_type::ClusterType;
use solana_genesis_config::GenesisConfig;
use solana_keypair::{read_keypair_file, Keypair};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use staccana_genesis_emit::ComposedGenesis;

pub use config::{assemble_genesis_config, BakeSummary};
pub use pdas::{
    lazy_claim_config_pda, treasury_pda, BRIDGE_PROGRAM_ID, LAZY_CLAIM_PROGRAM_ID,
    LAZY_CLAIM_CONFIG_SEED, MEGADROP_PROGRAM_ID, SECRET_PUMP_PROGRAM_ID, TREASURY_SEED,
    VALIDATOR_SUBSIDY_PROGRAM_ID,
};

/// Default lamport allocation for each of the bootstrap-validator-related accounts
/// (identity, vote, stake, faucet). Bumped from 1 SOL → 1000 SOL so the validator
/// identities have enough lamports to pay the rent for deploying our 5 programs
/// (~8 SOL total across all .so binaries; bridge alone needs ~2.3 SOL).
pub const BOOTSTRAP_LAMPORTS: u64 = 1_000_000_000_000;

/// All inputs required to bake the genesis. Assembled by [`load_inputs_from_paths`] (or
/// constructed by hand in tests).
///
/// `.so` paths are optional. For the mainnet-sigma launch night every program should be
/// supplied; in dev / staging it's useful to skip programs whose binary hasn't been
/// built yet — the chain still boots, those programs just need a post-boot
/// `solana program deploy` to materialize.
///
/// `cluster_type` controls which `ClusterType` enum variant is baked into the
/// `GenesisConfig`. The runtime branches on this for things like default warmup
/// behavior and feature-gate auto-activation; for the mainnet-sigma launch this
/// will be `MainnetBeta`, but for the devnet shake-out tonight we want
/// `Development` so the validator doesn't try to apply mainnet-only invariants
/// to a single-node throwaway chain. Default is `Development`.
pub struct BakeInputs {
    pub composed: ComposedGenesis,
    pub identity: Keypair,
    pub vote: Keypair,
    pub stake: Keypair,
    pub faucet: Keypair,
    pub cluster_type: ClusterType,
    /// Additional bootstrap validators beyond the primary one. Each entry is
    /// `(identity, vote, stake)`. Genesis will materialize identity (system-owned,
    /// 1 SOL), vote (vote-program-owned, 1 SOL, node_pubkey=identity), and stake
    /// (stake-program-owned, 1 SOL, delegated to vote, activation_epoch=u64::MAX —
    /// the bootstrap-stake marker) for each one — same byte layout as the primary
    /// validator.
    ///
    /// **Why this exists**: agave 2.0.x's tower-BFT threshold check has a
    /// single-validator bootstrap deadlock — the very first vote can't satisfy the
    /// supermajority condition because no prior vote has landed, and no prior
    /// vote can land because the threshold rejects every attempt. With ≥2
    /// validators in genesis, each one's first vote can pass through the "tower
    /// not deep enough" escape independently, and once they observe each other's
    /// votes via gossip, the threshold check converges. solana-test-validator
    /// sidesteps this with internal-only bank manipulation; vanilla
    /// `agave-validator` doesn't have an equivalent flag.
    pub additional_validators: Vec<AdditionalBootstrapValidator>,
    pub lazy_claim_so: Option<PathBuf>,
    pub bridge_so: Option<PathBuf>,
    pub secret_pump_so: Option<PathBuf>,
    pub validator_subsidy_so: Option<PathBuf>,
    pub megadrop_so: Option<PathBuf>,
    /// SPL stack baked at canonical mainnet pubkeys (sidesteps Anchor's
    /// hardcoded program-id checks in `Program<'info, Token2022>` etc.).
    pub spl_token_so: Option<PathBuf>,
    pub spl_token_2022_so: Option<PathBuf>,
    pub spl_associated_token_so: Option<PathBuf>,
    pub spl_memo_so: Option<PathBuf>,
    /// AddressLookupTable program (`AddressLookupTab1e1111111111111111111111111`).
    /// In agave 2.3+ this is no longer a native builtin — it's a core-BPF
    /// program that has to be deployed at the canonical address. Without
    /// this, every v0 transaction referencing a LUT pre-flight-rejects
    /// with `ProgramAccountNotFound`. Source `.so`:
    /// `solana-program-test-2.3.13/src/programs/core_bpf_address_lookup_table-3.0.0.so`.
    pub address_lookup_table_so: Option<PathBuf>,
    /// Upgrade authority baked into the staccana programs' `ProgramData`
    /// header at slot 0. `None` (the historical default) hard-freezes them
    /// immutable forever — which is what bit us when the proof-buffer
    /// additions arrived: re-deploying the patched .so was impossible
    /// without another full genesis rebake.
    ///
    /// Set this to a pubkey held by the bake operator (typically the
    /// deployer keypair) so future patches can ship via
    /// `solana program deploy --program-id ... --upgrade-authority ...`
    /// against `rpc.mp.fun` instead of needing to re-bake the chain.
    ///
    /// SPL programs always bake immutable regardless — they're upstream
    /// canonical and we never want to upgrade them out from under user txs.
    pub staccana_program_upgrade_authority: Option<Pubkey>,
}

/// Keypair triplet for a non-primary bootstrap validator. The primary validator's
/// triplet lives at the top level of [`BakeInputs`]; secondary ones go in
/// [`BakeInputs::additional_validators`].
pub struct AdditionalBootstrapValidator {
    pub identity: Keypair,
    pub vote: Keypair,
    pub stake: Keypair,
}

impl AdditionalBootstrapValidator {
    pub fn identity_pubkey(&self) -> Pubkey {
        self.identity.pubkey()
    }
    pub fn vote_pubkey(&self) -> Pubkey {
        self.vote.pubkey()
    }
    pub fn stake_pubkey(&self) -> Pubkey {
        self.stake.pubkey()
    }
}

impl BakeInputs {
    /// Convenience accessor — pubkey of the bootstrap-validator identity account.
    pub fn identity_pubkey(&self) -> Pubkey {
        self.identity.pubkey()
    }
    /// Convenience accessor — pubkey of the vote account.
    pub fn vote_pubkey(&self) -> Pubkey {
        self.vote.pubkey()
    }
    /// Convenience accessor — pubkey of the stake account.
    pub fn stake_pubkey(&self) -> Pubkey {
        self.stake.pubkey()
    }
    /// Convenience accessor — pubkey of the faucet account.
    pub fn faucet_pubkey(&self) -> Pubkey {
        self.faucet.pubkey()
    }
}

/// Read the `ComposedGenesis` JSON written by `staccana-genesis-emit`. Same on-disk
/// format the existing CLI produces — plain serde-json.
pub fn load_composed_genesis(path: impl AsRef<Path>) -> Result<ComposedGenesis> {
    let path = path.as_ref();
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading composed genesis from {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("parsing composed genesis JSON at {}", path.display()))
}

/// Read a single keypair file in the standard `solana-keygen` JSON format
/// (`[byte, byte, byte, ...]` of the secret-key concatenated with the pubkey, 64 bytes
/// total).
pub fn load_keypair(path: impl AsRef<Path>) -> Result<Keypair> {
    let path = path.as_ref();
    read_keypair_file(path).map_err(|e| {
        anyhow::anyhow!("reading keypair from {}: {}", path.display(), e)
    })
}

/// Load every input from disk. Wraps [`load_composed_genesis`] + four
/// [`load_keypair`] calls + path passthroughs.
#[allow(clippy::too_many_arguments)]
pub fn load_inputs_from_paths(
    composed: impl AsRef<Path>,
    identity: impl AsRef<Path>,
    vote: impl AsRef<Path>,
    stake: impl AsRef<Path>,
    faucet: impl AsRef<Path>,
    cluster_type: ClusterType,
    additional_validator_keypair_triplets: Vec<(PathBuf, PathBuf, PathBuf)>,
    lazy_claim_so: Option<PathBuf>,
    bridge_so: Option<PathBuf>,
    secret_pump_so: Option<PathBuf>,
    validator_subsidy_so: Option<PathBuf>,
    megadrop_so: Option<PathBuf>,
    spl_token_so: Option<PathBuf>,
    spl_token_2022_so: Option<PathBuf>,
    spl_associated_token_so: Option<PathBuf>,
    spl_memo_so: Option<PathBuf>,
    address_lookup_table_so: Option<PathBuf>,
    staccana_program_upgrade_authority: Option<Pubkey>,
) -> Result<BakeInputs> {
    let additional_validators = additional_validator_keypair_triplets
        .into_iter()
        .map(|(id_path, vote_path, stake_path)| {
            Ok::<_, anyhow::Error>(AdditionalBootstrapValidator {
                identity: load_keypair(id_path)?,
                vote: load_keypair(vote_path)?,
                stake: load_keypair(stake_path)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(BakeInputs {
        composed: load_composed_genesis(composed)?,
        identity: load_keypair(identity)?,
        vote: load_keypair(vote)?,
        stake: load_keypair(stake)?,
        faucet: load_keypair(faucet)?,
        cluster_type,
        additional_validators,
        lazy_claim_so,
        bridge_so,
        secret_pump_so,
        validator_subsidy_so,
        megadrop_so,
        spl_token_so,
        spl_token_2022_so,
        spl_associated_token_so,
        spl_memo_so,
        address_lookup_table_so,
        staccana_program_upgrade_authority,
    })
}

/// End-to-end entrypoint. Takes the loaded inputs, builds a [`GenesisConfig`], and
/// returns it together with a [`BakeSummary`] of what got injected (used for the CLI's
/// stdout report).
///
/// Pure function modulo `.so` file reads — the on-disk write happens in [`emit`].
pub fn bake(inputs: &BakeInputs) -> Result<(GenesisConfig, BakeSummary)> {
    config::assemble_genesis_config(inputs)
}

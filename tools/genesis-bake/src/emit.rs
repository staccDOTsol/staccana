//! Serialize the assembled [`GenesisConfig`] to disk and (for production) seed the
//! blockstore with the genesis tick entries the validator needs to boot.
//!
//! Three-mode emission:
//!
//! - [`write_ledger`] — **production path**. Calls
//!   `solana_ledger::blockstore::create_new_ledger`, which writes `genesis.bin`,
//!   creates a fresh rocksdb blockstore, materializes slot 0 with PoH tick entries
//!   chained off the genesis hash, sets root to slot 0, and produces
//!   `genesis.tar.bz2`. This is the only path that produces a ledger directory the
//!   validator can boot from without crashing in `process_bank_0` with
//!   `InvalidBlock(Incomplete)` (which is what happens when only `genesis.bin` is
//!   written and the blockstore has no slot-0 shreds).
//! - [`write_genesis_to_ledger_dir`] — writes only `genesis.bin` via
//!   `GenesisConfig::write`. **Not bootable on its own** — preserved for callers
//!   that want to inspect the genesis bytes without paying the cost of seeding the
//!   blockstore. Tests use this; production should use [`write_ledger`].
//! - [`write_genesis_bin_at_path`] — direct bincode emission at an explicit path.
//!   Same caveat as above: not bootable, useful only for tests.
//!
//! All three modes return the genesis hash via `GenesisConfig::hash` — the validator
//! computes the same hash internally; matching it is the launch-day gate.

use std::path::Path;

use anyhow::{Context, Result};
use solana_cluster_type::ClusterType;
use solana_genesis_config::GenesisConfig;
use solana_hash::Hash;
use solana_ledger::blockstore::create_new_ledger;
use solana_ledger::blockstore_options::LedgerColumnOptions;

use crate::BakeSummary;

/// Upper bound on the unpacked size of `genesis.tar.bz2` that
/// [`create_new_ledger`] will accept after re-unpacking it as a sanity check.
/// `u64::MAX` disables the check — we trust our own genesis bytes (they're the bytes
/// we just wrote five lines earlier), and our staccana genesis can be larger than
/// the agave default once the five program ELFs land in it.
const NO_GENESIS_ARCHIVE_SIZE_LIMIT: u64 = u64::MAX;

/// **Production path.** Write the full ledger directory the validator boots from.
///
/// Calls `solana_ledger::blockstore::create_new_ledger`, which:
///
///   1. Destroys any existing blockstore at `ledger_dir` (idempotent re-runs are
///      safe).
///   2. Writes `genesis.bin` (`config.write(ledger_dir)`).
///   3. Opens a fresh rocksdb blockstore.
///   4. Materializes slot 0 by calling `create_ticks(ticks_per_slot,
///      hashes_per_tick, genesis_hash)`, shredding the resulting entries, and
///      inserting them. The first tick chains off `genesis_hash`; subsequent ticks
///      chain off the previous tick.
///   5. Calls `set_roots(&[0])` so bank 0 is rooted.
///   6. Produces `genesis.tar.bz2` (a tar+bzip2 of `genesis.bin` + the rocksdb
///      directory) and re-unpacks it into a temp dir as a sanity check.
///
/// Without step 4 the validator's `process_blockstore_for_bank_0` finds no slot-0
/// shreds and panics with `InvalidBlock(Incomplete)` — that's the single failure
/// mode this function exists to fix.
///
/// Returns the genesis hash (`config.hash()`), NOT the last-tick hash that
/// `create_new_ledger` itself returns. The genesis hash is what shows up in the
/// validator's startup logs and what we report to the operator.
pub fn write_ledger(
    config: &GenesisConfig,
    ledger_dir: impl AsRef<Path>,
) -> Result<Hash> {
    let ledger_dir = ledger_dir.as_ref();
    if let Some(parent) = ledger_dir.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "creating ledger parent directory {} before bake",
                    parent.display()
                )
            })?;
        }
    }
    std::fs::create_dir_all(ledger_dir).with_context(|| {
        format!("creating ledger directory {} before bake", ledger_dir.display())
    })?;

    let _last_tick_hash = create_new_ledger(
        ledger_dir,
        config,
        NO_GENESIS_ARCHIVE_SIZE_LIMIT,
        LedgerColumnOptions::default(),
    )
    .with_context(|| {
        format!(
            "create_new_ledger failed for {} — genesis.bin may have been written but \
             the blockstore is incomplete; remove the directory and re-run",
            ledger_dir.display()
        )
    })?;

    Ok(config.hash())
}

/// Write only `genesis.bin` to a directory. **Not bootable** on its own — the
/// resulting directory has no rocksdb blockstore and no slot-0 shreds, so a
/// validator pointed at it will panic. Use [`write_ledger`] for production.
///
/// Preserved for tests and inspection. Returns the genesis hash.
pub fn write_genesis_to_ledger_dir(
    config: &GenesisConfig,
    ledger_dir: impl AsRef<Path>,
) -> Result<Hash> {
    let ledger_dir = ledger_dir.as_ref();
    config
        .write(ledger_dir)
        .with_context(|| format!("writing genesis.bin under {}", ledger_dir.display()))?;
    Ok(config.hash())
}

/// Write the genesis to a specific path (rather than `<dir>/genesis.bin`).
///
/// Useful when the caller has a precise output filename in mind. Internally serializes
/// via `bincode::serialize` — the byte format is identical to what
/// `GenesisConfig::write` produces.
pub fn write_genesis_bin_at_path(
    config: &GenesisConfig,
    output_path: impl AsRef<Path>,
) -> Result<Hash> {
    let path = output_path.as_ref();
    let bytes = bincode::serialize(config)
        .context("serializing GenesisConfig via bincode")?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent directory {}", parent.display()))?;
        }
    }
    std::fs::write(path, &bytes)
        .with_context(|| format!("writing genesis.bin to {}", path.display()))?;
    Ok(config.hash())
}

/// Pretty-print a [`BakeSummary`] to stderr in the same multi-line format the existing
/// staccana tools use. Caller decides whether to call this; the library doesn't
/// touch stdio on its own.
pub fn log_bake_summary(summary: &BakeSummary, genesis_hash: &Hash, cluster_type: ClusterType) {
    eprintln!("[bake] genesis hash:           {}", genesis_hash);
    eprintln!("[bake] cluster type:           {:?}", cluster_type);
    if !summary.additional_bootstrap_pubkeys.is_empty() {
        eprintln!("[bake] bootstrap validators:   {} (1 primary + {} additional)",
            summary.additional_bootstrap_pubkeys.len() + 1,
            summary.additional_bootstrap_pubkeys.len());
        for (i, pks) in summary.additional_bootstrap_pubkeys.iter().enumerate() {
            eprintln!("[bake]   extra-{}.identity:    {}", i + 2, pks.identity);
            eprintln!("[bake]   extra-{}.vote:        {}", i + 2, pks.vote);
            eprintln!("[bake]   extra-{}.stake:       {}", i + 2, pks.stake);
        }
    }
    eprintln!("[bake] bootstrap identity:     {}", summary.bootstrap_pubkeys.identity);
    eprintln!("[bake] bootstrap vote:         {}", summary.bootstrap_pubkeys.vote);
    eprintln!("[bake] bootstrap stake:        {}", summary.bootstrap_pubkeys.stake);
    eprintln!("[bake] faucet:                 {}", summary.bootstrap_pubkeys.faucet);
    eprintln!("[bake] treasury PDA:           {}", summary.treasury_pda);
    eprintln!(
        "[bake] treasury lamports:      {} ({:.4} SOL)",
        summary.treasury_lamports,
        summary.treasury_lamports as f64 / 1_000_000_000.0
    );
    eprintln!("[bake] lazy-claim config PDA:  {}", summary.lazy_claim_config_pda);
    eprintln!("[bake] claimable_root (hex):   0x{}", summary.claimable_root_hex);
    eprintln!("[bake] claimable count:        {}", summary.claimable_count);
    eprintln!("[bake] BPF programs installed: {}", summary.programs_installed.len());
    for p in &summary.programs_installed {
        eprintln!(
            "[bake]   - {} @ {} (data {} bytes)",
            p.name, p.program_id, p.elf_bytes
        );
    }
    eprintln!(
        "[bake] native processors:      {}",
        summary.native_programs_installed.len()
    );
    for (n, id) in &summary.native_programs_installed {
        eprintln!("[bake]   - {} @ {}", n, id);
    }
    eprintln!(
        "[bake] CTE feature gates ON:   {}",
        summary.feature_gates_activated.len()
    );
    for g in &summary.feature_gates_activated {
        eprintln!("[bake]   - {}", g);
    }
    eprintln!("[bake] total accounts:         {}", summary.total_accounts);
    eprintln!("[bake] total lamports:         {}", summary.total_lamports);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bake, BakeInputs};
    use solana_keypair::Keypair;
    use staccana_genesis::{
        ClassicDefaults, MerkleRoot, CTE_FEATURE_GATES_AT_GENESIS,
    };
    use staccana_genesis_emit::{ActiveFeatureGate, ComposedGenesis, LazyClaimGenesisAccount};
    use solana_program::hash::Hash as ProgramHash;

    fn synthetic_inputs() -> BakeInputs {
        BakeInputs {
            composed: ComposedGenesis {
                fee_governor: ClassicDefaults::fee_rate_governor(),
                inflation_disabled: true,
                active_feature_gates: CTE_FEATURE_GATES_AT_GENESIS
                    .iter()
                    .map(|(pk, desc)| ActiveFeatureGate {
                        pubkey_b58: (*pk).to_string(),
                        description: (*desc).to_string(),
                    })
                    .collect(),
                treasury_pda_lamports: 1_000,
                treasury_account_count: 1,
                lazy_claim_account: LazyClaimGenesisAccount::from_root(MerkleRoot(
                    ProgramHash::new_from_array([0x42; 32]),
                )),
                claimable_count: 1,
                bank_hash_seed: "STACCANA_GENESIS_V1".to_string(),
            },
            identity: Keypair::new(),
            vote: Keypair::new(),
            stake: Keypair::new(),
            faucet: Keypair::new(),
            cluster_type: ClusterType::Development,
            additional_validators: Vec::new(),
            lazy_claim_so: None,
            bridge_so: None,
            secret_pump_so: None,
            validator_subsidy_so: None,
            megadrop_so: None,
            spl_token_so: None,
            spl_token_2022_so: None,
            spl_associated_token_so: None,
            spl_memo_so: None,
            address_lookup_table_so: None,
            staccana_program_upgrade_authority: None,
        }
    }

    #[test]
    fn write_genesis_to_ledger_dir_creates_genesis_bin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = synthetic_inputs();
        let (config, _summary) = bake(&inputs).expect("bake");
        let hash = write_genesis_to_ledger_dir(&config, dir.path()).expect("write");

        let bin = dir.path().join("genesis.bin");
        assert!(bin.exists(), "genesis.bin must exist at {}", bin.display());
        assert!(bin.metadata().unwrap().len() > 0, "genesis.bin must be non-empty");

        // Reload via GenesisConfig::load and confirm the round-tripped hash matches.
        let reloaded = GenesisConfig::load(dir.path()).expect("load");
        assert_eq!(reloaded.hash(), hash);
    }

    #[test]
    fn write_ledger_creates_blockstore_and_archive() {
        // The single regression test for the `InvalidBlock(Incomplete)` panic. A
        // bootable ledger produced by `write_ledger` must contain:
        //   - genesis.bin     (the GenesisConfig bytes)
        //   - genesis.tar.bz2 (tar+bzip2 of genesis.bin + the rocksdb dir)
        //   - rocksdb/        (the blockstore with slot 0 shreds + root)
        // If ANY of those is missing, the validator's `process_blockstore_for_bank_0`
        // will panic with `InvalidBlock(Incomplete)`. We pin all three on disk and
        // round-trip the genesis hash for good measure.
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = synthetic_inputs();
        let (config, _summary) = bake(&inputs).expect("bake");

        let hash = write_ledger(&config, dir.path()).expect("write_ledger");

        let bin = dir.path().join("genesis.bin");
        assert!(bin.exists(), "genesis.bin must exist at {}", bin.display());
        assert!(bin.metadata().unwrap().len() > 0, "genesis.bin must be non-empty");

        let archive = dir.path().join("genesis.tar.bz2");
        assert!(
            archive.exists(),
            "genesis.tar.bz2 must exist at {} (snapshot bootstrappers fetch it)",
            archive.display()
        );

        let rocksdb_dir = dir.path().join("rocksdb");
        assert!(
            rocksdb_dir.exists() && rocksdb_dir.is_dir(),
            "rocksdb/ blockstore directory must exist at {} — its absence is the \
             root cause of `InvalidBlock(Incomplete)` at process_bank_0",
            rocksdb_dir.display()
        );
        // The rocksdb dir should have at least one SST file by the time we've
        // inserted slot 0 shreds and set the root.
        let rocksdb_entries: Vec<_> = std::fs::read_dir(&rocksdb_dir).unwrap().collect();
        assert!(
            !rocksdb_entries.is_empty(),
            "rocksdb/ must contain at least one file after seeding slot 0"
        );

        // Genesis hash must match what reloading the bytes from disk produces.
        let reloaded = GenesisConfig::load(dir.path()).expect("load");
        assert_eq!(reloaded.hash(), hash);
    }

    #[test]
    fn write_ledger_is_idempotent_across_reruns() {
        // `create_new_ledger` calls `Blockstore::destroy(ledger_path)` first, so
        // re-running the bake against the same directory must succeed and produce
        // the same genesis hash. This matches how step 30 of the deploy pipeline
        // is expected to behave when the operator re-runs after a crash.
        let dir = tempfile::tempdir().expect("tempdir");
        let inputs = synthetic_inputs();
        let (config, _) = bake(&inputs).expect("bake");

        let hash1 = write_ledger(&config, dir.path()).expect("first write_ledger");
        let hash2 = write_ledger(&config, dir.path()).expect("second write_ledger");
        assert_eq!(hash1, hash2, "genesis hash must be deterministic across reruns");
    }

    #[test]
    fn write_genesis_bin_at_path_writes_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("custom").join("genesis.bin");
        let inputs = synthetic_inputs();
        let (config, _summary) = bake(&inputs).expect("bake");
        let hash = write_genesis_bin_at_path(&config, &path).expect("write");

        assert!(path.exists());
        let raw = std::fs::read(&path).expect("read");
        assert!(!raw.is_empty());
        // The hash returned must match what re-parsing the file produces.
        let reparsed: GenesisConfig =
            bincode::deserialize(&raw).expect("re-deserialize");
        assert_eq!(reparsed.hash(), hash);
    }

    #[test]
    fn genesis_hash_is_deterministic_for_same_inputs() {
        // Two bakes with the SAME composed input + SAME bootstrap pubkeys produce the
        // same genesis hash. We pin the keypairs by serializing/deserializing them
        // through bytes since `Keypair::new()` is random.
        let mut inputs1 = synthetic_inputs();
        let mut inputs2 = synthetic_inputs();

        // Override randoms with deterministic ones so the test isn't comparing apples
        // to oranges.
        let id_bytes = inputs1.identity.to_bytes();
        inputs2.identity = Keypair::try_from(&id_bytes[..]).unwrap();
        let v_bytes = inputs1.vote.to_bytes();
        inputs2.vote = Keypair::try_from(&v_bytes[..]).unwrap();
        let s_bytes = inputs1.stake.to_bytes();
        inputs2.stake = Keypair::try_from(&s_bytes[..]).unwrap();
        let f_bytes = inputs1.faucet.to_bytes();
        inputs2.faucet = Keypair::try_from(&f_bytes[..]).unwrap();

        // Pin creation_time too (it's set from system clock by default).
        let (mut c1, _) = bake(&inputs1).expect("bake1");
        let (mut c2, _) = bake(&inputs2).expect("bake2");
        c1.creation_time = 1_700_000_000;
        c2.creation_time = 1_700_000_000;

        // Hashes must agree.
        let h1 = c1.hash();
        let h2 = c2.hash();
        assert_eq!(h1, h2);

        // Distinct random data ⇒ distinct hashes (regression check on the
        // determinism wiring).
        inputs1.identity = Keypair::new();
        let (mut c3, _) = bake(&inputs1).expect("bake3");
        c3.creation_time = 1_700_000_000;
        assert_ne!(c1.hash(), c3.hash());

        // Suppress the unused vars warning.
        let _ = inputs2;
    }
}

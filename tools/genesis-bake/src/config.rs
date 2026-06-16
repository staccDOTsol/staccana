//! Assemble the [`GenesisConfig`].
//!
//! This is the orchestration layer: it pulls together the bootstrap accounts, the
//! treasury PDA, the lazy-claim Config singleton, the program registrations, and the
//! feature gate accounts, then writes them all into a single
//! [`solana_genesis_config::GenesisConfig`] with the right top-level economic policy
//! (fee governor, inflation, cluster type).
//!
//! Pure function modulo the `.so` filesystem reads pulled in via
//! [`crate::programs::build_program_pair_from_path`]. The resulting `GenesisConfig` is
//! ready for `GenesisConfig::write` (or `bincode::serialize_into`) — see [`crate::emit`].

use anyhow::{Context, Result};
use solana_epoch_schedule::EpochSchedule;
use solana_fee_calculator::FeeRateGovernor;
use solana_genesis_config::GenesisConfig;
use solana_inflation::Inflation;
use solana_pubkey::Pubkey;
// `ClusterType` is referenced by tests below; the production code path consumes it
// via `inputs.cluster_type` (typed at the BakeInputs boundary in `lib.rs`), so no
// module-level import is needed for the non-test build.

use staccana_genesis::FeeRateGovernor as ComposedFeeGovernor;

use crate::accounts::{
    bootstrap_identity_account, bootstrap_stake_account, bootstrap_vote_account, faucet_account,
    lazy_claim_config_account, treasury_account,
};
use crate::features::build_all_feature_accounts;
use crate::programs::{
    build_program_pair_from_path, canonical_slots, native_program_account,
    zk_elgamal_proof_native_processor, ProgramPair,
};
use crate::BakeInputs;

/// Summary of what the bake injected. Used for the CLI's stdout report and the
/// integration tests' assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BakeSummary {
    pub bootstrap_pubkeys: BootstrapPubkeys,
    /// Additional bootstrap validators baked into genesis beyond the primary one.
    /// Each one has the same vote+stake+identity shape as the primary; they exist
    /// to break agave 2.0.x's solo-validator tower-BFT deadlock.
    pub additional_bootstrap_pubkeys: Vec<BootstrapPubkeys>,
    pub treasury_pda: Pubkey,
    pub treasury_lamports: u64,
    pub lazy_claim_config_pda: Pubkey,
    pub claimable_root_hex: String,
    pub claimable_count: u64,
    pub programs_installed: Vec<ProgramSummary>,
    pub feature_gates_activated: Vec<Pubkey>,
    pub native_programs_installed: Vec<(String, Pubkey)>,
    pub total_accounts: usize,
    pub total_lamports: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapPubkeys {
    pub identity: Pubkey,
    pub vote: Pubkey,
    pub stake: Pubkey,
    pub faucet: Pubkey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramSummary {
    pub name: &'static str,
    pub program_id: Pubkey,
    pub program_data_address: Pubkey,
    pub elf_bytes: usize,
}

/// Assemble the [`GenesisConfig`] from the loaded inputs.
///
/// Step-by-step:
///
/// 1. Convert the composed-genesis [`ComposedFeeGovernor`] mirror into the real
///    [`solana_fee_calculator::FeeRateGovernor`] — same field names, just different
///    type universe.
/// 2. Pick `Inflation::new_disabled()` since `composed.inflation_disabled` is always
///    `true` for staccana v2 (we still validate it as a guard against accidentally
///    booting an inflationary chain).
/// 3. Construct an empty `GenesisConfig` with cluster type `MainnetBeta`, then mutate
///    in the bootstrap accounts, treasury, lazy-claim config, programs, and features.
/// 4. Register `solana_zk_elgamal_proof_program` as a native instruction processor —
///    the runtime needs this to dispatch the ZK proof program calls that CTE uses.
/// 5. Build the [`BakeSummary`] alongside.
pub fn assemble_genesis_config(inputs: &BakeInputs) -> Result<(GenesisConfig, BakeSummary)> {
    if !inputs.composed.inflation_disabled {
        anyhow::bail!(
            "ComposedGenesis.inflation_disabled = false; staccana v2 requires inflation off"
        );
    }

    let fee_rate_governor = convert_fee_governor(&inputs.composed.fee_governor);
    let inflation = Inflation::new_disabled();

    // EpochSchedule with `warmup=false` so the four bootstrap validators we bake
    // with `Epoch::MAX` delegation markers all count as fully-active stake from
    // slot 0. With the default schedule (slots_per_epoch=432_000, warmup=true)
    // the runtime runs every stake — even Epoch::MAX bootstrap stakes — through
    // the warmup curve, leaving only the primary bootstrap with positive
    // active stake at epoch 0. The leader schedule then has only val-1 in it
    // for ~432k slots, so as soon as val-1 finishes its bootstrap leader
    // window (slots 0-3) the chain stalls forever waiting for non-existent
    // leaders. `slots_per_epoch=432_000` is kept (matches mainnet); the
    // change is solely the warmup flag.
    let epoch_schedule = EpochSchedule::custom(432_000, 432_000, false);

    let mut config = GenesisConfig {
        fee_rate_governor,
        inflation,
        epoch_schedule,
        cluster_type: inputs.cluster_type,
        ..GenesisConfig::default()
    };

    // ---- Bootstrap accounts ----
    let identity_pk = inputs.identity_pubkey();
    let vote_pk = inputs.vote_pubkey();
    let stake_pk = inputs.stake_pubkey();
    let faucet_pk = inputs.faucet_pubkey();

    let (k, a) = bootstrap_identity_account(identity_pk);
    config.add_account(k, a);
    // Vote first — the stake account constructor reads the vote account's serialized
    // VoteState to wire the delegation. Order matters here.
    let (k, vote_acct) = bootstrap_vote_account(vote_pk, identity_pk);
    config.add_account(k, vote_acct.clone());
    let (k, a) = bootstrap_stake_account(stake_pk, vote_pk, &vote_acct);
    config.add_account(k, a);
    let (k, a) = faucet_account(faucet_pk);
    config.add_account(k, a);

    // ---- Additional bootstrap validators ----
    //
    // For each extra `(identity, vote, stake)` triplet, materialize the same three-account
    // shape we just did for the primary. This is what unblocks agave 2.0.x's tower-BFT
    // single-validator deadlock — with ≥2 staked validators in the genesis bank, each
    // one's first-vote bootstrap escape can fire independently (each has 0 prior votes,
    // so `vote_state.nth_recent_lockout(threshold_depth)` returns None → PassedThreshold)
    // and once their vote txs land via gossip, voted_stakes[N] becomes non-zero for
    // future threshold checks. solo bootstrap deadlocks because the lone validator can
    // never satisfy the threshold against its own (0-vote) vote-account state on chain.
    let mut additional_bootstrap_pubkeys: Vec<BootstrapPubkeys> =
        Vec::with_capacity(inputs.additional_validators.len());
    for extra in &inputs.additional_validators {
        let id_pk = extra.identity_pubkey();
        let vote_pk = extra.vote_pubkey();
        let stake_pk = extra.stake_pubkey();
        let (k, a) = bootstrap_identity_account(id_pk);
        config.add_account(k, a);
        let (k, vote_acct) = bootstrap_vote_account(vote_pk, id_pk);
        config.add_account(k, vote_acct.clone());
        let (k, a) = bootstrap_stake_account(stake_pk, vote_pk, &vote_acct);
        config.add_account(k, a);
        additional_bootstrap_pubkeys.push(BootstrapPubkeys {
            identity: id_pk,
            vote: vote_pk,
            stake: stake_pk,
            // Reuse the primary faucet pubkey for the summary — extra validators don't
            // get their own faucet (only one faucet keypair makes sense per cluster).
            faucet: faucet_pk,
        });
    }

    // ---- Stake program genesis accounts (config + epoch rewards sysvar) ----
    //
    // `solana_runtime::genesis_utils::create_genesis_config_with_leader_ex_no_features`
    // calls this same helper at the end of its bootstrap. It installs:
    //   - the stake config program account, and
    //   - the epoch_rewards sysvar account.
    // Both are runtime prerequisites — without them, the bank-bootstrap path that
    // resolves the stake delegation can't load the stake program's config (used for
    // warmup/cooldown rate calculation), causing slot-0 init to fail.
    solana_stake_program::add_genesis_accounts(&mut config);

    // ---- Treasury PDA ----
    let treasury_lamports = inputs.composed.treasury_pda_lamports;
    let (treasury_pda, treasury_acct) = treasury_account(treasury_lamports);
    config.add_account(treasury_pda, treasury_acct);

    // ---- Lazy-claim Config singleton ----
    let claimable_root = inputs.composed.lazy_claim_account.claimable_root;
    let (lc_config_pda, lc_config_acct) = lazy_claim_config_account(claimable_root);
    config.add_account(lc_config_pda, lc_config_acct);

    // ---- Programs (BPF builtins via upgradeable loader) ----
    let slots = canonical_slots(
        inputs.lazy_claim_so.as_deref(),
        inputs.bridge_so.as_deref(),
        inputs.secret_pump_so.as_deref(),
        inputs.validator_subsidy_so.as_deref(),
        inputs.megadrop_so.as_deref(),
        inputs.spl_token_so.as_deref(),
        inputs.spl_token_2022_so.as_deref(),
        inputs.spl_associated_token_so.as_deref(),
        inputs.spl_memo_so.as_deref(),
        inputs.address_lookup_table_so.as_deref(),
    );
    // Staccana programs (lazy-claim, bridge, secret-pump, validator-subsidy,
    // megadrop) get the operator's upgrade authority baked in if one was
    // supplied — that lets us patch them post-boot without rebaking. SPL
    // programs always stay immutable (we never want to upgrade Token-2022
    // out from under live user txs).
    let staccana_program_ids: std::collections::HashSet<Pubkey> = [
        crate::pdas::LAZY_CLAIM_PROGRAM_ID,
        crate::pdas::BRIDGE_PROGRAM_ID,
        crate::pdas::SECRET_PUMP_PROGRAM_ID,
        crate::pdas::VALIDATOR_SUBSIDY_PROGRAM_ID,
        crate::pdas::MEGADROP_PROGRAM_ID,
    ]
    .into_iter()
    .collect();
    let staccana_authority = inputs.staccana_program_upgrade_authority;

    let mut programs_installed = Vec::with_capacity(slots.len());
    for slot in slots.iter() {
        let Some(path) = slot.so_path else {
            // Operator chose to skip this program — chain still boots; that program
            // can be deployed post-boot via `solana program deploy`. Logged in
            // BakeSummary by absence.
            continue;
        };
        let authority_for_this_slot = if staccana_program_ids.contains(&slot.program_id) {
            staccana_authority
        } else {
            None
        };
        let elf = std::fs::read(path)
            .with_context(|| format!("reading .so for {} at {}", slot.name, path.display()))?;
        let pair: ProgramPair = crate::programs::build_program_pair_with_authority(
            slot.program_id,
            elf,
            authority_for_this_slot,
        )
        .with_context(|| format!("building Program/ProgramData pair for {}", slot.name))?;
        programs_installed.push(ProgramSummary {
            name: slot.name,
            program_id: pair.program_id,
            program_data_address: pair.program_data_address,
            elf_bytes: pair.elf_bytes,
        });
        config.add_account(pair.program_id, pair.program_account);
        config.add_account(pair.program_data_address, pair.program_data_account);
    }

    // ---- Native programs ----
    //
    // Both registered via `add_native_instruction_processor`. Without these
    // entries, agave 3.x on `cluster_type != mainnet-beta` does NOT
    // auto-load them, and every tx that touches them pre-flight-rejects
    // with `ProgramAccountNotFound`.
    //
    //   * ZK ElGamal Proof (`ZkE1Gama1Proof11…`): required by Token-2022's
    //     ConfidentialTransfer / ConfidentialMintBurn extensions.
    //   * AddressLookupTable (`AddressLookupTab1e…`): required by every v0
    //     transaction. Hit by /launch/create, /validators init_subsidy,
    //     /claim proof-buffer flow, and every confidential transfer chain
    //     once they trip the 1232-byte legacy ceiling.
    // ZK ElGamal Proof IS still a native processor in agave 2.3 (gated by
    // feature `zk_elgamal_proof_program_enabled`). Both registrations needed.
    // AddressLookupTable, by contrast, has been migrated to core-BPF — it
    // enters via `canonical_slots()` further down, NOT here.
    let (zk_name, zk_id) = zk_elgamal_proof_native_processor();
    config.add_native_instruction_processor(zk_name.clone(), zk_id);
    config.add_account(zk_id, native_program_account(&zk_name));
    let native_programs_installed = vec![(zk_name, zk_id)];

    // ---- Bridge asset Token-22 mints (wsol/stsol/ssusdc) ----
    //
    // Pre-baked at deterministic addresses with mint_authority = bridge per-asset
    // PDA. Once the chain boots, `bridge::mint` and `bridge::burn` invoke_signed
    // against those PDAs to move supply — no separate post-boot mint creation
    // step is needed. wSOL specifically bakes at canonical
    // `So11111111111111111111111111111111111111112` so Token-22's `sync_native`
    // wrap/unwrap semantics work.
    for slot in crate::mints::canonical_mint_slots().iter() {
        let (pk, acct) = crate::mints::build_mint_account(slot)
            .with_context(|| format!("building bridge mint {} ({})", slot.name, slot.pubkey))?;
        config.add_account(pk, acct);
    }

    // ---- Feature gates ----
    let feature_accounts =
        build_all_feature_accounts(&inputs.composed.active_feature_gates)
            .context("building CTE feature accounts")?;
    let mut feature_gates_activated = Vec::with_capacity(feature_accounts.len());
    for (k, a) in feature_accounts {
        feature_gates_activated.push(k);
        config.add_account(k, a);
    }

    // ---- Tallies ----
    let total_accounts = config.accounts.len();
    let total_lamports: u64 = config.accounts.values().map(|a| a.lamports).sum();
    let claimable_root_hex = bytes_to_hex(&claimable_root);

    let summary = BakeSummary {
        bootstrap_pubkeys: BootstrapPubkeys {
            identity: identity_pk,
            vote: vote_pk,
            stake: stake_pk,
            faucet: faucet_pk,
        },
        additional_bootstrap_pubkeys,
        treasury_pda,
        treasury_lamports,
        lazy_claim_config_pda: lc_config_pda,
        claimable_root_hex,
        claimable_count: inputs.composed.claimable_count,
        programs_installed,
        feature_gates_activated,
        native_programs_installed,
        total_accounts,
        total_lamports,
    };

    Ok((config, summary))
}

/// Convert the `staccana-genesis` mirror of `FeeRateGovernor` (which avoids the heavy
/// `solana-fee-calculator` dep) into the real `solana_fee_calculator::FeeRateGovernor`.
/// Same field names, byte-equivalent semantics.
fn convert_fee_governor(g: &ComposedFeeGovernor) -> FeeRateGovernor {
    FeeRateGovernor {
        // The real type carries `lamports_per_signature` (current observed rate); we
        // pin it equal to the target for the fixed-fee model. This matches what
        // `FeeRateGovernor::new(target, signatures_per_slot=0)` would produce.
        lamports_per_signature: g.target_lamports_per_signature,
        target_lamports_per_signature: g.target_lamports_per_signature,
        target_signatures_per_slot: g.target_signatures_per_slot,
        min_lamports_per_signature: g.min_lamports_per_signature,
        max_lamports_per_signature: g.max_lamports_per_signature,
        burn_percent: g.burn_percent,
    }
}

/// Hex-encode a 32-byte hash for human-readable display in logs / the bake summary.
/// Avoids pulling in `hex` as a dep — we only need it for the summary log.
fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_cluster_type::ClusterType;
    use solana_keypair::Keypair;
    use solana_signer::Signer;
    use staccana_genesis_emit::{ActiveFeatureGate, ComposedGenesis, LazyClaimGenesisAccount};
    use staccana_genesis::{
        ClassicDefaults, MerkleRoot, CTE_FEATURE_GATES_AT_GENESIS,
    };
    use solana_program::hash::Hash;

    fn synthetic_composed() -> ComposedGenesis {
        ComposedGenesis {
            fee_governor: ClassicDefaults::fee_rate_governor(),
            inflation_disabled: true,
            active_feature_gates: CTE_FEATURE_GATES_AT_GENESIS
                .iter()
                .map(|(pk, desc)| ActiveFeatureGate {
                    pubkey_b58: (*pk).to_string(),
                    description: (*desc).to_string(),
                })
                .collect(),
            treasury_pda_lamports: 485_192_075_139_020_370,
            treasury_account_count: 12_345,
            lazy_claim_account: LazyClaimGenesisAccount::from_root(MerkleRoot(
                Hash::new_from_array([0xAB; 32]),
            )),
            claimable_count: 99_999,
            bank_hash_seed: "STACCANA_GENESIS_V1".to_string(),
        }
    }

    fn synthetic_inputs() -> BakeInputs {
        BakeInputs {
            composed: synthetic_composed(),
            identity: Keypair::new(),
            vote: Keypair::new(),
            stake: Keypair::new(),
            faucet: Keypair::new(),
            // Tests pin to MainnetBeta to preserve the existing assertions and so
            // the production-target invariants (capitalization math, etc) are the
            // ones being exercised. The CLI default is Development, but the
            // assemble_genesis_config function itself is cluster-type-agnostic.
            cluster_type: ClusterType::MainnetBeta,
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
    fn assemble_with_one_extra_validator_adds_three_accounts() {
        // Each additional validator triplet must yield exactly 3 new accounts
        // (identity + vote + stake), wired the same way as the primary.
        let mut inputs = synthetic_inputs();
        let baseline_total = assemble_genesis_config(&inputs).expect("bake1").1.total_accounts;

        inputs.additional_validators.push(crate::AdditionalBootstrapValidator {
            identity: Keypair::new(),
            vote: Keypair::new(),
            stake: Keypair::new(),
        });
        let (_, summary) = assemble_genesis_config(&inputs).expect("bake2");
        assert_eq!(summary.total_accounts, baseline_total + 3);
        assert_eq!(summary.additional_bootstrap_pubkeys.len(), 1);
        // Vote pubkey of the extra validator must be different from the primary's.
        assert_ne!(
            summary.additional_bootstrap_pubkeys[0].vote,
            summary.bootstrap_pubkeys.vote
        );
    }

    #[test]
    fn assemble_extra_validator_stake_account_decodes_with_active_delegation() {
        // The whole point of adding extra validators is to break the solo tower-BFT
        // deadlock. For that to work, each extra validator's stake account MUST be
        // a fully-active StakeStateV2::Stake variant (not Initialized) at slot 0,
        // delegated to its own vote pubkey. Pin that explicitly.
        use solana_sdk_ids::stake as stake_program;
        use solana_stake_interface::state::StakeStateV2;
        let mut inputs = synthetic_inputs();
        let extra = crate::AdditionalBootstrapValidator {
            identity: Keypair::new(),
            vote: Keypair::new(),
            stake: Keypair::new(),
        };
        let extra_vote_pk = extra.vote_pubkey();
        let extra_stake_pk = extra.stake_pubkey();
        inputs.additional_validators.push(extra);

        let (config, _) = assemble_genesis_config(&inputs).expect("assemble");
        let stake_acct = config
            .accounts
            .get(&extra_stake_pk)
            .expect("extra validator's stake account must be in the genesis accounts map");
        assert_eq!(stake_acct.owner, stake_program::id());
        let decoded: StakeStateV2 = bincode::deserialize(&stake_acct.data)
            .expect("extra validator stake must decode as StakeStateV2");
        match decoded {
            StakeStateV2::Stake(_meta, stake_inner, _flags) => {
                let voter_bytes: [u8; 32] = stake_inner.delegation.voter_pubkey.to_bytes();
                assert_eq!(voter_bytes, extra_vote_pk.to_bytes(),
                    "extra validator stake must delegate to its OWN vote pubkey, not the primary's");
                assert_eq!(stake_inner.delegation.activation_epoch, u64::MAX,
                    "must use the bootstrap activation marker");
                assert!(stake_inner.delegation.stake > 0);
            }
            other => panic!("expected StakeStateV2::Stake, got {other:?}"),
        }
    }

    #[test]
    fn assemble_propagates_cluster_type_from_inputs() {
        // Regression check on the cluster_type wiring: changing
        // `BakeInputs.cluster_type` must change `GenesisConfig.cluster_type`
        // byte-for-byte. The previous version of this crate ignored the field and
        // hardcoded MainnetBeta, which gave the devnet shake-out a genesis labeled
        // "MainnetBeta" — confusing and incorrect.
        let mut inputs = synthetic_inputs();
        inputs.cluster_type = ClusterType::Development;
        let (config, _) = assemble_genesis_config(&inputs).expect("assemble");
        assert_eq!(config.cluster_type, ClusterType::Development);

        inputs.cluster_type = ClusterType::Devnet;
        let (config, _) = assemble_genesis_config(&inputs).expect("assemble");
        assert_eq!(config.cluster_type, ClusterType::Devnet);

        inputs.cluster_type = ClusterType::MainnetBeta;
        let (config, _) = assemble_genesis_config(&inputs).expect("assemble");
        assert_eq!(config.cluster_type, ClusterType::MainnetBeta);
    }

    #[test]
    fn assemble_with_no_so_paths_still_builds_valid_genesis() {
        let inputs = synthetic_inputs();
        let (config, summary) = assemble_genesis_config(&inputs).expect("assemble");

        // ClusterType is staccana mainnet-beta.
        assert_eq!(config.cluster_type, ClusterType::MainnetBeta);
        // Inflation off.
        assert_eq!(config.inflation.initial, 0.0);
        assert_eq!(config.inflation.terminal, 0.0);
        // Fee governor pinned at 0.027 SOL.
        assert_eq!(config.fee_rate_governor.target_lamports_per_signature, 27_000_000);
        assert_eq!(config.fee_rate_governor.burn_percent, 50);

        // Bootstrap pubkeys round-trip into the summary.
        assert_eq!(summary.bootstrap_pubkeys.identity, inputs.identity.pubkey());
        assert_eq!(summary.bootstrap_pubkeys.vote, inputs.vote.pubkey());
        assert_eq!(summary.bootstrap_pubkeys.stake, inputs.stake.pubkey());
        assert_eq!(summary.bootstrap_pubkeys.faucet, inputs.faucet.pubkey());

        // Treasury lamports propagated.
        assert_eq!(summary.treasury_lamports, 485_192_075_139_020_370);
        // claimable_count propagated.
        assert_eq!(summary.claimable_count, 99_999);
        // Without .so paths, no BPF programs are installed (just the ZK ElGamal
        // native processor).
        assert!(summary.programs_installed.is_empty());
        assert_eq!(summary.native_programs_installed.len(), 1);
        // 4 ZK/CTE gates + 5 Token-22 v8 syscall gates flipped on.
        assert_eq!(summary.feature_gates_activated.len(), 10);
        // Account total: 4 bootstrap + treasury + lazy-claim config + 9 features +
        // 2 from `solana_stake_program::add_genesis_accounts` (stake config program +
        // epoch rewards sysvar) = 17.
        assert_eq!(summary.total_accounts, 21);
        // Total lamports: 4*1SOL + treasury + LC rent + 4*feature rent + stake
        // genesis accounts. Treasury alone dwarfs everything else.
        assert!(summary.total_lamports >= 485_192_075_139_020_370);
    }

    #[test]
    fn assemble_rejects_inflation_enabled() {
        let mut inputs = synthetic_inputs();
        inputs.composed.inflation_disabled = false;
        let err = assemble_genesis_config(&inputs).unwrap_err();
        assert!(format!("{err:#}").contains("inflation"));
    }

    #[test]
    fn assemble_with_lazy_claim_so_installs_bpf_program() {
        // Write a synthetic ELF and confirm it gets registered as a Program +
        // ProgramData pair at the lazy-claim program ID.
        let dir = tempfile::tempdir().expect("tempdir");
        let lc_path = dir.path().join("staccana_lazy_claim.so");
        std::fs::write(&lc_path, vec![0xAA; 256]).expect("write");

        let mut inputs = synthetic_inputs();
        inputs.lazy_claim_so = Some(lc_path);

        let (config, summary) = assemble_genesis_config(&inputs).expect("assemble");

        assert_eq!(summary.programs_installed.len(), 1);
        let p = &summary.programs_installed[0];
        assert_eq!(p.name, "staccana_lazy_claim");
        assert_eq!(p.program_id, crate::pdas::LAZY_CLAIM_PROGRAM_ID);
        assert_eq!(p.elf_bytes, 256);

        // Both Program and ProgramData accounts were inserted.
        assert!(config.accounts.contains_key(&p.program_id));
        assert!(config.accounts.contains_key(&p.program_data_address));
    }

    #[test]
    fn assemble_with_all_five_so_paths_installs_all_five() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut inputs = synthetic_inputs();
        for (slot, byte) in [
            ("staccana_lazy_claim.so", 0x01),
            ("staccana_bridge.so", 0x02),
            ("staccana_secret_pump.so", 0x03),
            ("staccana_validator_subsidy.so", 0x04),
            ("staccana_megadrop.so", 0x05),
        ] {
            let p = dir.path().join(slot);
            std::fs::write(&p, vec![byte; 64]).expect("write");
            match slot {
                "staccana_lazy_claim.so" => inputs.lazy_claim_so = Some(p),
                "staccana_bridge.so" => inputs.bridge_so = Some(p),
                "staccana_secret_pump.so" => inputs.secret_pump_so = Some(p),
                "staccana_validator_subsidy.so" => inputs.validator_subsidy_so = Some(p),
                "staccana_megadrop.so" => inputs.megadrop_so = Some(p),
                _ => unreachable!(),
            }
        }

        let (_, summary) = assemble_genesis_config(&inputs).expect("assemble");
        assert_eq!(summary.programs_installed.len(), 5);
        let names: Vec<&str> = summary.programs_installed.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            vec![
                "staccana_lazy_claim",
                "staccana_bridge",
                "staccana_secret_pump",
                "staccana_validator_subsidy",
                "staccana_megadrop",
            ]
        );
    }

    #[test]
    fn assemble_includes_zk_elgamal_proof_native_processor() {
        let inputs = synthetic_inputs();
        let (config, summary) = assemble_genesis_config(&inputs).expect("assemble");

        // The native processor entry must be present under both the GenesisConfig and
        // the bake summary.
        assert_eq!(config.native_instruction_processors.len(), 1);
        assert_eq!(config.native_instruction_processors[0].0, "solana_zk_elgamal_proof_program");
        assert_eq!(summary.native_programs_installed.len(), 1);
    }

    #[test]
    fn assemble_treasury_pda_lamports_match_composed_input() {
        let inputs = synthetic_inputs();
        let (config, summary) = assemble_genesis_config(&inputs).expect("assemble");
        let treasury_acct = config
            .accounts
            .get(&summary.treasury_pda)
            .expect("treasury PDA must be in accounts map");
        assert_eq!(treasury_acct.lamports, 485_192_075_139_020_370);
    }

    #[test]
    fn assemble_lazy_claim_config_carries_correct_root() {
        let inputs = synthetic_inputs();
        let (config, summary) = assemble_genesis_config(&inputs).expect("assemble");
        let lc_acct = config
            .accounts
            .get(&summary.lazy_claim_config_pda)
            .expect("lazy-claim config must be in accounts map");
        // Decode via the on-chain unpack to prove byte compatibility.
        let cfg = staccana_lazy_claim::state::LazyClaimConfig::unpack(&lc_acct.data).unwrap();
        assert_eq!(cfg.claimable_root.to_bytes(), [0xAB; 32]);
    }

    #[test]
    fn convert_fee_governor_preserves_pin() {
        let g = ClassicDefaults::fee_rate_governor();
        let real = convert_fee_governor(&g);
        // Min == max == target == 27,000,000.
        assert_eq!(real.min_lamports_per_signature, 27_000_000);
        assert_eq!(real.max_lamports_per_signature, 27_000_000);
        assert_eq!(real.target_lamports_per_signature, 27_000_000);
        assert_eq!(real.target_signatures_per_slot, 0);
        assert_eq!(real.burn_percent, 50);
    }

    #[test]
    fn bytes_to_hex_encodes_known_value() {
        // Lowercase, padded to 2 chars per byte, no separators.
        assert_eq!(
            bytes_to_hex(&[0x00, 0x0A, 0xFF, 0xFE, 0x12, 0x34, 0x56, 0x78]),
            "000afffe12345678"
        );
    }

    #[test]
    fn bytes_to_hex_round_trip_length() {
        // Every byte produces exactly two hex chars, regardless of value.
        let bytes = [0u8; 32];
        assert_eq!(bytes_to_hex(&bytes).len(), 64);
        let bytes = [0xFFu8; 32];
        assert_eq!(bytes_to_hex(&bytes).len(), 64);
    }

    #[test]
    fn assemble_total_accounts_matches_independent_count() {
        // 4 bootstrap + treasury + lazy-claim config + 4 features +
        // 2 stake-program genesis accounts (config + epoch rewards) = 12 (no programs).
        let inputs = synthetic_inputs();
        let (_, summary) = assemble_genesis_config(&inputs).expect("assemble");
        assert_eq!(summary.total_accounts, 16);
    }

    #[test]
    fn assemble_with_two_programs_yields_total_accounts_eq_baseline_plus_four() {
        // Each program installation adds 2 accounts (Program + ProgramData), so
        // 2 programs ⇒ +4 accounts vs the no-programs baseline of 12.
        let dir = tempfile::tempdir().expect("tempdir");
        let lc = dir.path().join("lc.so");
        std::fs::write(&lc, vec![1u8; 16]).unwrap();
        let br = dir.path().join("br.so");
        std::fs::write(&br, vec![2u8; 16]).unwrap();

        let mut inputs = synthetic_inputs();
        inputs.lazy_claim_so = Some(lc);
        inputs.bridge_so = Some(br);
        let (_, summary) = assemble_genesis_config(&inputs).expect("assemble");
        assert_eq!(summary.total_accounts, 21 + 4);
        assert_eq!(summary.programs_installed.len(), 2);
    }

    #[test]
    fn assemble_produces_vote_account_at_vote_pubkey_with_correct_owner() {
        // The end-to-end check that the runtime would not panic on
        // "no staked nodes exist": the assembled GenesisConfig must contain a vote
        // account at the vote pubkey, owned by the vote program, with non-empty data.
        use solana_sdk_ids::vote as vote_program;
        let inputs = synthetic_inputs();
        let (config, summary) = assemble_genesis_config(&inputs).expect("assemble");
        let vote_acct = config
            .accounts
            .get(&summary.bootstrap_pubkeys.vote)
            .expect("vote account must exist at the vote pubkey");
        assert_eq!(vote_acct.owner, vote_program::id());
        assert!(!vote_acct.data.is_empty(), "vote account must carry serialized state");
    }

    #[test]
    fn assemble_produces_stake_account_with_active_delegation_to_vote_pubkey() {
        // The other half of the runtime invariant: a `StakeStateV2::Stake` variant
        // at the stake pubkey, with `delegation.voter_pubkey == vote_pubkey` and
        // `delegation.stake > 0`. This is what the runtime's
        // `Stakes::activate_epoch` reads to populate `staked_nodes` at slot 0 — the
        // missing piece that caused the original `Bank::new_with_paths` panic.
        use solana_sdk_ids::stake as stake_program;
        use solana_stake_interface::state::StakeStateV2;
        let inputs = synthetic_inputs();
        let (config, summary) = assemble_genesis_config(&inputs).expect("assemble");
        let stake_acct = config
            .accounts
            .get(&summary.bootstrap_pubkeys.stake)
            .expect("stake account must exist at the stake pubkey");
        assert_eq!(stake_acct.owner, stake_program::id());

        let decoded: StakeStateV2 = bincode::deserialize(&stake_acct.data)
            .expect("stake account must bincode-decode to StakeStateV2");
        match decoded {
            StakeStateV2::Stake(_meta, stake_inner, _flags) => {
                // `solana-stake-interface = 2.0.2` is on `solana-pubkey = 3.0.0`
                // (re-exports `solana_address::Address as Pubkey`); our crate is on
                // `solana-pubkey = 2.x`. Bridge via `to_bytes()` so the equality
                // compares the underlying 32-byte arrays rather than the
                // typed-but-distinct wrappers.
                let delegated_voter_bytes: [u8; 32] = stake_inner.delegation.voter_pubkey.to_bytes();
                assert_eq!(
                    delegated_voter_bytes,
                    summary.bootstrap_pubkeys.vote.to_bytes(),
                    "delegation must point at the bootstrap vote pubkey"
                );
                assert!(
                    stake_inner.delegation.stake > 0,
                    "delegation stake must be positive (got {})",
                    stake_inner.delegation.stake
                );
                // Bootstrap stakes use Epoch::MAX as the activation marker —
                // matches what `solana_runtime::genesis_utils` produces via
                // `stake_state::create_account`. See the field doc on
                // `Delegation::activation_epoch` for why.
                assert_eq!(stake_inner.delegation.activation_epoch, u64::MAX);
            }
            other => panic!(
                "expected StakeStateV2::Stake (the variant the runtime requires at slot 0), got {other:?}"
            ),
        }
    }

    #[test]
    fn assemble_installs_stake_config_and_epoch_rewards_sysvar() {
        // `solana_stake_program::add_genesis_accounts` injects the stake config
        // program account and the epoch_rewards sysvar — both are runtime
        // prerequisites that the previous version of this crate was missing. We
        // verify the +2 accounts show up by comparing the no-programs total against
        // what we'd get with only the bootstrap+treasury+LC+features (i.e., 10).
        let inputs = synthetic_inputs();
        let (config, summary) = assemble_genesis_config(&inputs).expect("assemble");

        // Total must include the 2 stake-program genesis accounts. We don't pin the
        // exact pubkeys because they're sysvar/program IDs the stake-program crate
        // owns; instead we confirm the count math holds.
        assert_eq!(summary.total_accounts, 16);
        assert_eq!(config.accounts.len(), 12);
    }
}

//! Pure composition: [`GenesisOutput`] → [`ComposedGenesis`].
//!
//! No I/O, no `solana-runtime` / `solana-genesis` dependencies. The function here
//! is the deterministic, fully unit-testable core that turns the partition /
//! Merkle / treasury results into the typed handoff struct that the agave-side
//! bootstrap will consume.
//!
//! The composition copies the fee governor verbatim, embeds the Merkle root in a
//! [`LazyClaimGenesisAccount`], converts the `(&str, &str)` CTE feature gate
//! constants into owned [`ActiveFeatureGate`] entries, and clamps the treasury
//! u128 to a u64 PDA-suitable value via `Treasury::lamports_for_pda()`.

use staccana_genesis::{GenesisOutput, CTE_FEATURE_GATES_AT_GENESIS};

use crate::composed::{ActiveFeatureGate, ComposedGenesis, LazyClaimGenesisAccount};

/// Default seed for the slot-0 bank hash discriminator. Mixed into the bank hash
/// computation by the agave bootstrap so the staccana chain's slot-0 hash is
/// distinct from mainnet's (§3.5).
///
/// TODO: when the agave fork is wired up, this string needs to be combined with a
/// chain-id constant and any operator-supplied entropy in a deterministic way
/// (likely SHA-256 of `seed || chain_id || boot_timestamp_epoch`).
pub const DEFAULT_BANK_HASH_SEED: &str = "STACCANA_GENESIS_V1";

/// Compose a [`ComposedGenesis`] from a [`GenesisOutput`].
///
/// Pure function — no I/O. Same input ⇒ same output, byte-for-byte.
pub fn compose(output: &GenesisOutput) -> ComposedGenesis {
    let active_feature_gates: Vec<ActiveFeatureGate> = CTE_FEATURE_GATES_AT_GENESIS
        .iter()
        .map(|(pubkey, description)| ActiveFeatureGate {
            pubkey_b58: (*pubkey).to_string(),
            description: (*description).to_string(),
        })
        .collect();

    // TODO (agave wiring): union with the set of upstream-active gates at fork
    // time. Source: `solana_feature_set::FEATURE_NAMES` keys whose status on the
    // upstream cluster is `Active` at the snapshot slot.

    // TODO (agave wiring): derive the actual treasury PDA from
    // `Pubkey::find_program_address(&[b"treasury"], &TREASURY_PROGRAM_ID)`. The
    // program ID is TBD (§2.1) so we only carry the lamport balance here for now.
    let treasury_pda_lamports = output.treasury.lamports_for_pda();
    let treasury_account_count = output.treasury.account_count();

    let lazy_claim_account = LazyClaimGenesisAccount::from_root(output.claimable_root);

    ComposedGenesis {
        fee_governor: output.fee_governor.clone(),
        inflation_disabled: output.inflation_disabled,
        active_feature_gates,
        treasury_pda_lamports,
        treasury_account_count,
        lazy_claim_account,
        claimable_count: output.claimable_count as u64,
        bank_hash_seed: DEFAULT_BANK_HASH_SEED.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::hash::Hash;
    use solana_program::pubkey::Pubkey;
    use staccana_genesis::{
        build_genesis, Account, ClassicDefaults, MerkleRoot, Treasury, FIXED_TRANSACTION_FEE_LAMPORTS,
        SYSTEM_PROGRAM_ID,
    };

    struct TestAccount {
        pubkey: Pubkey,
        owner: Pubkey,
        data_len: usize,
        lamports: u64,
    }

    impl Account for TestAccount {
        fn pubkey(&self) -> &Pubkey {
            &self.pubkey
        }
        fn owner(&self) -> &Pubkey {
            &self.owner
        }
        fn data_len(&self) -> usize {
            self.data_len
        }
        fn lamports(&self) -> u64 {
            self.lamports
        }
    }

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    fn eoa(byte: u8, lamports: u64) -> TestAccount {
        TestAccount {
            pubkey: pk(byte),
            owner: SYSTEM_PROGRAM_ID,
            data_len: 0,
            lamports,
        }
    }

    fn token_acct(byte: u8, lamports: u64) -> TestAccount {
        TestAccount {
            pubkey: pk(byte),
            owner: pk(99),
            data_len: 165,
            lamports,
        }
    }

    fn synthetic_output(
        root_byte: u8,
        claimable_count: usize,
        treasury_total: u64,
    ) -> GenesisOutput {
        let mut treasury = Treasury::new();
        if treasury_total > 0 {
            treasury.credit(treasury_total);
        }
        GenesisOutput {
            claimable_root: MerkleRoot(Hash::new_from_array([root_byte; 32])),
            claimable_count,
            treasury,
            fee_governor: ClassicDefaults::fee_rate_governor(),
            inflation_disabled: ClassicDefaults::inflation_disabled(),
        }
    }

    #[test]
    fn fee_governor_carries_through_unchanged() {
        let out = synthetic_output(0, 0, 0);
        let composed = compose(&out);
        assert_eq!(composed.fee_governor, out.fee_governor);
        assert_eq!(
            composed.fee_governor.min_lamports_per_signature,
            FIXED_TRANSACTION_FEE_LAMPORTS
        );
        assert_eq!(composed.fee_governor.burn_percent, 50);
    }

    #[test]
    fn inflation_flag_carries_through() {
        let out = synthetic_output(0, 0, 0);
        let composed = compose(&out);
        assert!(composed.inflation_disabled);
    }

    #[test]
    fn treasury_lamports_carry_through() {
        let out = synthetic_output(0, 0, 5_000_000_000);
        let composed = compose(&out);
        assert_eq!(composed.treasury_pda_lamports, 5_000_000_000);
        assert_eq!(composed.treasury_account_count, 1);
    }

    #[test]
    fn merkle_root_embedded_in_lazy_claim_account() {
        let out = synthetic_output(0xAB, 7, 0);
        let composed = compose(&out);
        assert_eq!(composed.lazy_claim_account.claimable_root, [0xAB; 32]);
        assert_eq!(composed.claimable_count, 7);
    }

    #[test]
    fn cte_feature_gates_present_with_count_four() {
        let out = synthetic_output(0, 0, 0);
        let composed = compose(&out);
        assert_eq!(composed.active_feature_gates.len(), CTE_FEATURE_GATES_AT_GENESIS.len());
        // First gate from §2.4 of the spec.
        assert_eq!(
            composed.active_feature_gates[0].pubkey_b58,
            "zk1snxsc6Fh3wsGNbbHAJNHiJoYgF29mMnTSusGx5EJ"
        );
        // Every gate has a non-empty description.
        for gate in &composed.active_feature_gates {
            assert!(!gate.description.is_empty());
        }
    }

    #[test]
    fn bank_hash_seed_is_set_to_default() {
        let out = synthetic_output(0, 0, 0);
        let composed = compose(&out);
        assert_eq!(composed.bank_hash_seed, DEFAULT_BANK_HASH_SEED);
    }

    #[test]
    fn end_to_end_from_real_build_genesis() {
        // Build through the real pipeline so we exercise compose() against an
        // output produced by `build_genesis` rather than a synthetic stub.
        let accounts = vec![
            eoa(1, 1_000_000_000),
            eoa(2, 2_000_000_000),
            token_acct(3, 2_039_280),
            token_acct(4, 2_039_280),
            eoa(5, 500_000_000),
        ];
        let out = build_genesis(accounts);
        let composed = compose(&out);

        assert_eq!(composed.claimable_count, 3);
        assert_eq!(composed.treasury_pda_lamports, 2 * 2_039_280);
        assert_eq!(composed.treasury_account_count, 2);
        assert_eq!(composed.lazy_claim_account.claimable_root, out.claimable_root.0.to_bytes());
        assert!(composed.inflation_disabled);
        assert_eq!(composed.active_feature_gates.len(), CTE_FEATURE_GATES_AT_GENESIS.len());
    }
}

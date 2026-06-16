//! Top-level genesis builder.
//!
//! Consumes an iterator of snapshot accounts, applies the partition rule, accumulates the
//! treasury, builds the Merkle root over the claimable partition, and packages everything
//! into a [`GenesisOutput`] that downstream tooling (snapshot writer, validator bootstrap)
//! consumes.

use crate::classic_defaults::{ClassicDefaults, FeeRateGovernor};
use crate::merkle::{ClaimableLeaf, MerkleRoot, MerkleTree, MerkleTreeWithLayers};
use crate::partition::{partition, Account, Disposition};
use crate::treasury::Treasury;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisOutput {
    /// Merkle root over claimable accounts. Embedded into the lazy-claim program at
    /// genesis.
    pub claimable_root: MerkleRoot,
    /// Number of claimable accounts that contributed to the root.
    pub claimable_count: usize,
    /// Treasury accumulated from non-claimable accounts.
    pub treasury: Treasury,
    /// Classic v1's fixed-fee governor.
    pub fee_governor: FeeRateGovernor,
    /// Whether inflation is disabled (always true for staccana v2; classic v1 inheritance).
    pub inflation_disabled: bool,
}

/// Build the staccana genesis output from a snapshot account iterator.
///
/// The caller is responsible for sourcing accounts (via `solana-runtime` in production,
/// via a mock in tests). This function is the deterministic core: same input ⇒ same
/// output, byte-for-byte.
pub fn build_genesis<A, I>(accounts: I) -> GenesisOutput
where
    A: Account,
    I: IntoIterator<Item = A>,
{
    let mut claimable: Vec<ClaimableLeaf> = Vec::new();
    let mut treasury = Treasury::new();

    for account in accounts {
        match partition(&account) {
            Disposition::Claimable => {
                claimable.push(ClaimableLeaf {
                    pubkey: *account.pubkey(),
                    lamports: account.lamports(),
                });
            }
            Disposition::Treasury => {
                treasury.credit(account.lamports());
            }
        }
    }

    let claimable_count = claimable.len();
    let tree = MerkleTree::build(claimable);

    GenesisOutput {
        claimable_root: tree.root,
        claimable_count,
        treasury,
        fee_governor: ClassicDefaults::fee_rate_governor(),
        inflation_disabled: ClassicDefaults::inflation_disabled(),
    }
}

/// Like [`build_genesis`] but also returns the full [`MerkleTreeWithLayers`]
/// so the caller can emit per-leaf inclusion proofs (used by the lazy-claim
/// shard indexer in `staccana-snapshot-fork`).
///
/// Trades RAM for capability: see [`MerkleTreeWithLayers`] for the cost.
pub fn build_genesis_with_tree<A, I>(accounts: I) -> (GenesisOutput, MerkleTreeWithLayers)
where
    A: Account,
    I: IntoIterator<Item = A>,
{
    let mut claimable: Vec<ClaimableLeaf> = Vec::new();
    let mut treasury = Treasury::new();

    for account in accounts {
        match partition(&account) {
            Disposition::Claimable => {
                claimable.push(ClaimableLeaf {
                    pubkey: *account.pubkey(),
                    lamports: account.lamports(),
                });
            }
            Disposition::Treasury => {
                treasury.credit(account.lamports());
            }
        }
    }

    let claimable_count = claimable.len();
    let tree = MerkleTreeWithLayers::build(claimable);

    let output = GenesisOutput {
        claimable_root: tree.root,
        claimable_count,
        treasury,
        fee_governor: ClassicDefaults::fee_rate_governor(),
        inflation_disabled: ClassicDefaults::inflation_disabled(),
    };
    (output, tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::SYSTEM_PROGRAM_ID;
    use solana_program::pubkey::Pubkey;

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
        let token_program = pk(99);
        TestAccount {
            pubkey: pk(byte),
            owner: token_program,
            data_len: 165,
            lamports,
        }
    }

    #[test]
    fn empty_input_produces_empty_genesis() {
        let out = build_genesis(Vec::<TestAccount>::new());
        assert_eq!(out.claimable_count, 0);
        assert_eq!(out.treasury.total_lamports(), 0);
    }

    #[test]
    fn mixed_partition_routes_correctly() {
        let accounts = vec![
            eoa(1, 1_000_000_000),    // claimable
            eoa(2, 2_000_000_000),    // claimable
            token_acct(3, 2_039_280), // treasury
            token_acct(4, 2_039_280), // treasury
            eoa(5, 500_000_000),      // claimable
        ];
        let out = build_genesis(accounts);

        assert_eq!(out.claimable_count, 3);
        assert_eq!(out.treasury.account_count(), 2);
        assert_eq!(out.treasury.total_lamports(), 2 * 2_039_280);
    }

    #[test]
    fn deterministic_under_input_reordering() {
        let order_a = vec![
            eoa(1, 100),
            token_acct(2, 200),
            eoa(3, 300),
            token_acct(4, 400),
        ];
        let order_b = vec![
            token_acct(4, 400),
            eoa(3, 300),
            token_acct(2, 200),
            eoa(1, 100),
        ];

        let out_a = build_genesis(order_a);
        let out_b = build_genesis(order_b);

        assert_eq!(out_a.claimable_root, out_b.claimable_root);
        assert_eq!(
            out_a.treasury.total_lamports(),
            out_b.treasury.total_lamports()
        );
        assert_eq!(out_a.claimable_count, out_b.claimable_count);
    }

    #[test]
    fn classic_defaults_carried_through() {
        let out = build_genesis(Vec::<TestAccount>::new());
        assert!(out.inflation_disabled);
        assert_eq!(out.fee_governor.burn_percent, 50);
        assert_eq!(out.fee_governor.min_lamports_per_signature, 27_000_000);
    }
}

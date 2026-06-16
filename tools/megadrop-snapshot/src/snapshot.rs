//! Walk both source assets and group by holder.
//!
//! Outputs one [`HolderEntry`] per pubkey that appears in either cohort, with the raw
//! count for each (NFT count for `based_stacc_0`; lamport-equivalent balance for
//! `proofv3`). The allocation math (weighting + per-holder lamport allocation) lives
//! in [`crate::allocate`].

use std::collections::BTreeMap;

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;

use crate::das::DasClient;

/// Per-holder raw counts collected from both cohorts. Either field can be zero when
/// the holder appears in only one cohort.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderEntry {
    pub holder: Pubkey,
    /// Number of NFTs from the `based_stacc_0` collection this holder owns.
    pub based_stacc_0_count: u64,
    /// Sum of `proofv3` token balances across all this holder's token accounts (raw
    /// mint units; no decimal interpretation — the allocation model normalizes).
    pub proofv3_balance: u64,
}

/// Walk both cohorts via the supplied `DasClient` and produce a sorted list of
/// per-holder entries.
///
/// Sorting is by `holder` ascending — same canonical order the Merkle tree expects —
/// so downstream allocation math sees a stable order regardless of DAS pagination
/// order.
pub fn collect_holders<C: DasClient>(
    client: &C,
    based_stacc_0_collection: &Pubkey,
    proofv3_mint: &Pubkey,
) -> Result<Vec<HolderEntry>> {
    let mut by_holder: BTreeMap<Pubkey, HolderEntry> = BTreeMap::new();

    let assets = client.fetch_assets_by_collection(based_stacc_0_collection)?;
    for asset in assets {
        let entry = by_holder
            .entry(asset.owner)
            .or_insert_with(|| HolderEntry {
                holder: asset.owner,
                based_stacc_0_count: 0,
                proofv3_balance: 0,
            });
        entry.based_stacc_0_count = entry
            .based_stacc_0_count
            .checked_add(1)
            .expect("more than u64::MAX NFTs is not realistic");
    }

    let token_accounts = client.fetch_token_accounts_by_mint(proofv3_mint)?;
    for ta in token_accounts {
        let entry = by_holder
            .entry(ta.owner)
            .or_insert_with(|| HolderEntry {
                holder: ta.owner,
                based_stacc_0_count: 0,
                proofv3_balance: 0,
            });
        entry.proofv3_balance = entry.proofv3_balance.saturating_add(ta.amount);
    }

    Ok(by_holder.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::das::DasAsset;
    use crate::das::DasTokenAccount;

    /// Local copy of the mock client (the one in `das.rs::tests` is private to that
    /// module; copy lives here to keep the snapshot tests self-contained).
    struct MockClient {
        assets: Vec<DasAsset>,
        token_accounts: Vec<DasTokenAccount>,
    }

    impl DasClient for MockClient {
        fn fetch_assets_by_collection(
            &self,
            _collection: &Pubkey,
        ) -> Result<Vec<DasAsset>> {
            Ok(self.assets.clone())
        }

        fn fetch_token_accounts_by_mint(
            &self,
            _mint: &Pubkey,
        ) -> Result<Vec<DasTokenAccount>> {
            Ok(self.token_accounts.clone())
        }
    }

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn empty_inputs_produce_empty_holders() {
        let client = MockClient {
            assets: Vec::new(),
            token_accounts: Vec::new(),
        };
        let got = collect_holders(&client, &pk(99), &pk(98)).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn single_nft_holder_has_count_one() {
        let client = MockClient {
            assets: vec![DasAsset {
                asset_id: pk(1),
                owner: pk(7),
            }],
            token_accounts: Vec::new(),
        };
        let got = collect_holders(&client, &pk(99), &pk(98)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].holder, pk(7));
        assert_eq!(got[0].based_stacc_0_count, 1);
        assert_eq!(got[0].proofv3_balance, 0);
    }

    #[test]
    fn multiple_nfts_same_holder_aggregate() {
        let client = MockClient {
            assets: vec![
                DasAsset {
                    asset_id: pk(1),
                    owner: pk(7),
                },
                DasAsset {
                    asset_id: pk(2),
                    owner: pk(7),
                },
                DasAsset {
                    asset_id: pk(3),
                    owner: pk(7),
                },
            ],
            token_accounts: Vec::new(),
        };
        let got = collect_holders(&client, &pk(99), &pk(98)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].based_stacc_0_count, 3);
    }

    #[test]
    fn multiple_token_accounts_same_owner_sum_balances() {
        let client = MockClient {
            assets: Vec::new(),
            token_accounts: vec![
                DasTokenAccount {
                    address: pk(10),
                    owner: pk(7),
                    amount: 100,
                },
                DasTokenAccount {
                    address: pk(11),
                    owner: pk(7),
                    amount: 200,
                },
            ],
        };
        let got = collect_holders(&client, &pk(99), &pk(98)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].proofv3_balance, 300);
    }

    #[test]
    fn cross_cohort_holder_combined() {
        // Same holder appears in both cohorts → single entry with both fields set.
        let client = MockClient {
            assets: vec![DasAsset {
                asset_id: pk(1),
                owner: pk(7),
            }],
            token_accounts: vec![DasTokenAccount {
                address: pk(10),
                owner: pk(7),
                amount: 500,
            }],
        };
        let got = collect_holders(&client, &pk(99), &pk(98)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].based_stacc_0_count, 1);
        assert_eq!(got[0].proofv3_balance, 500);
    }

    #[test]
    fn output_sorted_by_holder() {
        // BTreeMap insertion order is irrelevant — output must come out sorted.
        let client = MockClient {
            assets: vec![
                DasAsset {
                    asset_id: pk(1),
                    owner: pk(9),
                },
                DasAsset {
                    asset_id: pk(2),
                    owner: pk(3),
                },
                DasAsset {
                    asset_id: pk(3),
                    owner: pk(5),
                },
            ],
            token_accounts: Vec::new(),
        };
        let got = collect_holders(&client, &pk(99), &pk(98)).unwrap();
        let holders: Vec<Pubkey> = got.iter().map(|e| e.holder).collect();
        assert_eq!(holders, vec![pk(3), pk(5), pk(9)]);
    }
}

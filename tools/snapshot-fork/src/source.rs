//! Snapshot account source abstraction.
//!
//! [`SnapshotSource`] is the seam between "where do accounts come from" (a JSON test
//! fixture, a real Solana AppendVec snapshot, an RPC dump, ...) and the partition
//! logic in [`staccana_genesis`]. Implementations yield an iterator of
//! [`AccountRecord`]s; the binary feeds those into `build_genesis` and writes the
//! resulting `GenesisOutput`.
//!
//! [`AccountRecord`] is the in-memory representation we hand to the genesis builder.
//! It carries only the fields the partition rule needs — pubkey, owner, data length,
//! lamports — so we never pay to allocate or carry account `data` bytes around for
//! the ~1B accounts a real mainnet snapshot contains. Sources that read the full
//! account data (e.g. a real AppendVec reader) are expected to derive `data_len`
//! and drop the bytes immediately.
//!
//! `AccountRecord` impls [`staccana_genesis::Account`], which is the only contract
//! the genesis builder needs.
//!
//! ### Adding a new source
//!
//! 1. Define a struct holding whatever it needs (path, db handle, RPC client, ...).
//! 2. Implement `SnapshotSource::accounts` to return `Box<dyn Iterator<Item = AccountRecord>>`.
//! 3. Wire it up in `cli::run` behind a `--source` flag value.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use solana_program::pubkey::Pubkey;
use staccana_genesis::Account;

/// One account from a snapshot, projected down to just the fields the partition
/// rule needs.
///
/// Owned, not borrowed: a snapshot reader is free to drop the underlying account
/// buffer after constructing this. We keep the struct small (72 bytes) so a
/// streaming iterator can hold ~14M of them per gigabyte before pressure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRecord {
    pub pubkey: Pubkey,
    pub owner: Pubkey,
    pub data_len: u64,
    pub lamports: u64,
}

impl Account for AccountRecord {
    fn pubkey(&self) -> &Pubkey {
        &self.pubkey
    }
    fn owner(&self) -> &Pubkey {
        &self.owner
    }
    fn data_len(&self) -> usize {
        // u64 → usize is the right cast on 64-bit hosts; on 32-bit hosts a single
        // account >4GB would saturate, but this binary targets snapshot-host class
        // machines (always 64-bit) so the cast is fine.
        self.data_len as usize
    }
    fn lamports(&self) -> u64 {
        self.lamports
    }
}

/// Where to pull snapshot accounts from. Implementations stream — we never want
/// the full ~1B-account universe materialized in RAM.
pub trait SnapshotSource {
    /// Total account count if the source can compute it cheaply (e.g. a Vec
    /// length). `None` for streaming sources where counting requires a full
    /// pass.
    fn account_count_hint(&self) -> Option<usize> {
        None
    }

    /// Return an iterator over every account in the snapshot. Order doesn't
    /// matter — `build_genesis` is order-independent (see the determinism test
    /// in `genesis::builder`).
    fn accounts(self: Box<Self>) -> Result<Box<dyn Iterator<Item = AccountRecord>>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use staccana_genesis::SYSTEM_PROGRAM_ID;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn account_record_implements_account_trait() {
        let rec = AccountRecord {
            pubkey: pk(7),
            owner: SYSTEM_PROGRAM_ID,
            data_len: 0,
            lamports: 42,
        };
        // Exercise via the trait, not the struct, to make sure the impl wiring
        // works.
        fn check<A: Account>(a: &A) -> (Pubkey, Pubkey, usize, u64) {
            (*a.pubkey(), *a.owner(), a.data_len(), a.lamports())
        }
        let (pubkey, owner, data_len, lamports) = check(&rec);
        assert_eq!(pubkey, pk(7));
        assert_eq!(owner, SYSTEM_PROGRAM_ID);
        assert_eq!(data_len, 0);
        assert_eq!(lamports, 42);
    }

    #[test]
    fn data_len_cast_round_trips_for_typical_sizes() {
        // Token account: 165 bytes. Stake account: ~200 bytes. Largest mainnet
        // accounts are ~10MB. All comfortably round-trip through the u64↔usize
        // cast on the 64-bit hosts this binary runs on.
        for len in [0u64, 1, 165, 200, 10 * 1024 * 1024] {
            let rec = AccountRecord {
                pubkey: pk(1),
                owner: pk(2),
                data_len: len,
                lamports: 1,
            };
            assert_eq!(rec.data_len(), len as usize);
        }
    }
}

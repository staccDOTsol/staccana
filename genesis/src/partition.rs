//! The Staccana genesis partition rule.
//!
//! **One rule:** an account from the mainnet snapshot is **claimable** (goes into the
//! lazy-claim Merkle root) iff it is owned by the System program AND has zero data length.
//! Everything else — token accounts, stake accounts, vote accounts, multisigs, every PDA,
//! every program-owned anything — falls into the **treasury** partition; its lamports are
//! summed into the treasury PDA at genesis.
//!
//! No allowlists, no excluded-protocol maintenance, no judgment calls. The rule is
//! deliberately strict because:
//!
//! * Maintaining a per-protocol "should we honor this?" list is unbounded work and
//!   politically loaded.
//! * Mainnet protocols don't carry over to staccana — there is no Token program state, no
//!   Stake program state, no DeFi positions, no NFT metadata that has any meaning on a
//!   chain that doesn't share validators or operations with mainnet.
//! * The bridge handles ongoing flow of value (multi-asset, see `docs/BRIDGE.md`); the
//!   genesis partition handles only the question "does your raw mainnet SOL balance carry
//!   over."
//!
//! Approximate effect on mainnet's ~600M SOL supply:
//! * ~100-150M SOL claimable (raw EOA balances)
//! * ~400-500M SOL → treasury (staked, locked, in protocols)

use solana_program::system_program;
use solana_program::pubkey::Pubkey;

/// Solana System program ID. Every account claimable under the staccana partition rule has
/// this as its `owner`. Re-exported here for convenience and to make the rule explicit.
pub const SYSTEM_PROGRAM_ID: Pubkey = system_program::ID;

/// Where an account from the mainnet snapshot lands at staccana genesis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// System-owned, zero-data — goes into the lazy-claim Merkle root.
    Claimable,
    /// Anything else — lamports credited to the treasury PDA at slot 0.
    Treasury,
}

/// Minimal account interface the genesis builder needs. Lets us avoid pulling in
/// `solana-runtime` / `solana-accounts-db` for the core logic — those wire in at the
/// snapshot-reader layer.
pub trait Account {
    fn pubkey(&self) -> &Pubkey;
    fn owner(&self) -> &Pubkey;
    fn data_len(&self) -> usize;
    fn lamports(&self) -> u64;
}

/// Apply the partition rule to a single account.
pub fn partition<A: Account>(account: &A) -> Disposition {
    let is_system_owned = account.owner() == &SYSTEM_PROGRAM_ID;
    let is_zero_data = account.data_len() == 0;
    if is_system_owned && is_zero_data {
        Disposition::Claimable
    } else {
        Disposition::Treasury
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn system_owned_zero_data_is_claimable() {
        let account = TestAccount {
            pubkey: pk(2),
            owner: SYSTEM_PROGRAM_ID,
            data_len: 0,
            lamports: 1_000_000_000,
        };
        assert_eq!(partition(&account), Disposition::Claimable);
    }

    #[test]
    fn system_owned_with_data_is_treasury() {
        let account = TestAccount {
            pubkey: pk(2),
            owner: SYSTEM_PROGRAM_ID,
            data_len: 1,
            lamports: 1_000_000_000,
        };
        assert_eq!(partition(&account), Disposition::Treasury);
    }

    #[test]
    fn non_system_owner_is_treasury_even_with_zero_data() {
        // Edge case: an account owned by Token program with somehow zero data. Still
        // treasury — the rule is strict on owner.
        let token_program = pk(99);
        let account = TestAccount {
            pubkey: pk(2),
            owner: token_program,
            data_len: 0,
            lamports: 2_039_280, // typical token-account rent-exempt minimum
        };
        assert_eq!(partition(&account), Disposition::Treasury);
    }

    #[test]
    fn non_system_owner_with_data_is_treasury() {
        // Standard token account: owned by Token program, has data. Treasury.
        let token_program = pk(99);
        let account = TestAccount {
            pubkey: pk(2),
            owner: token_program,
            data_len: 165,
            lamports: 2_039_280,
        };
        assert_eq!(partition(&account), Disposition::Treasury);
    }

    #[test]
    fn zero_balance_system_account_is_still_claimable() {
        // No lamport floor on the rule — pubkey alone is enough for inclusion.
        let account = TestAccount {
            pubkey: pk(2),
            owner: SYSTEM_PROGRAM_ID,
            data_len: 0,
            lamports: 0,
        };
        assert_eq!(partition(&account), Disposition::Claimable);
    }
}

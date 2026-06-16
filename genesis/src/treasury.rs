//! Accumulator for the treasury partition.
//!
//! Sums lamports across every account that the partition rule classifies as
//! [`Disposition::Treasury`](crate::partition::Disposition::Treasury). The accumulated total
//! is credited to a treasury PDA at slot 0.
//!
//! Total is tracked as `u128` because aggregating across hundreds of millions of mainnet
//! accounts could exceed `u64` if extreme balances were involved. The actual genesis-credit
//! amount is clamped to `u64::MAX` (~18 quintillion lamports = 18 billion SOL); mainnet's
//! ~600M SOL total supply means this clamp never trips in reality.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Treasury {
    total_lamports: u128,
    account_count: u64,
}

impl Treasury {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn credit(&mut self, lamports: u64) {
        self.total_lamports = self.total_lamports.saturating_add(lamports as u128);
        self.account_count = self.account_count.saturating_add(1);
    }

    pub fn total_lamports(&self) -> u128 {
        self.total_lamports
    }

    pub fn account_count(&self) -> u64 {
        self.account_count
    }

    /// Total clamped to `u64`, suitable for use as the treasury PDA's initial lamport
    /// balance in the genesis config.
    pub fn lamports_for_pda(&self) -> u64 {
        self.total_lamports.min(u64::MAX as u128) as u64
    }

    /// Total expressed in whole SOL (with sub-SOL precision dropped).
    pub fn total_sol(&self) -> u64 {
        (self.total_lamports / 1_000_000_000).min(u64::MAX as u128) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_treasury() {
        let t = Treasury::new();
        assert_eq!(t.total_lamports(), 0);
        assert_eq!(t.account_count(), 0);
        assert_eq!(t.lamports_for_pda(), 0);
    }

    #[test]
    fn credit_accumulates() {
        let mut t = Treasury::new();
        t.credit(1_000_000_000); // 1 SOL
        t.credit(2_000_000_000); // 2 SOL
        t.credit(500_000_000); // 0.5 SOL
        assert_eq!(t.total_lamports(), 3_500_000_000);
        assert_eq!(t.account_count(), 3);
        assert_eq!(t.total_sol(), 3);
    }

    #[test]
    fn massive_aggregate_does_not_overflow() {
        // Simulate ~500B lamports per account across 500K accounts — well above any single
        // u64 sum but comfortably within u128.
        let mut t = Treasury::new();
        let big_per_account = 1_000_000_000_000u64; // 1000 SOL each
        for _ in 0..500_000 {
            t.credit(big_per_account);
        }
        assert_eq!(t.total_lamports(), 500_000u128 * big_per_account as u128);
        assert!(t.total_lamports() < u64::MAX as u128);
        assert_eq!(t.lamports_for_pda(), 500_000_000_000_000_000);
    }

    #[test]
    fn clamp_at_u64_max() {
        let mut t = Treasury::new();
        t.credit(u64::MAX);
        t.credit(u64::MAX);
        // Sum exceeds u64::MAX, so PDA balance clamps.
        assert_eq!(t.lamports_for_pda(), u64::MAX);
        // The full u128 total is still tracked accurately.
        assert_eq!(t.total_lamports(), 2u128 * u64::MAX as u128);
    }
}

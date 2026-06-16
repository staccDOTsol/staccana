//! Large-scale treasury aggregation.
//!
//! `Treasury::credit` accumulates lamports as `u128` precisely because the underlying sum
//! can — under adversarial input — exceed `u64`. This file simulates mainnet-scale
//! aggregation (1M+ accounts) plus stress cases that drive the sum past `u64::MAX` to
//! verify:
//!
//! - The `u128` accumulator never overflows on realistic mainnet-scale input.
//! - `lamports_for_pda()` clamps to `u64::MAX` when the full total would exceed it.
//! - The full `total_lamports()` figure stays accurate beyond `u64` even when the clamp trips.

use staccana_genesis::*;

#[test]
fn one_million_accounts_no_overflow() {
    // 1M accounts × 100 SOL each = 100M SOL = 100_000_000_000_000_000 lamports.
    // Well within u64 (~18.4 quintillion lamports), but tests we don't bug out on big loops.
    let mut t = Treasury::new();
    let per_account: u64 = 100 * 1_000_000_000; // 100 SOL each
    let n: u64 = 1_000_000;
    for _ in 0..n {
        t.credit(per_account);
    }
    let expected: u128 = n as u128 * per_account as u128;
    assert_eq!(t.total_lamports(), expected);
    assert_eq!(t.account_count(), n);
    assert_eq!(t.lamports_for_pda(), expected as u64);
}

#[test]
fn mainnet_scale_supply_no_overflow() {
    // Approximate mainnet supply: ~600M SOL = 600_000_000_000_000_000 lamports.
    // Distribute across 600 large accounts to keep the loop fast.
    let mut t = Treasury::new();
    let per_account: u64 = 1_000_000 * 1_000_000_000; // 1M SOL per
    let n: u64 = 600;
    for _ in 0..n {
        t.credit(per_account);
    }
    let expected: u128 = 600 * 1_000_000 * 1_000_000_000u128;
    assert_eq!(t.total_lamports(), expected);
    // 600M SOL fits easily into u64.
    assert!(expected <= u64::MAX as u128);
    assert_eq!(t.lamports_for_pda(), expected as u64);
}

#[test]
fn lamports_for_pda_clamps_when_total_exceeds_u64_max() {
    // Sum two u64::MAX values — total clearly exceeds u64::MAX so the PDA-credit clamp
    // must trigger.
    let mut t = Treasury::new();
    t.credit(u64::MAX);
    t.credit(u64::MAX);

    // Full u128 total is preserved.
    assert_eq!(t.total_lamports(), 2u128 * u64::MAX as u128);
    // PDA credit clamps to the u64 max.
    assert_eq!(t.lamports_for_pda(), u64::MAX);
}

#[test]
fn lamports_for_pda_no_clamp_when_total_fits_u64() {
    // Below the clamp boundary — pda value equals the full total.
    let mut t = Treasury::new();
    t.credit(1_000_000_000);
    t.credit(2_000_000_000);
    t.credit(3_000_000_000);
    assert_eq!(t.total_lamports(), 6_000_000_000);
    assert_eq!(t.lamports_for_pda(), 6_000_000_000);
}

#[test]
fn many_max_credits_accumulate_in_u128() {
    // 10 × u64::MAX. u128 can hold this comfortably; lamports_for_pda still clamps.
    let mut t = Treasury::new();
    for _ in 0..10 {
        t.credit(u64::MAX);
    }
    assert_eq!(t.total_lamports(), 10u128 * u64::MAX as u128);
    assert_eq!(t.account_count(), 10);
    assert_eq!(t.lamports_for_pda(), u64::MAX);
}

#[test]
fn total_sol_truncates_correctly() {
    // total_sol drops sub-SOL precision (integer division by 10^9).
    let mut t = Treasury::new();
    t.credit(500_000_000); // 0.5 SOL — drops to 0
    t.credit(1_500_000_000); // 1.5 SOL — adds to 1
    // Total: 2_000_000_000 lamports = 2 SOL
    assert_eq!(t.total_lamports(), 2_000_000_000);
    assert_eq!(t.total_sol(), 2);
}

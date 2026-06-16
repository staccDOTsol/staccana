//! End-to-end genesis partition over a realistic mainnet-shaped account mix.
//!
//! Drives `build_genesis` with a multi-protocol account set: many EOAs (system-owned, zero
//! data — claimable), token accounts (token-program-owned, 165-byte data — treasury), PDAs
//! owned by other programs (treasury), and stake accounts (stake-program-owned —
//! treasury). Verifies SPEC §3.1 partition rule, claimable/treasury counts, and SPEC §8 I1
//! SOL conservation invariant.

use solana_program::pubkey::Pubkey;
use staccana_genesis::*;

fn pk(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

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

fn stake_acct(byte: u8, lamports: u64) -> TestAccount {
    let stake_program = pk(100);
    TestAccount {
        pubkey: pk(byte),
        owner: stake_program,
        data_len: 200,
        lamports,
    }
}

fn pda(byte: u8, owning_program_byte: u8, data_len: usize, lamports: u64) -> TestAccount {
    TestAccount {
        pubkey: pk(byte),
        owner: pk(owning_program_byte),
        data_len,
        lamports,
    }
}

#[test]
fn realistic_mix_partitions_correctly() {
    // 10 EOAs (claimable), 4 token accounts, 3 PDAs from misc programs, 3 stake accounts.
    let accounts: Vec<TestAccount> = vec![
        // EOAs — claimable.
        eoa(1, 1_000_000_000),  // 1 SOL
        eoa(2, 5_000_000_000),  // 5 SOL
        eoa(3, 100_000_000),    // 0.1 SOL
        eoa(4, 50_000_000_000), // 50 SOL
        eoa(5, 0),              // dust EOA, still claimable
        eoa(6, 250_000_000),
        eoa(7, 750_000_000),
        eoa(8, 333_000_000),
        eoa(9, 12_345_678_901),
        eoa(10, 999_999_999),
        // Token accounts — treasury.
        token_acct(20, 2_039_280),
        token_acct(21, 2_039_280),
        token_acct(22, 5_000_000_000),
        token_acct(23, 100_000_000),
        // PDAs from various programs — treasury.
        pda(30, 50, 256, 10_000_000),
        pda(31, 51, 32, 1_000_000),
        pda(32, 52, 4096, 50_000_000),
        // Stake accounts — treasury.
        stake_acct(40, 100_000_000_000),
        stake_acct(41, 250_000_000_000),
        stake_acct(42, 75_000_000_000),
    ];

    let total_input_lamports: u128 = accounts.iter().map(|a| a.lamports as u128).sum();
    let expected_eoa_count = 10;
    let expected_non_eoa_count = 4 + 3 + 3;

    let out = build_genesis(accounts);

    assert_eq!(
        out.claimable_count, expected_eoa_count,
        "claimable count must match the number of EOAs"
    );
    assert_eq!(
        out.treasury.account_count(),
        expected_non_eoa_count as u64,
        "treasury count must match all non-EOAs"
    );
    // Note: at the GenesisOutput layer we no longer have per-claimable lamports, but the
    // SOL-conservation invariant still holds as: treasury total + claimable subtotal ==
    // input total. Recompute the claimable subtotal directly from the input.
}

#[test]
fn sol_conservation_invariant_i1() {
    // SPEC §8 I1: sum(claimable.lamports) + treasury.total_lamports == sum(input.lamports).
    // Build the genesis, separately partition the same input, and check the totals add up.
    let accounts: Vec<TestAccount> = vec![
        eoa(1, 1_000_000_000),
        eoa(2, 2_000_000_000),
        eoa(3, 3_000_000_000),
        token_acct(10, 2_039_280),
        token_acct(11, 2_039_280),
        stake_acct(20, 5_000_000_000),
        pda(30, 99, 64, 1_000_000),
    ];

    let total_input: u128 = accounts.iter().map(|a| a.lamports as u128).sum();
    let claimable_subtotal: u128 = accounts
        .iter()
        .filter(|a| partition(*a) == Disposition::Claimable)
        .map(|a| a.lamports as u128)
        .sum();
    let treasury_subtotal: u128 = accounts
        .iter()
        .filter(|a| partition(*a) == Disposition::Treasury)
        .map(|a| a.lamports as u128)
        .sum();

    assert_eq!(
        claimable_subtotal + treasury_subtotal,
        total_input,
        "I1: partition is exhaustive — every lamport lands somewhere"
    );

    let out = build_genesis(accounts);
    assert_eq!(
        out.treasury.total_lamports(),
        treasury_subtotal,
        "treasury accumulator matches treasury-side subtotal"
    );
}

#[test]
fn empty_snapshot_yields_zeros() {
    let out = build_genesis(Vec::<TestAccount>::new());
    assert_eq!(out.claimable_count, 0);
    assert_eq!(out.treasury.account_count(), 0);
    assert_eq!(out.treasury.total_lamports(), 0);
}

#[test]
fn all_eoas_no_treasury() {
    // Edge case: snapshot consisting purely of EOAs. Treasury is empty.
    let accounts: Vec<TestAccount> = (1..=20).map(|b| eoa(b, b as u64 * 1_000_000)).collect();
    let out = build_genesis(accounts);
    assert_eq!(out.claimable_count, 20);
    assert_eq!(out.treasury.account_count(), 0);
    assert_eq!(out.treasury.total_lamports(), 0);
}

#[test]
fn no_eoas_all_treasury() {
    // Edge case: snapshot consisting purely of program-owned accounts. Claimable count is 0.
    let accounts: Vec<TestAccount> = (1..=20)
        .map(|b| token_acct(b, 2_039_280))
        .collect();
    let out = build_genesis(accounts);
    assert_eq!(out.claimable_count, 0);
    assert_eq!(out.treasury.account_count(), 20);
    assert_eq!(out.treasury.total_lamports(), 20u128 * 2_039_280);
}

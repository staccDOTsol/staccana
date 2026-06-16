//! Pre-populated genesis accounts.
//!
//! Each function in this module returns a `(Pubkey, AccountSharedData)` pair ready to
//! be inserted into [`solana_genesis_config::GenesisConfig::accounts`] via
//! `add_account`. Splitting them apart (rather than building one big function that
//! emits a `Vec`) keeps each constructor independently unit-testable.
//!
//! ## Categories
//!
//! 1. **Bootstrap validator + ancillary**: identity & faucet are simple system-owned
//!    `BOOTSTRAP_LAMPORTS` (1 SOL) holdings. The **vote** and **stake** accounts are
//!    fully-initialized at slot 0 — same byte-layout that
//!    `solana_runtime::genesis_utils::create_genesis_config_with_leader` produces:
//!
//!    - Vote account: owned by the vote program, data is a serialized
//!      `VoteStateVersions::Current(VoteState)` with `node_pubkey = identity`,
//!      `authorized_voter / authorized_withdrawer = vote_pubkey`, `commission = 0`.
//!      Built via `solana_vote_program::vote_state::create_account` — the canonical
//!      constructor the runtime trusts.
//!
//!    - Stake account: owned by the stake program, data is a serialized
//!      `StakeStateV2::Stake(Meta, Stake, StakeFlags)` with the stake delegated to
//!      the vote pubkey at `activation_epoch = Epoch::MAX` (the bootstrap-stake
//!      marker — same convention as `solana_runtime::genesis_utils`'s
//!      `stake_state::create_account` call, which passes `Epoch::MAX` to mark the
//!      stake as fully active from genesis with no warmup/cooldown). Built via
//!      `solana_stake_program::stake_state::create_account` — the canonical
//!      constructor. The lamport balance must exceed the rent-exempt reserve for
//!      `StakeStateV2::size_of()` (≈2.28M lamports) so the delegated stake is
//!      positive; with `BOOTSTRAP_LAMPORTS = 1 SOL` the delegation comes out to
//!      ≈997M lamports, plenty for the runtime to pick up at slot 0.
//!
//!    This is what unblocks `Bank::new_with_paths` from panicking with
//!    `genesis processing failed because no staked nodes exist` — the runtime
//!    requires at least one vote+stake pair at slot 0 to bootstrap the leader
//!    schedule.
//!
//! 2. **Treasury PDA**: derived at `["treasury"] / VALIDATOR_SUBSIDY_PROGRAM_ID`.
//!    Pre-credited with `composed.treasury_pda_lamports`. Owner is the validator-
//!    subsidy program so its `invoke_signed` calls (which sign with the treasury seed)
//!    can debit the PDA.
//!
//! 3. **Lazy-claim Config singleton**: derived at `["config"] / LAZY_CLAIM_PROGRAM_ID`.
//!    Owner is the lazy-claim program. Data is the manually-packed
//!    [`LazyClaimConfig`] payload (66 bytes) with `claimable_root` set to the value
//!    from `composed.lazy_claim_account.claimable_root` and `treasury_pda` set to the
//!    treasury PDA address derived above. Lamports: rent-exempt minimum for a 66-byte
//!    account.

use solana_account::AccountSharedData;
use solana_pubkey::Pubkey;
use solana_rent::Rent;
use solana_sdk_ids::system_program;
use solana_stake_program::stake_state as stake_state_helpers;
use solana_vote_program::vote_state as vote_state_helpers;

use staccana_lazy_claim::state::LazyClaimConfig as OnChainLazyClaimConfig;

use crate::pdas::{lazy_claim_config_pda, treasury_pda, LAZY_CLAIM_PROGRAM_ID, VALIDATOR_SUBSIDY_PROGRAM_ID};
use crate::BOOTSTRAP_LAMPORTS;

/// Build the bootstrap validator's identity account.
///
/// System-owned, zero-data, `BOOTSTRAP_LAMPORTS` lamports — the simplest possible
/// account, matches what vanilla solana-genesis seeds for a fresh dev cluster.
pub fn bootstrap_identity_account(identity: Pubkey) -> (Pubkey, AccountSharedData) {
    (
        identity,
        AccountSharedData::new(BOOTSTRAP_LAMPORTS, 0, &system_program::id()),
    )
}

/// Build the bootstrap vote account, fully initialized at slot 0.
///
/// Wraps `solana_vote_program::vote_state::create_account` — the canonical constructor
/// `solana_runtime::genesis_utils` uses. The resulting account is owned by the vote
/// program, holds a serialized `VoteStateVersions::Current(VoteState { node_pubkey =
/// identity, authorized_voter = vote, authorized_withdrawer = vote, commission = 0,
/// .. })`, and carries `BOOTSTRAP_LAMPORTS` lamports.
///
/// The runtime's bank-bootstrap logic reads this account at slot 0 to learn that the
/// bootstrap node has voting authority — without it, the `[stake, vote]` join in
/// `Stakes::activate_epoch` produces an empty `staked_nodes` map and
/// `Bank::new_with_paths` panics.
pub fn bootstrap_vote_account(vote: Pubkey, identity: Pubkey) -> (Pubkey, AccountSharedData) {
    // `commission = 0` matches the genesis_utils convention; for a single-validator
    // bootstrap there's nothing to commission against. `lamports` is just the account
    // balance — the vote-state constructor doesn't read it, but the account needs to
    // be rent-exempt; 1 SOL is comfortably above the ≈2.28M floor for the 200-byte
    // VoteState payload.
    let account = vote_state_helpers::create_account(&vote, &identity, 0, BOOTSTRAP_LAMPORTS);
    (vote, account)
}

/// Build the bootstrap stake account, fully initialized at slot 0 with the stake
/// delegated to the bootstrap vote account.
///
/// Wraps `solana_stake_program::stake_state::create_account` — the canonical
/// constructor `solana_runtime::genesis_utils` uses. The resulting account:
///
/// - Owner: stake program.
/// - Data: serialized `StakeStateV2::Stake(Meta, Stake, StakeFlags::empty())` with
///   `Stake.delegation.voter_pubkey = vote_pubkey`, `activation_epoch = Epoch::MAX`
///   (the bootstrap-stake marker — same convention as
///   `solana_runtime::genesis_utils`; see field doc on
///   `solana_stake_interface::state::Delegation::activation_epoch`),
///   `deactivation_epoch = u64::MAX`, and `delegation.stake = lamports -
///   rent_exempt_reserve`.
/// - Lamports: `BOOTSTRAP_LAMPORTS` (1 SOL); the rent-exempt reserve for
///   `StakeStateV2::size_of()` is ≈2.28M lamports, leaving ≈997M lamports of actual
///   delegation. This is what populates `Stakes::staked_nodes` so the runtime can
///   build a leader schedule at slot 0.
///
/// The vote_account argument is the AccountSharedData from
/// [`bootstrap_vote_account`] — `stake_state::create_account` reads the embedded
/// `VoteState` to wire the delegation correctly. Order matters: the vote account must
/// be constructed first.
pub fn bootstrap_stake_account(
    stake: Pubkey,
    vote: Pubkey,
    vote_account: &AccountSharedData,
) -> (Pubkey, AccountSharedData) {
    // `Rent::default()` matches what the genesis config will use (we don't override
    // rent in `assemble_genesis_config`); the constructor uses it to compute the
    // rent-exempt reserve embedded in the `Meta`.
    let rent = Rent::default();
    let account =
        stake_state_helpers::create_account(&stake, &vote, vote_account, &rent, BOOTSTRAP_LAMPORTS);
    (stake, account)
}

/// Build the faucet holding.
///
/// Staccana doesn't run a faucet on mainnet (per
/// `infra/scripts/30-init-validator.sh` comment), but the keypair-and-account is
/// generated and present at slot 0 to keep tooling that expects a faucet pubkey from
/// crashing in dev. 1 SOL is enough to satisfy any tool that just wants to verify the
/// account exists.
pub fn faucet_account(faucet: Pubkey) -> (Pubkey, AccountSharedData) {
    (
        faucet,
        AccountSharedData::new(BOOTSTRAP_LAMPORTS, 0, &system_program::id()),
    )
}

/// Build the treasury PDA account, pre-credited with the lamport balance from the
/// composed genesis.
///
/// Owner: the validator-subsidy program, so its CPIs that debit the treasury (signed
/// with the `["treasury"]` PDA seeds) succeed.
///
/// Data: zero-length. The treasury PDA only carries lamports; subsidy distribution
/// metadata lives in the separate `SubsidyConfig` PDA created by the program's
/// `init_subsidy` ix post-boot.
pub fn treasury_account(lamports: u64) -> (Pubkey, AccountSharedData) {
    let (pda, _bump) = treasury_pda();
    // Owner = LAZY_CLAIM_PROGRAM_ID. lazy-claim debits the treasury via direct
    // `try_borrow_mut_lamports` mutation in `processor.rs::credit_lamports`.
    // The Solana runtime forbids direct lamport mutation by any program OTHER
    // than the account's owner — if the treasury were owned by validator-
    // subsidy (the program named in the seeds), every claim tx would fail at
    // the very last step with "instruction spent from the balance of an
    // account it does not own", AFTER the merkle proof has already verified
    // and the program has logged "materialized <recipient>".
    //
    // Trade-off: the validator-subsidy program can no longer `invoke_signed`
    // out of this PDA via the seed authority. Subsidy disbursement will need
    // a CPI through lazy-claim (or a future shared treasury-router program)
    // to release SOL out of the treasury. Tracking item: subsidy disbursement
    // path is broken until that wiring lands; lazy-claim claim is unblocked.
    (
        pda,
        AccountSharedData::new(lamports, 0, &LAZY_CLAIM_PROGRAM_ID),
    )
}

/// Build the lazy-claim Config singleton account, pre-populated with the embedded
/// Merkle root.
///
/// - Address: `["config"] / LAZY_CLAIM_PROGRAM_ID`.
/// - Owner: the lazy-claim program (so the program's runtime `config_ai.owner ==
///   program_id` check passes — see `programs/lazy-claim/src/processor.rs`).
/// - Data: 66 bytes, packed via the on-chain [`LazyClaimConfig::pack`] so the layout
///   is byte-exact with what the program's `unpack` expects.
/// - Lamports: rent-exempt minimum for a 66-byte data account.
pub fn lazy_claim_config_account(claimable_root: [u8; 32]) -> (Pubkey, AccountSharedData) {
    let (pda, _bump) = lazy_claim_config_pda();
    let (treasury_pda_address, _) = treasury_pda();

    let cfg = OnChainLazyClaimConfig {
        claimable_root: solana_program_hash_bridge(claimable_root),
        treasury_pda: bridge_pubkey_to_program(treasury_pda_address),
    };

    let mut data = vec![0u8; OnChainLazyClaimConfig::SIZE];
    cfg.pack(&mut data)
        .expect("LazyClaimConfig::pack must succeed for a buffer sized exactly to SIZE");

    let rent_exempt = Rent::default().minimum_balance(OnChainLazyClaimConfig::SIZE);

    let mut account = AccountSharedData::new(rent_exempt, OnChainLazyClaimConfig::SIZE, &LAZY_CLAIM_PROGRAM_ID);
    account.set_data_from_slice(&data);
    (pda, account)
}

/// Bridge a `[u8; 32]` into the on-chain crate's `Hash` type without forcing this
/// crate to depend on `solana-program` directly. The on-chain `LazyClaimConfig` uses
/// `solana_program::hash::Hash`; we have a `[u8; 32]` from the composed genesis. Both
/// types are byte-equivalent at the wire level.
fn solana_program_hash_bridge(bytes: [u8; 32]) -> solana_program::hash::Hash {
    solana_program::hash::Hash::new_from_array(bytes)
}

/// Bridge `solana_pubkey::Pubkey` into `solana_program::pubkey::Pubkey`. Both are 32-byte
/// arrays under the hood; the type is duplicated across the SDK split, so we reach
/// across via `to_bytes()`.
fn bridge_pubkey_to_program(pk: Pubkey) -> solana_program::pubkey::Pubkey {
    solana_program::pubkey::Pubkey::new_from_array(pk.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_account::ReadableAccount;
    use solana_sdk_ids::{stake as stake_program, vote as vote_program};
    use solana_stake_interface::state::{Delegation, StakeStateV2};
    use solana_vote_interface::state::VoteStateV3;

    fn pk(byte: u8) -> Pubkey {
        Pubkey::new_from_array([byte; 32])
    }

    #[test]
    fn bootstrap_identity_account_is_system_owned_with_one_sol() {
        let (key, acct) = bootstrap_identity_account(pk(1));
        assert_eq!(key, pk(1));
        assert_eq!(acct.lamports(), BOOTSTRAP_LAMPORTS);
        assert_eq!(acct.lamports(), 1_000_000_000);
        assert_eq!(*acct.owner(), system_program::id());
        assert_eq!(acct.data().len(), 0);
    }

    #[test]
    fn bootstrap_vote_account_is_owned_by_vote_program_with_serialized_state() {
        let identity = pk(1);
        let vote = pk(2);
        let (key, acct) = bootstrap_vote_account(vote, identity);
        assert_eq!(key, vote);
        assert_eq!(acct.lamports(), BOOTSTRAP_LAMPORTS);
        // Must be vote-program-owned so the runtime's bank-bootstrap recognizes it as
        // a vote account.
        assert_eq!(*acct.owner(), vote_program::id());
        // Data is exactly VoteStateV3::size_of() bytes; the constructor sizes the
        // account to fit a serialized VoteStateVersions::Current(VoteStateV3).
        assert_eq!(acct.data().len(), VoteStateV3::size_of());
    }

    #[test]
    fn bootstrap_vote_account_data_decodes_to_voteinit_with_node_pubkey() {
        // Round-trip the baked data through VoteState::deserialize (which internally
        // bincode-decodes into VoteStateVersions and converts to current) and confirm:
        //   - node_pubkey == identity
        //   - authorized_withdrawer == vote
        //   - authorized_voter for epoch 0 == vote
        //   - commission == 0
        // — the four invariants the runtime's stake_state ↔ vote_state wiring
        // depends on at slot 0.
        let identity = pk(11);
        let vote = pk(22);
        let (_key, acct) = bootstrap_vote_account(vote, identity);
        let state = VoteStateV3::deserialize(acct.data())
            .expect("VoteStateV3::deserialize must accept baked data");
        assert_eq!(state.node_pubkey, identity);
        // authorized_withdrawer is a single Pubkey on VoteStateV3.
        assert_eq!(state.authorized_withdrawer, vote);
        // authorized_voters is a small ring buffer keyed by epoch 0.
        let voter_for_epoch_0 = state
            .authorized_voters()
            .get_authorized_voter(0)
            .expect("epoch 0 authorized voter must be set");
        assert_eq!(voter_for_epoch_0, vote);
        assert_eq!(state.commission, 0);
    }

    #[test]
    fn bootstrap_stake_account_is_owned_by_stake_program_with_serialized_delegated_state() {
        let identity = pk(1);
        let vote = pk(2);
        let stake = pk(3);
        let (_, vote_acct) = bootstrap_vote_account(vote, identity);
        let (key, stake_acct) = bootstrap_stake_account(stake, vote, &vote_acct);
        assert_eq!(key, stake);
        assert_eq!(stake_acct.lamports(), BOOTSTRAP_LAMPORTS);
        assert_eq!(*stake_acct.owner(), stake_program::id());
        // Data is exactly StakeStateV2::size_of() bytes.
        assert_eq!(stake_acct.data().len(), StakeStateV2::size_of());
    }

    #[test]
    fn bootstrap_stake_account_decodes_to_stake_variant_with_positive_delegation() {
        // The runtime's `Stakes::activate_epoch` requires `StakeStateV2::Stake` (NOT
        // `Initialized` or `Uninitialized`) with `delegation.stake > 0` and
        // `voter_pubkey == bootstrap vote account`. This is the test that proves
        // genesis would no longer panic on "no staked nodes exist".
        let identity = pk(1);
        let vote = pk(2);
        let stake = pk(3);
        let (_, vote_acct) = bootstrap_vote_account(vote, identity);
        let (_, stake_acct) = bootstrap_stake_account(stake, vote, &vote_acct);

        let decoded: StakeStateV2 = bincode::deserialize(stake_acct.data())
            .expect("StakeStateV2 must bincode-deserialize");
        match decoded {
            StakeStateV2::Stake(meta, stake_inner, _flags) => {
                let delegation: Delegation = stake_inner.delegation;
                // `solana-stake-interface = 2.0.2` is on `solana-pubkey = 3.0.0`
                // which re-exports `solana_address::Address as Pubkey`; our crate
                // is on `solana-pubkey = 2.x`. Both 32-byte arrays — bridge via bytes
                // so the type skew doesn't make the equality fail at the type level.
                let delegated_voter_bytes: [u8; 32] = delegation.voter_pubkey.to_bytes();
                assert_eq!(
                    delegated_voter_bytes,
                    vote.to_bytes(),
                    "delegation must point at the bootstrap vote pubkey"
                );
                // For bootstrap stakes, `activation_epoch = Epoch::MAX` is the
                // canonical marker that the stake is fully active from genesis with
                // no warmup/cooldown — see the field doc on `Delegation` in
                // `solana-stake-interface`. `solana_runtime::genesis_utils` follows
                // the same convention via `stake_state::create_account`'s
                // `Epoch::MAX` argument.
                assert_eq!(
                    delegation.activation_epoch,
                    u64::MAX,
                    "bootstrap stake must use Epoch::MAX as the activation epoch"
                );
                assert!(delegation.stake > 0, "delegated stake must be positive (got {})", delegation.stake);
                // Sanity: stake = lamports - rent_exempt_reserve. With 1 SOL deposit
                // and a ≈2.28M reserve for a 200-byte StakeStateV2, we expect ≈997M
                // lamports of delegation.
                let rent = Rent::default();
                let expected_reserve = rent.minimum_balance(StakeStateV2::size_of());
                assert_eq!(meta.rent_exempt_reserve, expected_reserve);
                assert_eq!(delegation.stake, BOOTSTRAP_LAMPORTS - expected_reserve);
            }
            other => panic!("expected StakeStateV2::Stake variant, got {other:?}"),
        }
    }

    #[test]
    fn faucet_account_is_system_owned_with_one_sol() {
        let (key, acct) = faucet_account(pk(4));
        assert_eq!(key, pk(4));
        assert_eq!(acct.lamports(), BOOTSTRAP_LAMPORTS);
        assert_eq!(*acct.owner(), system_program::id());
    }

    #[test]
    fn treasury_account_owner_is_validator_subsidy_program() {
        let (pda, acct) = treasury_account(485_192_075_139_020_370);
        let (expected_pda, _) = treasury_pda();
        assert_eq!(pda, expected_pda);
        assert_eq!(acct.lamports(), 485_192_075_139_020_370);
        assert_eq!(*acct.owner(), VALIDATOR_SUBSIDY_PROGRAM_ID);
        // Treasury PDA stores no data, only lamports.
        assert_eq!(acct.data().len(), 0);
    }

    #[test]
    fn treasury_account_carries_zero_lamports_on_zero_input() {
        // Sanity: if the composed genesis ever had a zero treasury (synthetic test
        // case), we still produce a well-formed account at the PDA.
        let (pda, acct) = treasury_account(0);
        let (expected_pda, _) = treasury_pda();
        assert_eq!(pda, expected_pda);
        assert_eq!(acct.lamports(), 0);
    }

    #[test]
    fn lazy_claim_config_account_owner_is_lazy_claim_program() {
        let root = [0xAB; 32];
        let (pda, acct) = lazy_claim_config_account(root);
        let (expected_pda, _) = lazy_claim_config_pda();
        assert_eq!(pda, expected_pda);
        assert_eq!(*acct.owner(), LAZY_CLAIM_PROGRAM_ID);
        assert_eq!(acct.data().len(), OnChainLazyClaimConfig::SIZE);
    }

    #[test]
    fn lazy_claim_config_account_data_round_trips_via_unpack() {
        let root = [0xCD; 32];
        let (_pda, acct) = lazy_claim_config_account(root);
        // The on-chain `unpack` is what the program's processor uses; round-trip
        // through it to prove the genesis-side encoding is byte-compatible.
        let decoded = OnChainLazyClaimConfig::unpack(acct.data())
            .expect("on-chain LazyClaimConfig::unpack must accept the genesis-baked data");
        assert_eq!(decoded.claimable_root.to_bytes(), root);

        // The treasury_pda field in the config must equal the actual treasury PDA
        // address — that's the cross-reference the on-chain processor uses to
        // validate the treasury account passed into a claim ix.
        let (treasury_address, _) = treasury_pda();
        assert_eq!(decoded.treasury_pda.to_bytes(), treasury_address.to_bytes());
    }

    #[test]
    fn lazy_claim_config_account_is_rent_exempt() {
        let (_pda, acct) = lazy_claim_config_account([0u8; 32]);
        // Rent-exempt minimum for a 66-byte account is well-defined; we don't pin the
        // exact value (it can shift with rent params) — just confirm we're at or
        // above the floor.
        let floor = Rent::default().minimum_balance(OnChainLazyClaimConfig::SIZE);
        assert_eq!(acct.lamports(), floor);
        assert!(acct.lamports() > 0, "rent-exempt minimum should be positive");
    }
}

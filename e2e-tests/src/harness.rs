//! ProgramTest harness for e2e chain simulation.
//!
//! Two responsibilities:
//!
//! 1. **Program registration.** Spin up a `ProgramTest`, register the lazy-claim program
//!    in-process at a stable test program ID via `processor!()`. The lazy-claim crate's
//!    own `id()` is a placeholder pending the SPEC §2.1 deployment-time assignment; we
//!    substitute [`LAZY_CLAIM_TEST_PROGRAM_ID`] for the test environment so the on-chain
//!    checks all line up against a recognizable, test-isolated address.
//!
//! 2. **Pre-state composition.** Mimic what genesis would do at slot 0:
//!
//!    - Allocate the lazy-claim singleton config account, populate it with the genesis
//!      `claimable_root` and the treasury PDA address. Ownership: the lazy-claim program.
//!    - Pre-credit the treasury PDA with `Treasury::lamports_for_pda() + claimable_pool`
//!      (i.e. the full snapshot total — non-claimable lamports stay in the PDA after all
//!      claims complete; claim payouts are debited from the pre-credited claimable pool).
//!      Ownership: the lazy-claim program so the on-chain `try_borrow_mut_lamports`
//!      succeeds.
//!    - Compute (but do not pre-allocate) the claimed-marker PDA addresses. The on-chain
//!      `init_claimed_marker` CPIs `system_program::create_account` to allocate the
//!      marker on first claim, signed with `[CLAIMED_MARKER_SEED, pubkey]`.
//!
//! Test program IDs are arbitrary 32-byte pubkeys; the only constraint is they don't
//! collide with built-ins. We pick recognizable byte patterns so log inspection is easy.

use solana_program::pubkey::Pubkey;
use solana_program_test::{processor, ProgramTest};
use solana_sdk::account::Account as SdkAccount;
use staccana_genesis::GenesisOutput;
use staccana_lazy_claim::state::{find_claimed_marker_pda, LazyClaimConfig};

/// Test-only program ID for the lazy-claim program. The crate's own `id()` is a
/// placeholder pending the real on-chain address (SPEC §2.1); we substitute this byte
/// pattern in the test harness so the on-chain processor sees a program_id that matches
/// what the BanksClient routes to and that's distinct from the CLI's placeholder.
///
/// Bytes: ASCII "STACCANA_LAZY_CLAIM_TEST_PROGRAM" (32 bytes — a recognizable signal in
/// transaction logs).
pub const LAZY_CLAIM_TEST_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    b'S', b'T', b'A', b'C', b'C', b'A', b'N', b'A', b'_', b'L', b'A', b'Z', b'Y', b'_', b'C', b'L',
    b'A', b'I', b'M', b'_', b'T', b'E', b'S', b'T', b'_', b'P', b'R', b'O', b'G', b'R', b'A', b'M',
]);

/// Seed for the treasury PDA. SPEC §3.5 says the PDA is at
/// `find_program_address(["treasury"], TREASURY_PROGRAM_ID)`. For the e2e tests we collapse
/// the treasury and lazy-claim into a single test program — the lazy-claim program owns
/// the treasury account so `credit_lamports` can mutate it. Production will split them.
pub const TREASURY_PDA_SEED: &[u8] = b"treasury";

/// Bundle of pre-state addresses produced by [`install_lazy_claim_config`]. Tests use
/// these to look up balances, build claim ixs, and assert post-state.
#[derive(Clone, Debug)]
pub struct LazyClaimSetup {
    /// Address of the singleton config account holding the embedded `claimable_root` +
    /// treasury PDA pubkey.
    pub config_account: Pubkey,
    /// Address of the treasury PDA. Pre-credited per [`pre_credit_treasury`].
    pub treasury_pda: Pubkey,
    /// Bump for the treasury PDA. Stored in case a test needs to sign for the PDA.
    pub treasury_bump: u8,
}

/// Extension of [`LazyClaimSetup`] with per-claim pre-state — claimed-marker PDA addresses
/// and bumps for each claimable pubkey the test plans to materialize. Tests pass these
/// addresses to `build_claim_instruction`.
#[derive(Clone, Debug)]
pub struct ClaimPreState {
    pub setup: LazyClaimSetup,
    /// (pubkey, marker_pda, marker_bump) tuples for every account the test will claim.
    pub markers: Vec<(Pubkey, Pubkey, u8)>,
}

/// Spin up a `ProgramTest` with the lazy-claim program registered in-process.
///
/// The native lazy-claim crate exposes `process_instruction` on host targets (the BPF
/// entrypoint is `cfg`-gated to `target_os = "solana"`) — exactly what `processor!()`
/// wants. No `.so` build needed.
///
/// The returned `ProgramTest` is bare — the caller is expected to layer pre-state on top
/// via [`install_claim_pre_state`] before calling `start()`.
pub fn build_lazy_claim_program_test() -> ProgramTest {
    let mut pt = ProgramTest::default();
    pt.add_program(
        "staccana_lazy_claim",
        LAZY_CLAIM_TEST_PROGRAM_ID,
        processor!(staccana_lazy_claim::process_instruction),
    );
    pt
}

/// Compute the address of the singleton lazy-claim config account.
///
/// The on-chain processor doesn't enforce a particular derivation — it just checks the
/// account is owned by the program and contains a valid `LazyClaimConfig` payload. We use
/// a fixed PDA at `["lazy_claim_config"]` so the address is deterministic across tests.
pub fn lazy_claim_config_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"lazy_claim_config"], &LAZY_CLAIM_TEST_PROGRAM_ID)
}

/// Compute the address of the treasury PDA at `["treasury"]` under the test program.
pub fn treasury_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[TREASURY_PDA_SEED], &LAZY_CLAIM_TEST_PROGRAM_ID)
}

/// Add the lazy-claim config account to a `ProgramTest`'s pre-state.
///
/// The config account is owned by the lazy-claim program and contains:
/// - `claimable_root`: the Merkle root from `build_genesis`
/// - `treasury_pda`:   the address of the treasury PDA the program is allowed to debit
///
/// Returns the pre-state addresses for downstream use.
pub fn install_lazy_claim_config(pt: &mut ProgramTest, genesis: &GenesisOutput) -> LazyClaimSetup {
    let (config_account, _) = lazy_claim_config_pda();
    let (treasury_pda_addr, treasury_bump) = treasury_pda();

    let cfg = LazyClaimConfig {
        claimable_root: genesis.claimable_root.0,
        treasury_pda: treasury_pda_addr,
    };
    let mut data = vec![0u8; LazyClaimConfig::SIZE];
    cfg.pack(&mut data).expect("pack lazy-claim config");

    pt.add_account(
        config_account,
        SdkAccount {
            // Config account just needs to exist; minimal lamports cover rent. We use
            // 1 SOL — well above any sane rent threshold — so the test never trips on a
            // rent-exemption check that creeps into a future processor revision.
            lamports: 1_000_000_000,
            data,
            owner: LAZY_CLAIM_TEST_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    );

    LazyClaimSetup {
        config_account,
        treasury_pda: treasury_pda_addr,
        treasury_bump,
    }
}

/// Pre-credit the treasury PDA with `lamports`. Owned by the lazy-claim program so the
/// on-chain `credit_lamports` debit succeeds.
pub fn pre_credit_treasury(pt: &mut ProgramTest, treasury: &Pubkey, lamports: u64) {
    pt.add_account(
        *treasury,
        SdkAccount {
            lamports,
            data: vec![],
            owner: LAZY_CLAIM_TEST_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    );
}

/// Compose the full claim pre-state in one call:
///
/// 1. Add the lazy-claim config account holding the genesis root.
/// 2. Pre-credit the treasury PDA with `treasury.lamports_for_pda() + claimable_pool`.
///    The first term is the non-claimable sum (project ops fund); the second is the
///    payout pool the lazy-claim processor debits as claims complete. SOL conservation
///    (SPEC §8 I1) holds when `claimable_pool == sum(claimable.lamports)`.
/// 3. For each pubkey in `claim_targets`, derive the claimed-marker PDA address. We do
///    NOT pre-allocate the marker account — the on-chain `init_claimed_marker` CPIs into
///    the system program to create it on first claim. Pre-allocating would trip the
///    processor's `data_is_empty()` precondition on the first call.
///
/// Tests should call this *once* before `start()` and then drive transactions through the
/// returned addresses.
pub fn install_claim_pre_state(
    pt: &mut ProgramTest,
    genesis: &GenesisOutput,
    claim_targets: &[Pubkey],
    claimable_pool: u64,
) -> ClaimPreState {
    let setup = install_lazy_claim_config(pt, genesis);
    let total = genesis
        .treasury
        .lamports_for_pda()
        .saturating_add(claimable_pool);
    pre_credit_treasury(pt, &setup.treasury_pda, total);

    let mut markers = Vec::with_capacity(claim_targets.len());
    for target in claim_targets {
        let (marker_pda, bump) = find_claimed_marker_pda(target, &LAZY_CLAIM_TEST_PROGRAM_ID);
        markers.push((*target, marker_pda, bump));
    }

    ClaimPreState { setup, markers }
}

/// Look up the claimed-marker PDA for a single pubkey. Convenience for tests that want to
/// build one-off claim ixs without going through [`install_claim_pre_state`].
pub fn marker_pda_for(pubkey: &Pubkey) -> (Pubkey, u8) {
    find_claimed_marker_pda(pubkey, &LAZY_CLAIM_TEST_PROGRAM_ID)
}

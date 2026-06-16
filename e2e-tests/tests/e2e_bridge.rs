//! End-to-end bridge flow — currently stubbed.
//!
//! The intent is to register an asset via `register_asset`, publish an initial
//! `update_ratio` attestation signed by a synthetic federation key set, mint Token-22
//! wrappers via `mint`, then burn them via `burn` and verify the emitted event +
//! state updates.
//!
//! ## Why this is stubbed in v0
//!
//! Two blockers stand between the lazy-claim-style in-process load and a working bridge
//! BanksClient run:
//!
//! 1. **Anchor's `entry` signature mismatch.** The macro-expanded `entry` function
//!    declared by `#[program]` has the form
//!
//!    ```ignore
//!    pub fn entry<'info>(
//!        program_id: &Pubkey,
//!        accounts: &'info [AccountInfo<'info>],
//!        data: &[u8],
//!    ) -> ProgramResult
//!    ```
//!
//!    `solana-program-test`'s `processor!()` macro expects a non-generic
//!    `fn(&Pubkey, &[AccountInfo], &[u8]) -> ProgramResult`. Wrapping `entry` in a
//!    plain function adapter with concrete lifetimes is the conventional workaround, but
//!    we also need to make sure Anchor's runtime initialization path (mostly fine in
//!    tests) doesn't trip the validator.
//!
//! 2. **Token-22 dependency.** Both `mint` and `burn` CPI into the Token-22 program with
//!    Confidential Transfer extension set up on the staccana mint. ProgramTest does ship
//!    with the SPL Token program registered (and Token-22 is loadable), but standing up
//!    a CTE-active mint requires a `Token-2022` initialize-mint flow plus the CTE
//!    extension setup — non-trivial scaffolding that's tangential to the bridge logic
//!    under test.
//!
//! ## Path to v1
//!
//! 1. `anchor build --program-name staccana_bridge` to produce
//!    `target/deploy/staccana_bridge.so`.
//! 2. Register via `program_test.add_program("staccana_bridge", staccana_bridge::ID, None)`
//!    so ProgramTest loads the BPF artifact directly.
//! 3. Pre-load the SPL Token-2022 program (built or downloaded as a `.so`) via
//!    `add_program("spl_token_2022", spl_token_2022::ID, None)`.
//! 4. Build the staccana mint with the CTE extension, set the bridge AssetConfig PDA as
//!    mint authority.
//! 5. Generate a fake federation key set (e.g. 5 of 9 ed25519 keypairs); register via
//!    `register_asset`.
//! 6. Build the §5.3 attestation message, sign with M of N keys, submit `update_ratio`,
//!    `mint`, `burn` in turn; assert the emitted `BurnEvent` and the post-state.
//!
//! Until then this file ships an `#[ignore]`d test placeholder so `cargo test` doesn't
//! complain about an empty file but anyone running `cargo test -- --ignored` sees the
//! stub message and the v1 path above.

#[ignore = "TODO v1: needs anchor build + .so load + Token-22 CTE mint setup; see file header"]
#[test]
fn bridge_register_then_mint_then_burn_full_flow() {
    // The implementation outline lives in the file header. Keeping it as a single
    // ignored test (rather than a `todo!()` body) so the test runner reports a clear
    // skipped count in CI without spuriously failing.
    panic!(
        "unreachable: this test is gated by #[ignore] until the bridge .so build pipeline lands"
    );
}

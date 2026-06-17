# Staccana Roadmap

Phased and honest. Each phase is shippable on its own.

**v1 mainnet release codename: `mainnet-sigma`**.

Target date: 2026-05-16 (~2 weeks out from Phase 0 commit). Devnet polish window between now and then; cutover to `mainnet-sigma` on the target date with whichever subset of phases is locked. See "Two-week shipping plan" below.

## Two-week shipping plan (Phase 0 → `mainnet-sigma`)

Target: 2026-05-16. The honest read of what fits in 14 days vs. what slips to v1.1.

**Fits — `mainnet-sigma` scope:**
- Snapshot ingestion + genesis builder driving real mainnet snapshot
- Lazy-claim program deployed; gas-exempt rule wired in (validator-side patch)
- CTE feature gates active at slot 0
- Treasury PDA pre-credited from partition
- Single Hetzner validator booting from genesis with classic defaults
- secret-pump deployed (SOL-quoted; no Raydium graduation pipeline yet)
- CEX listing as the on/off-ramp (no bridge)
- One semi-public RPC + minimal block explorer + claim/pump frontend
- E2e flows green on devnet between now and cutover

**Slips to v1.1 (post-`mainnet-sigma`):**
- **FBA enforcement at consensus** — banking-stage hook + replay verifier + slashing in the Agave fork. Multi-week eng lift; the matcher library is ready but actual in-validator wiring is not 14-day work.
- **secret-ray** — forked Raydium AMM v4 / CLMM / CPMM + router, plus the graduation migrator. secret-pump graduation is event-driven and latches, so this is off the genesis critical path — but it must land before the first curve hits 85 SOL.
- **On-chain drawdown distributor** — validators are hand-paid the treasury drawdown from the governance multisig at launch; a thin transfer-scheduler (no staking, no yield) follows.

So `mainnet-sigma` ships as: "Solana fork, secrecy live by default, classic v1 fee model, treasury-funded ops (drawdown, no inflation/yield), CEX-listed (no bridge), secret-pump live. FBA enforcement and secret-ray coming in v1.1." The narrative stays intact; the consensus-rule moat lands a few weeks behind the secrecy moat.

Devnet between now and 2026-05-16 is where everything in the "fits" bucket gets exercised end-to-end, and where the v1.1 work ALSO matures so it can ship 4-6 weeks after `mainnet-sigma`.

## Phase 0: Scaffold (this commit)

- [x] Workspace + docs + crate skeleton
- [x] Matcher library: per-mint AMM-anchored uniform clearing price with deterministic-replay tests
- [x] Genesis library: partition rule, treasury accumulator, Merkle root construction, classic defaults port
- [x] Lazy-claim program (native solana-program; 22 unit tests; correct end-to-end flow including marker-PDA init via system_program CPI per discovery in e2e harness)
- [x] ~~Bridge program~~ — **removed** (federated bridge cut before audit; see `docs/AUDIT_SCOPE.md`)
- [x] secret-pump (Anchor 0.30 + Token-22 CTE; 20 unit tests on bonding curve math + graduation latching)
- [x] snapshot-fork tool (mock + Solana stub with detailed integration TODO; 20 tests)
- [x] genesis-emit tool (compose pipeline; 12 tests)
- [x] claim-cli (proof construction + ix builders; 19 tests)
- [x] ~~bridge-cli, federation-attestor daemon~~ — **removed** with the bridge
- [x] integration-tests crate (cross-crate flows + SPEC byte conformance + property invariants + Merkle consistency; bridge/federation conformance dropped)
- [x] e2e-tests crate (solana-program-test in-process chain simulation; real e2e tests for claim/genesis/matcher)
- [x] Per-crate `tests/` integration files (43 tests across matcher + genesis)
- [x] Workspace `cargo check` green on the Anchor-free crates (matcher + genesis + lazy-claim)

## Phase 1: Genesis + secrecy live (2-4 weeks)

- [ ] Snapshot ingestion: pull mainnet snapshot at slot `S`, walk AccountsDB
- [ ] Wire genesis crate's partition + Merkle root + treasury against real snapshot accounts
- [ ] Lazy-claim program: Merkle proof verification + ed25519 mainnet signature verification + gas-exempt rule
- [ ] Genesis builder: classic defaults composed in, vote accounts cleared, validator set redone, treasury PDA pre-credited, ZK ElGamal Proof program activated as builtin, four CTE feature gates ON at slot 0
- [ ] Treasury bootstrap allocation reserved per SPEC §7.3 (`TREASURY_BOOTSTRAP_BPS = 200`) for direct validator subsidy in the first 60 epochs
- [ ] First validator boot from genesis on a Hetzner box
- [ ] CTE end-to-end test (mint → confidential transfer → balance check) on the live chain

**Shippable as**: "Solana, but secrecy is on by default, the state is yours, and the protocol expropriations fund the project."

## Phase 2: Anti-sandwich at consensus (4-8 weeks)

- [ ] Vixen integration as in-validator library; instruction → intent decoding for the Raydium program family
- [ ] Banking stage modification: extract intents, run matcher, rewrite tx ordering
- [ ] Replay verifier: re-derive matcher output from input set, invalid-block on mismatch
- [ ] Slashing program: new conditions for canonical-ordering violations
- [ ] No-bundle ingest: validator binary does not honor Jito-style bundle messages
- [ ] secret-ray = forked Raydium AMM v4 / CLMM / CPMM + router as the residual liquidity layer

**Shippable as**: "MEV-proof at the validator layer, required by consensus."

## Phase 3: ~~Multi-asset bridge + treasury productive position~~ — REMOVED

The federated bridge and the treasury productive position were cut before audit. No bridge,
no yield. What this phase used to carry, and its replacement:

- **User on/off-ramp** → **CEX listings** (exchange custodies + runs a node; off our audit surface).
- **On-chain stable quote (ssUSDC)** → none at genesis; SOL-quoted pools. Court native Circle/Tether later.
- **Native-SOL oracle** → **permissionless Switchboard feed pointed at the CEX**.
- **Validator funding** → **treasury principal drawdown** (no staking, no yield). Replacement work:
  - [ ] On-chain drawdown distributor: per-epoch fixed transfer from the treasury PDA, pro-rata to active validators by `(uptime × stake × votes)`. Until it ships, the multisig hand-distributes.

**Shippable as**: "CEX-listed on/off-ramp; validators paid by transparent treasury drawdown. No bridge to apologize for."

## Phase 4: secret-pump (4-6 weeks)

- [ ] Confidential bonding-curve launchpad
- [ ] Hidden buy/sell amounts during bonding curve phase
- [ ] Anti-snipe falls out for free (copy-trading bots can't read amounts)
- [ ] Treasury seeds initial pools

**Shippable as**: degen liquidity engine for the chain.

## Phase 5: Launch infrastructure (parallel with 1-4)

- [ ] Single semi-public RPC endpoint (Hetzner box)
- [ ] Block explorer (fork solana-explorer)
- [ ] Wallet integration docs (Phantom / Solflare / Backpack as custom cluster)
- [ ] Token list bootstrap
- [ ] Documentation site (claim / swap / launch)

## Phase 5.5: v1.1 dep-graph fix — Anchor 0.30 → 1.x upgrade

> Note: `programs/bridge` and `programs/validator-subsidy` were **removed** after this
> upgrade (bridge cut before audit). The Anchor-1.x upgrade details below are retained as
> history but now apply only to `secret-pump` and `megadrop`.

- [x] Workspace-wide `cargo check` (the headline blocker; passes in a single invocation)
- [x] `programs/secret-pump` and `programs/megadrop` on `anchor-lang = "1"` / `anchor-spl = "1"`
      (which use `solana-program 2.x` natively via the split `spl-token-2022-interface` crate)
- [x] Anchor account-discriminator algorithm preserved (`sha256("account:Name")[0..8]`),
      so the on-chain wire format is byte-identical
- [x] In-tree unit tests pass (`cargo test -p staccana-secret-pump --lib`)

Follow-ups (future work, no longer blocking the workspace):

- [ ] `programs/secret-pump` as a path-dep of `integration-tests` (cross-crate consistency tests)

### Breaking changes hit during the upgrade

- `CpiContext::new` / `CpiContext::new_with_signer` now take the program id as a
  `Pubkey` instead of an `AccountInfo`. Every CPI call site (4 in bridge, 6 in
  secret-pump) was updated to `ctx.accounts.<program>.key()`.
- `anchor_lang::solana_program::sysvar::instructions::{load_instruction_at_checked,
  load_current_index_checked, ID}` and `solana_program::ed25519_program::ID` are no
  longer re-exported from `anchor_lang`. Bridge and validator-subsidy now depend
  directly on `solana-instructions-sysvar = "3"` and `solana-sdk-ids = "3"`.
- `anchor_spl::token_2022` re-exports `spl-token-2022-interface` (not `spl-token-2022`).
  secret-pump renamed its direct dep with `package = "spl-token-2022-interface"` so the
  raw Token-22 ix builders (`extension::confidential_transfer::instruction::*`,
  `instruction::initialize_mint2`, `instruction::initialize_account3`) resolve to the
  same types as the anchor_spl wrappers.
- `spl_token_2022_interface::extension::confidential_transfer::instruction::initialize_mint`
  now takes `authority: Option<Pubkey>` (was `Option<&Pubkey>`).
- `spl_token_2022::state::Account::LEN` requires the `Pack` trait in scope —
  `use anchor_lang::solana_program::program_pack::Pack;` added in
  `programs/secret-pump/src/instructions/create.rs`.
- `seeds = [..., &arg.to_le_bytes()]` no longer type-checks when the `seeds` slice
  contains items of differing fixed-array sizes. Replaced with
  `arg.to_le_bytes().as_ref()` (slice form) in every Anchor `Accounts` context.
- Newer rustc tightened the borrow-checker rule on `reg.validators[reg.count as usize]
  = ...` (treated as simultaneous mutable + immutable borrow of `*reg`). Bind the
  index to a local first.
- `AccountInfo<'info>` inside `#[derive(Accounts)]` now emits a deprecation warning;
  `UncheckedAccount<'info>` is the recommended replacement. We migrated where it was
  cheap (bridge, secret-pump) and added crate-level `#![allow(deprecated)]` on
  `validator-subsidy` (whose CPI plumbing into the bridge naturally uses raw
  `AccountInfo`s).

## Phase 6+: secret-* expansion (quarters, not weeks)

In rough demand-priority order:

- secret-stake (confidential delegation)
- secret-payroll (recurring confidential payments — the legitimacy story)
- secret-orderbook / secret-RFQ (whale-grade execution)
- secret-perps
- secret-vote / secret-DAO (governance privacy)
- secret-lend

Each is a real ZK design problem, not a port. Sequence by demand.

## Phase 7: Decentralization

- [ ] Multi-validator testnet
- [ ] Public validator onboarding

## Out of scope (intentionally)

- **Encrypted mempool / threshold decryption** — overkill for the threat model. We care about atomic sandwich + Jito-style extraction, not all frontrun. The FBA kills the sandwich without paying the latency / DKG complexity cost.
- **Any cross-chain bridge** — no federated bridge, no light-client bridge. Value moves in/out via CEX listings; the exchange takes the custody/trust assumption. Non-SOL assets arrive as native issuance (Circle/Tether) once volume warrants, not as bridge-mints or snapshot carryover.
- **Public RPC operator** — one semi-public box at launch is the line. Operators who want public RPC run their own.

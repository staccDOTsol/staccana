# Lineage: from Solana Classic to Staccana

## Solana Classic (v1, May 2025)

[stacc](https://github.com/staccDOTsol)'s first take on the same problem: a Solana fork that runs on a laptop and tries to make MEV uneconomical via a fixed fee model.

- **Repo**: [staccDOTsol/solana-classic](https://github.com/staccDOTsol/solana-classic)
- **Distribution**: [`jrsdunn/solana-classic-validator`](https://hub.docker.com/r/jrsdunn/solana-classic-validator) on Docker Hub (332 pulls, multi-arch amd64 + arm64)
- **Status**: dormant since May 2025; still being pulled month-over-month with zero maintenance

### What classic shipped

Four targeted modifications to Agave 2.x:

1. **`fee/src/lib.rs`** — fixed transaction fee of 0.027 SOL (= average mainnet arbitrage profit) for non-vote txs, 5,000 lamports for votes. Thesis: if every tx costs you the average MEV extraction profit, MEV becomes economically marginal.
2. **`genesis/src/solana_classic_defaults.rs`** — disabled inflation, 50% burn rate, fee governor matching the fixed-fee model.
3. **`validator/src/commands/run/execute.rs`** — single-line hookup wiring the classic defaults into the validator startup path.
4. **`Cargo.toml`** — workspace including `programs/zk-elgamal-proof = 2.3.0` as a path dep, so the ZK ElGamal Proof program is built into the validator binary.

That's it. Tower BFT untouched. Consensus untouched. Vote/stake mechanics untouched. The two `core/src/consensus/tower1_*.rs` changes in the v1 forcepush were `frozen_abi` digest updates — bookkeeping that follows from dep bumps, not real consensus modifications.

### What classic didn't solve

- Sandwich MEV remained profitable for trades larger than the fixed fee (>0.076 SOL of value extractable per attack still > 0.027 SOL fee)
- Regular users paid 0.027 SOL/tx — high friction
- No protocol-level privacy
- No bridge to / from mainnet
- No path to capture mainnet network effects beyond "run a validator on your laptop"

## Staccana (v2, this repo)

Same target, sharper weapons. The four key changes from classic v1:

| Surface | Classic v1 | Staccana v2 |
|---|---|---|
| Anti-MEV mechanism | Fixed fee deters extraction | **Per-mint FBA at consensus eliminates extraction** |
| Privacy | Vanilla Solana | **CTE feature gates ON at genesis** |
| Bridge | None | **Non-1:1 accruing peg, multi-asset, federated v1** |
| Genesis | Fresh | **Snapshot-based with strict partition rule + treasury** |

### What carries forward from classic

- **Fee model** (0.027 SOL fixed). Kept "elevated for fun" — staccana doesn't need it for MEV deterrence (the FBA does that), but it shapes a different fee market that the v1 audience already tolerates.
- **Disabled inflation**. Validator rewards come from fees; project ops are funded from the genesis treasury.
- **50% burn rate**. Inherited.
- **`programs/zk-elgamal-proof` path-dep**. Already in classic's Cargo.toml; staccana flips its activation gate at genesis instead of just bundling the program inert.
- **Docker packaging pattern** (multi-arch, single-validator-runs-on-laptop genesis bootstrap).
- **Distribution channel**: same `jrsdunn/solana-classic-validator` image path. The next push is staccana. Existing 332-pull cohort gets upgraded on `:latest`; new pulls land on the v2 architecture.

### What's net-new in staccana

- The matcher (`/matcher`) — per-mint AMM-anchored uniform clearing
- The genesis builder (`/genesis`) — strict partition rule, treasury accumulation, Merkle root construction, classic defaults composition
- The lazy-claim program — Merkle-proof + ed25519-sig verification + gas-exempt rule
- The multi-asset bridge — non-1:1 accruing peg per asset (stSOL backed by pSYRUP, ssUSDC backed by mainnet USDC, etc.)
- secret-pump, secret-ray (forked Raydium AMM/CLMM/CPMM), and the secret-* line of confidential primitives
- ZK ElGamal Proof program activated at slot 0; four CTE feature gates flipped at genesis
- Treasury-funded project ops in lieu of inflation

## Why one repo, not two

Staccana ships as `solana-classic` v2.0.0 — same repo (`staccDOTsol/solana-classic`), same docker image path (`jrsdunn/solana-classic-validator`), same audience. The v1 architecture is preserved in the git history; v2 is the next major release.

Rationale: the docker image is the most concrete asset. 332 organic pulls over 11 months of total dormancy (with pulls continuing into the current week) means the laptop-validator-for-Solana niche has slow but real demand. Forking the brand into a separate repo loses that residual gravitational pull. Continuing as v2 keeps it.

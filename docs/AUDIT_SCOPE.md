# Staccana — Audit Scope (delta from upstream Solana/Agave)

This is the complete index of every divergence from stock Solana. Staccana is a Solana
fork; the audit surface is **not** "all of Solana" — it is the delta below. It splits into
three buckets plus an upstream-fix confirmation. Anything not listed here is unchanged
upstream Agave / SPL and out of scope.

Cluster: `mainnet-sigma`. Validator branch: `agave` 2.3 line + `staccana-ct-fixes`.

---

## What we cut, and why (summary)

We deliberately *shrank* the audit surface before engaging. Each cut below removes attack
surface or an unbacked trust assumption — none of it weakens the product (claim → SOL-quoted
secret-pump → secret-ray graduation is intact).

| Cut | Why it's gone | What replaces it |
|---|---|---|
| **Federated multi-asset bridge** (`programs/bridge`, `bridge-vault`, 5 tools) | A 5-of-9 federation that can collude to drain a vault is the single worst finding an auditor leads with. Removing it deletes the entire "trust us not to collude" surface. | **CEX listings** — the exchange custodies and runs a node; custody risk and its audit live with the exchange, off our scope. |
| **wSOL/stSOL/ssUSDC bridge mints** | stSOL/ssUSDC only existed to give the bridge yield-bearing assets and a stable quote. No bridge → no reason to mint them. | Pools are **SOL-quoted** at genesis. Only canonical **wSOL** (`So111…112`, plain Token-2022 mint, no authority) is baked, for AMM `sync_native`. Court native Circle/Tether issuance later. |
| **Treasury productive position / yield engine** | Required staking treasury principal on mainnet *via the bridge* to earn yield that paid validators. Contradicts the no-inflation/no-yield posture and re-introduces the bridge. | **Treasury principal drawdown** — no staking, no yield, no mainnet footprint. ~485M ÷ ~96k SOL/day ≈ 10+ yr runway. |
| **`validator-subsidy` program** | Its whole job was to CPI into the bridge to run the productive position and distribute *yield*. With no bridge and no yield it has no reason to exist; it was also bridge-coupled so it couldn't compile post-removal. | At launch the **Squads governance multisig hand-distributes** the drawdown; a thin no-dep drawdown distributor is a fast-follow. |
| **wSOL↔native-SOL AMM price oracle** | Was the bridge's way to price native SOL. | A **permissionless Switchboard feed pointed at the CEX** price. |
| **Bridge/federation conformance tests** (`e2e_bridge.rs`, the §5.3 ratio-attestation tests, `federation-attestor`/`bridge-cli` deps) | They tested removed code. | Claim / swap-intent / partition / matcher conformance tests retained. |

**Treasury ownership (no decision needed at genesis):** the treasury is baked **lazy-claim-owned**
because the gas-exempt claim path direct-debits it (a System-owned multisig account would break
every claim). Post-launch, the `ADMIN_AUTHORITY`-gated `DrainTreasury` / `AssignTreasuryOwner`
ixs hand custody to the **Squads governance multisig**, which becomes the drawdown source. No
multisig address is committed at slot 0.

---

## Bucket 1 — Agave validator-binary diff (`staccana-ct-fixes` branch)

The consensus/runtime layer. This is the bucket already shared.

| # | Change | Where | Notes |
|---|--------|-------|-------|
| 1 | 9 feature gates active at slot 0 (4 ZK + 5 Token-22 v8 syscall prereqs) | genesis activation set | Inactive on mainnet/devnet/testnet today. List in `docs/ARCHITECTURE.md` §"Confidential transfer gates ON". |
| 2 | ZK ElGamal Proof program re-enabled as a live builtin | `ZkE1Gama1Proof11…` | Re-enable gate. |
| 3 | `percentage_with_cap` Fiat-Shamir transcript fix | proof verifier | One of the two flagged CT bugs; backported fix. |
| 4 | **Per-mint Frequent Batch Auction in the banking stage** | `matcher/` + banking stage | Intent extraction (Vixen parsers compiled in), size-sorted pair matching, single AMM-anchored clearing price, deterministic tx rewrite. **Deepest novel surface.** |
| 5 | Replay-determinism verification of the matcher → slashing on mismatch | replay + slashing | Non-deterministic block = invalid = slashable. |
| 6 | No Jito bundle protocol | validator binary | Bundle messages not compiled in / not honored. |

**Auditor focus in this bucket:** #4/#5. A leader who can produce a block whose matching
can't be deterministically re-derived, or who can bias the size-sorted clearing, breaks the
core anti-MEV claim. The matcher is the whole point of the fork.

---

## Bucket 2 — Genesis builder (`genesis/`)

The chain is built from a Solana mainnet snapshot at slot `S`. The builder is novel code.

| # | Change | Notes |
|---|--------|-------|
| 7 | **Strict partition rule** | `claimable iff owner==SystemProgram AND data.is_empty()`, else → treasury. One rule, no allowlists. Bug here = wrong accounts survive or wrong lamports expropriated. |
| 8 | Treasury PDA credited with expropriated lamports (~400–500M SOL) | Owned by governance multisig. |
| 9 | Merkle root of the claimable partition embedded in genesis | Consumed by lazy-claim. |
| 10 | Feature-gate activation set applied at slot 0 | The 9 gates from Bucket 1. |
| 11 | **Gas-exempt `claim` instruction — CONSENSUS-LEVEL rule, not a program** | A genesis rule makes the `claim` ix fee-exempt (solves the claim-needs-gas-needs-claim deadlock). Fee-exemption is a runtime change; audit for abuse (free compute, spam, exemption scope). |
| 12 | Classic v1 economic defaults | Fixed fee (0.027 SOL non-vote / 5000 lamports vote), **inflation disabled**, 50% fee burn, genesis hash distinct from mainnet (no cross-chain double-sign slashing). |

---

## Bucket 3 — On-chain program set

New programs deployed at/near genesis. Each is its own audit surface.

### Launch-blocking (IN scope)
| Program | Surface |
|---------|---------|
| `lazy-claim` | Merkle inclusion proof + ed25519 signature verify + idempotent one-shot per account + gas-exempt rule. Forge a proof or replay a claim → mint SOL from nothing. |
| `secret-pump` | Confidential bonding-curve launchpad (Token-22 / CTE). |
| `secret-ray` | Forked Raydium AMM v4 / CLMM / CPMM + router, Token-22/CTE-aware. SOL-quoted pools at launch (no stable quote — see "Removed"). |
| FBA matcher | Covered in Bucket 1 #4/#5; the on-chain/consensus seam. |

### Deferred (OUT of launch scope — not deployed at genesis)
`megadrop`, `agent-faucet`, `agent-messaging`/`agent-mail`, and `validator-subsidy`
(must be rewritten as a no-dep drawdown distributor before it ships — see "Removed").

---

## Upstream Token-2022 fixes to confirm in the vendored ELF

The two bugs flagged to CertiK are **upstream solana-program-library** issues, not staccana
originals. Action: diff staccana's vendored Token-2022 ELF against the fixed upstream commits
to confirm both fixes are present in what ships at genesis.

- **Bug #1 — transfer-fee circumvention** (deposit/withdraw didn't enforce same source/dest).
- **Bug #2 — non-transferable token bypass** (missing non-transferable extension check).

---

## Removed / explicitly OUT of scope

These existed in earlier designs and are **deleted** — do not audit, they don't ship:

- **Federated multi-asset bridge** (`programs/bridge`, `programs/bridge-vault`, and the
  `bridge-cli` / `bridge-init` / `bridge-vault-init` / `wssol-init` / `federation-attestor`
  tools). The 5-of-9 federation was the single worst liability; removed. **User on/off-ramp
  is CEX listings.**
- **wSOL / stSOL / ssUSDC** bridge mints and their genesis seeding. Gone with the bridge.
- **Treasury productive position / yield engine.** Inflation stays off; validators are paid
  by **direct principal drawdown** of the treasury, not yield. No mainnet staking position.
- **wSOL↔native-SOL AMM oracle.** Replaced by a **permissionless Switchboard feed pointed at
  the CEX price** for native-SOL/USD.

---

## One-line scope statement for CertiK

> Audit the delta from Agave 2.3 + SPL: (1) the `staccana-ct-fixes` validator diff —
> CT feature gates + the per-mint FBA matcher and its slashing/replay determinism; (2) the
> genesis builder — partition rule, treasury credit, Merkle root, and the consensus-level
> gas-exempt `claim` rule; (3) the launch programs lazy-claim, secret-ray, secret-pump.
> Confirm the vendored Token-2022 ELF carries both upstream fixes. No bridge, no yield
> engine — those are removed.

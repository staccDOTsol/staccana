# Staccana Architecture

## North star

A Solana fork where:

- **Confidential transfers are live at genesis** (Token-22 extensions active from slot 0)
- **Atomic sandwich MEV is structurally impossible** (per-mint FBA, no bundles, canonical clearing)
- **Genesis is built from a mainnet snapshot via a strict partition rule** (only raw-SOL EOAs survive; everything else funds the treasury)
- **Treasury funds project ops, not inflation** (inherited from classic v1's disabled-inflation choice)
- **The chain runs on a laptop** for v1; scales when warranted

## Lineage

Staccana = `solana-classic` v2.0.0. The v1 architecture (fixed-fee MEV deterrent, disabled inflation, 50% burn, single-validator-on-laptop docker image) shipped in May 2025 and accumulated 332 organic Docker pulls over a year of dormancy. v2 keeps the fee model + economic posture + distribution channel; replaces the deterrent-via-fees mechanism with structural anti-MEV at consensus; adds secrecy at genesis; adds a multi-asset bridge.

See `docs/LINEAGE.md`.

## Genesis

Built from a Solana mainnet snapshot at slot `S`.

### Partition rule

Every account in the snapshot is partitioned by **one rule**:

```
Claimable  iff   account.owner == SystemProgram::id  AND  account.data.is_empty()
Treasury   otherwise
```

That's it. No allowlists, no excluded-protocol maintenance, no judgment calls. Effects:

- Plain SOL on personal wallets (system-owned EOAs with zero data) → **claimable** via lazy-claim
- Token accounts (data-bearing, Token program-owned) → **treasury**
- Stake accounts (data-bearing) → **treasury**
- Vote accounts (data-bearing) → **treasury** (and staccana redoes the validator set at genesis anyway)
- Multisig SOL (Squads etc., data-bearing) → **treasury**
- DeFi positions, NFTs, every PDA → **treasury**

Approximate effect on mainnet's ~600M SOL supply:
- ~100-150M SOL claimable (raw EOA balances)
- ~400-500M SOL → treasury (staked, locked, in protocols)

The bridge becomes the dominant path for the staked majority to enter staccana — drives bridge fee volume, grows the stSOL R ratio.

### Treasury

The lamports accumulated from the treasury partition are credited at slot 0 to a `treasury` PDA owned by a staccana governance multisig. Uses:

- Seed liquidity for secret-pump bonding curves
- Initial pools for secret-ray (so launch isn't an empty AMM)
- Bridge insurance fund
- Project ops, grants
- **Validator subsidy** — the load-bearing one (see below)

#### Validator subsidy (since inflation is disabled)

Inflation is disabled (classic v1 inheritance), the FBA structurally eliminates MEV revenue, and base fees are tiny at low TPS. Validators need an income source that doesn't depend on chain throughput, and that income shouldn't dilute SOL holders.

The treasury solves this. Sized at ~400-500M SOL at genesis, staked productively (initially as pSYRUP via the bridge; long-term as staccana-native staking once a non-trivial validator set exists), it generates roughly:

```
500M SOL × 7% APY ≈ 35M SOL/year ≈ 96k SOL/day
```

Far more than launch-TPS base fees can pay validators. The **yield** (NOT the principal) is distributed pro-rata to active validators each epoch, weighted by `(uptime × delegated-stake × votes-cast)`.

**Bootstrap window**: until the staking position has earned its first yield, a small reserved direct-allocation (`TREASURY_BOOTSTRAP_BPS = 200` of treasury per SPEC §7.3) funds validators directly for ~30 days. After that, yield-only.

Net effect: staccana validators are paid from yield on capital the chain expropriated from dormant mainnet protocols. Non-dilutive, self-funding indefinitely, cleaner story than "we have inflation but less of it."

### Lazy claim

The `lazy-claim` program lives at a well-known address at genesis. It holds the Merkle root of the claimable partition. Users claim by:

1. Producing an inclusion proof for `(account_pubkey, lamports)` against the embedded root.
2. Signing the claim payload with the keypair that controls `account_pubkey` on mainnet.
3. Submitting both to the claim program.

The program verifies the proof and the signature, then writes the account into staccana's AccountsDB. Claims are idempotent and one-shot per account.

**Gas exemption**: the `claim` instruction is fee-exempt by genesis rule (the lazy-claim program covers the fee from the treasury). Otherwise users face a chicken-and-egg problem: claim requires gas, gas requires claim.

### Confidential transfer gates ON (+ Token-22 v8 syscall prerequisites)

The ZK ElGamal Proof program (`ZkE1Gama1Proof11111111111111111111111111111`) ships as a live builtin — it's already a path-dep in classic v1's Cargo.toml as `programs/zk-elgamal-proof = 2.3.0`. Genesis activates **9 feature gates** at slot 0 (see `staccana_genesis::CTE_FEATURE_GATES_AT_GENESIS`):

**ZK / confidential transfer (4 gates):**

- `zk1snxsc6Fh3wsGNbbHAJNHiJoYgF29mMnTSusGx5EJ` — enable Zk Token proof program and syscalls
- `zkesAyFB19sTkX8i9ReoKaMNDA4YNTPYJpZKPDt7FMW` — re-enable zk-elgamal-proof program
- `zkNLP7EQALfC1TYeB3biDU7akDckj8iPkvh9y2Mt2K3` — enable Zk Token proof program transfer with fee
- `zkiTNuzBKxrCLMKehzuQeKZyLtX2yvFcEKMML8nExU8` — proof from accounts instead of ix data

**Token-22 v8 syscall prerequisites (5 gates):**

The spl-token-2022 v8 ELF references `sol_curve_group_op`, `sol_alt_bn128_*`, `sol_big_mod_exp`, and `sol_poseidon` directly. With those gates inactive at deploy time, the BPF loader fails with `Unresolved symbol (sol_curve_group_op)` and the program never lands. Token-22 v8 is the version that ships the on-chain proof verifier we need — it must be deployable from slot 0.

- `7rcw5UtqgDTBBv2EcynNfYckgdAaH1MAsCjKgXMkN7Ri` — curve25519 syscalls (`sol_curve_group_op`, `sol_curve_multiscalar_mul`, `sol_curve_validate_point`)
- `A16q37opZdQMCbe5qJ6xpBB9usykfv8jZaMkxvZQi4GJ` — alt_bn128 syscalls (`sol_alt_bn128_group_op`)
- `EJJewYSddEEtSZHiqugnvhQHiWyZKjkFDQASd7oKSagn` — big_mod_exp syscall (`sol_big_mod_exp`)
- `EeyoXa3AyQuHkhRmT9mhKtTPrLNPBuNQbLEvyt5VrYxv` — alt_bn128 compression syscall (`sol_alt_bn128_compression`)
- `EaQpmC6GtRssaZ3PCUM5YksGqUdMLeZ46BQXYtHYakDS` — poseidon syscall (`sol_poseidon`)

All 9 are **inactive on mainnet, devnet, and testnet** as of fork time — Token-22 mints opting into Confidential Transfer don't work on vanilla Solana today, but they work on staccana from slot 0 with no governance vote and no waiting for an epoch-boundary activation.

### Classic v1 defaults inherited

Reused unchanged from `solana-classic v1`:

- **Fee model**: 0.027 SOL fixed for non-vote txs, 5,000 lamports for votes
- **Inflation**: disabled
- **Burn rate**: 50% (the half of fees not going to validators)
- **Genesis hash**: distinct from mainnet's (no cross-chain double-signing slashing risk)

## Consensus rule: per-mint frequent batch auction

The signature change on staccana relative to vanilla Solana lives at the validator layer. Within each slot:

1. **Swap intents are extracted** from the slot's transactions. An intent is `{signer, in_mint, in_amount, out_mint, min_out, nonce}`. Yellowstone Vixen parsers (compiled into the validator binary as a library) decode known DEX program calls into intents. For v1 the registry covers the forked Raydium program family (secret-ray AMM v4 / CLMM / CPMM) plus secret-pump bonding curve trades.
2. **Intents are grouped by base mint.** Each intent's base mint is the longtail (non-quote) side; quotes are USDC, USDT, native SOL, and a small registry of others. Quote mints don't get their own batch — they participate as the counterparty side of base-mint batches. Quote-vs-quote pairs (USDC↔USDT) fall back to a deterministic tiebreak (lex order in v1; volume rank later).
3. **Each base-mint batch clears at a single AMM-anchored price.**
   - Compute net base demand: Σ(buys translated to base via P_pre) − Σ(sells in base).
   - Hit the AMM with the net only → produces P_post.
   - Clearing price = midpoint of P_pre and P_post.
   - Match buyers and sellers in size order (largest first), pair-wise, at the clearing price.
   - Anything unmatched (residual) hits the AMM at the post-batch price.
4. **The leader rewrites the slot's transactions** into a deterministic ordered sequence: matched crossings first, then residual AMM hits. Block header commits to the input intent set's hash.
5. **Replay verifies determinism.** Any other validator can re-derive the same matching from the same input set. Mismatch = invalid block. Producing an invalid block is a slashing condition.

The key property: the leader has zero discretion over ordering. They can't insert their own tx between two others. Atomic sandwich requires both tail-end placements to land deterministically; under size-sorted matching at uniform clearing, the searcher's "buy ahead" tx and "sell behind" tx clear at the same price as the victim's swap. There is nothing to extract.

What is preserved: cross-DEX arb (the arb tx is its own intent, gets matched or hits AMM at clearing price, profits if pools are mispriced), liquidations (single-tx, AMM-bound), backruns (single-tx, no atomicity guarantee). What dies: sandwich, Jito-style bundles, leader-extracted reorder rent.

## Pipeline

```
banking stage receives txs
  ↓
Vixen parsers decode each ix → typed SwapIntent { signer, in_mint, in_amount, out_mint, min_out }
  ↓
batch matcher per (base, quote) pair:
  • partition into buys vs sells (relative to base)
  • compute net flow → query AMM → compute clearing price
  • size-sorted pair matching at clearing price
  • residual → AMM at clearing price
  ↓
deterministic rewritten ix sequence
  ↓
execution
  ↓
replay: re-derive matcher output from input set → invalid block on mismatch (slashable)
```

## Bridge

Federated v1 with a **non-1:1 accruing peg**, multi-asset from day one (not stSOL only). Each supported asset has a mainnet vault holding yield-bearing backing and a corresponding Token-22 mint on staccana with the Confidential Transfer extension active by default. Detailed design in `docs/BRIDGE.md`.

## Out-the-gate launch slate

What ships when the chain goes live:

**Core (without these the chain doesn't function)**
- Validator binary running from genesis (forked Agave, classic v1 patches inherited, FBA layered on)
- Lazy-claim program with Merkle root + ed25519 verification + gas-exempt rule
- Multi-asset bridge: stSOL (pSYRUP-backed) AND ssUSDC (mainnet-USDC-backed) at minimum
- secret-ray (forked Raydium AMM/CLMM/CPMM + router)
- secret-pump (confidential bonding-curve launchpad)
- One semi-public RPC endpoint (single Hetzner box at launch)

**Within days of launch**
- Block explorer (forked solscan or solana-explorer pointed at staccana RPC)
- Wallet config docs (Phantom/Solflare/Backpack as custom Solana cluster)
- Token list bootstrap (SOL, stSOL, ssUSDC; secret-pump tokens auto-register)

**Within weeks**
- Token registry / metadata service for secret-ray pool display
- Documentation site (claim, bridge, swap, launch flows)

**Later** (separate phases, not launch-blocking)
- secret-stake, secret-payroll, secret-perps, secret-orderbook, secret-vote, secret-lend
- Multi-validator decentralization
- Public RPC scaling
- Federation expansion / bridge trust-minimization

## Hardware

Designed to run on a single dedicated server (Hetzner AX-line class, ~$150/mo) for v1. Lazy-claim genesis means AccountsDB starts at near-zero; growth driven by actual on-chain activity. Pruning aggressive by default. Secondary indexes (tx history) off unless explicitly enabled.

A laptop validator works for testnet/devnet from genesis (this was the proven point of classic v1, with 332 organic Docker pulls validating the niche). Mainnet-grade staccana validators target the Hetzner class; laptop validation remains supported as a "follower" or testnet path.

## What this fork does not change

- VM semantics (Sealevel, BPF programs run as on mainnet)
- Slot timing, tick rate, epoch length (defaults inherited from Solana)
- Account model, rent
- Transaction format

The intrusive surfaces are: the genesis builder (partition + treasury + Merkle root + classic defaults + feature gate activation), the banking stage's intent extraction + matching, and the slashing program updates.

# staccana launch — devnet drop, 2026-05-02

The chain is alive. This file is the receipt — what shipped, where to find it,
and the numbers as they came out of the genesis-bake pipeline.

## the headline numbers

Forked from Solana mainnet at slot **417,107,143** (2026-05-02). The
`staccana-snapshot-fork` walked every storage in the snapshot, applied the
partition rule from `docs/ARCHITECTURE.md` §3, and split the account graph in
two:

| bucket             | accounts        | balance                | rule |
|--------------------|----------------:|-----------------------:|------|
| **claimable**      | **85,655,757** | (per-leaf in airdrop)  | EOAs the lazy-claim merkle root pays out to |
| **treasury**       |  1,041,776,142 | **485,192,075.14 SOL** | swept to the genesis treasury PDA |
| total unique       |  1,127,431,899 | —                      | every account in the source snapshot |

That bottom-row `total unique` is — to our knowledge — the largest account
graph anyone has rebooted as a fresh L1 from a Solana fork. Every one of those
balances exists in genesis at slot 0; nothing was migrated post-boot.

The lazy-claim Merkle root is embedded in the genesis treasury config:
`0xfd9922eb088f9e087cdfee5020b20e453f0f6f4167d2b517387e7c66348d6134`

The 30M-SOL **megadrop** to based-stacc-0 NFT holders + proofv3 token holders
ships on a separate root, computed by `tools/megadrop-merkle` against the
operator's snapshot files at `~/megadrop-snapshot-2026-05-02T1900Z/`:
- 826 unique recipients (266 NFT holders + 577 token holders, dedup'd)
- exact lamport sum: 30,000,000,000,000,000 (30M SOL)
- root: `0x4cd7098ee9dec30f8fa3818401dbb74876302a1075b429d20c6e324c7f07d237`
- Gini: 0.910 (heavy-tailed by design — the NFT bonus + token bonus
  formulae in `tools/megadrop-merkle/src/allocate.rs` weight active holders
  significantly higher than dust holders)

## economic posture (inherited from solana-classic v1)

- **Inflation: disabled.** No new SOL ever issued post-genesis. Validator
  rewards come from fees only.
- **Fixed transaction fee: 0.027 SOL** for non-vote txs (5,000 lamports for
  votes). The fixed-fee model is one of v1's two structural anti-MEV
  mechanisms; v2 keeps it because the v1 audience already tolerated it.
- **50% burn rate.** Half of every fee burns, half goes to validators.

`docs/ARCHITECTURE.md` §1 has the full reasoning.

## feature gates active at slot 0

`staccana-genesis::CTE_FEATURE_GATES_AT_GENESIS` (9 gates total) ships ON at
slot 0. Five of these are the syscall prerequisites for **Token-22 v8** (the
version with the on-chain proof verifier needed for confidential transfers).
On vanilla Solana mainnet/devnet/testnet these are still inactive — meaning a
plain Token-22 v8 deploy fails with `Unresolved symbol (sol_curve_group_op)`.
On staccana it just works.

The 9 gates:

| pubkey                                          | what it enables |
|-------------------------------------------------|-----------------|
| `zk1snxsc6Fh3wsGNbbHAJNHiJoYgF29mMnTSusGx5EJ`   | Zk Token proof program + syscalls |
| `zkesAyFB19sTkX8i9ReoKaMNDA4YNTPYJpZKPDt7FMW`   | re-enables zk-elgamal-proof program |
| `zkNLP7EQALfC1TYeB3biDU7akDckj8iPkvh9y2Mt2K3`   | Zk Token proof transfer with fee |
| `zkiTNuzBKxrCLMKehzuQeKZyLtX2yvFcEKMML8nExU8`   | proof-from-account (instead of ix data) |
| `7rcw5UtqgDTBBv2EcynNfYckgdAaH1MAsCjKgXMkN7Ri`  | curve25519 syscalls (sol_curve_group_op et al.) |
| `A16q37opZdQMCbe5qJ6xpBB9usykfv8jZaMkxvZQi4GJ`  | alt_bn128 syscalls |
| `EJJewYSddEEtSZHiqugnvhQHiWyZKjkFDQASd7oKSagn`  | big_mod_exp syscall |
| `EeyoXa3AyQuHkhRmT9mhKtTPrLNPBuNQbLEvyt5VrYxv`  | alt_bn128 compression syscall |
| `EaQpmC6GtRssaZ3PCUM5YksGqUdMLeZ46BQXYtHYakDS`  | poseidon syscall |

## fleet

4 validators, 2 regions:

| node     | host             | region   | role |
|----------|------------------|----------|------|
| val-1    | 84.32.220.211    | US       | bootstrap + RPC |
| val-2    | 84.32.220.76     | US       | follower |
| val-3    | 84.32.103.186    | US       | follower |
| val-4    | 84.32.64.19      | NL       | follower |

Each validator: 25% stake, 1000 SOL bootstrap balance (genesis-baked).
Supermajority is 3-of-4 = 75%. The cluster tolerates one validator down.

## endpoints

- **Public RPC**: `https://rpc.mp.fun` (cloudflared tunnel from val-1's localhost:8899; full RPC API + transaction history + extended tx metadata + block subscription enabled)
- **App**: `https://app.mp.fun` — claim, bridge, pump UI
- **Explorer**: `https://explorer.mp.fun` — staccana-branded fork of solana-labs/explorer with XMR-style ConfidentialIndicator components for Token-22 CTE accounts
- **Docker image** (laptop validator): `jrsdunn/solana-classic-validator:v2.0.0-devnet-20260502` and `:latest` (slug inherited from solana-classic v1's 332 organic Docker pulls; v2 keeps the brand and the audience)

## live chain

- **Genesis hash**: `75Ymas2GiSjX4YRGHE9oJKoXKthYSKBimGycC62Pyswd`
- **Initial validator set**: 4 (BtTrfSMe, GfqwY5En, 6dG5FtSn, 4fk2Ky8K) × 1000 SOL stake each
- **Epoch length**: 1024 slots
- **Block time**: ~400 ms (Solana default)
- **First block produced**: 2026-05-03 ~01:45 UTC

## programs deployed

5 staccana programs + 4 SPL-stack programs, all live on staccana devnet
2026-05-03. IDs land in `/etc/staccana/program-ids.json` on val-1.

### staccana programs

- **lazy-claim**         `BK95n7mFdF7Wk5T8oiSFLtmULprQe6bRcpgLMQGC3oeK` — the 85.6M claimable airdrop (root-embedded in genesis)
- **bridge**             `LA7h3hjvD62MeTtdeE4h2vq3EGxbU1oqzHtewp4xb9b` — staccana-side bridge (wSOL R-locked + AMM-oracle-quoted native SOL, see `docs/BRIDGE.md`)
- **secret-pump**        `3Pbv3bHBh7SvcMDZqBFjJ3T9jLdrpiednaTRdViitMWF` — Token-22 CTE bonding-curve launcher ("pump.fun, but every balance encrypted by default")
- **validator-subsidy**  `Subsidy111111111111111111111111111111111111` — treasury → validator runtime payouts
- **megadrop**           `Aicff1zk6b5ifYzFoyhenUD5ehhFYb8GiDbRCrWt9t34` — 30M SOL second drop (root `0x4cd7098e…` initialized post-boot at PDA `GSPLWBykuVJNjFyqeLKMDkpoi4rZSD4fkXzVVwL9xGpV`)

### SPL stack (deployed at fresh addresses; canonical IDs require Anza's keypairs)

- **Token-22 v8**         `7bFHH22ASoMF1MGPvKPSWVfKXku8UJQUh355rmdrwAjU` — confidential transfers work day one
- **SPL Token v3**        `4PsxvxhPuysYQAf8FrggZKQvxQkCVG6hQCVHVJFrmFRj`
- **Associated Token**    `2osq4Xf5YxbpyR4nWqJkqpsyRYwPrVD6CjZztCHCvYd6`
- **SPL Memo v3**         `2o6EJBtsFaf4yBpgZ992zjaQPjukUFHZT7SmE2J8e9pG`

### Solana side

- **bridge-vault**        `F2AypZ8FDWnR5bdyLHzo4idof9YrBpdBmbgLwLBjLfVU` (devnet) — escrows wSOL/stSOL/ssUSDC, releases on M-of-N federation attestation

## federation

5-of-9 multisig federation runs the cross-chain attestor daemon
(`tools/federation-attestor/`). 9 instances, templated systemd unit at
`/etc/systemd/system/staccana-federation-attestor@.service`, signer keys at
`/etc/staccana/federation/signer-{1..9}.json`. Each instance polls both
chains for `DepositEvent` (Solana → staccana) and `BurnEvent` (staccana →
Solana), signs the corresponding `STACCANA_MINT_V1` (68B) or
`MAINNET_RELEASE_V1` (70B) attestation message, persists its cursor at
`/var/lib/staccana/attestor/attestor-state-<signer>.json`. M=5 signatures
unlock a mint or release.

## what's NOT in this drop (deferred)

- `secret-ray` — forked Raydium AMM v4 / CLMM / CPMM + router. Spec in
  `docs/SECRET_RAY.md`. v1.0 (mainnet-sigma).
- `secret-stake` — payroll-style streaming.
- `secret-payroll` — one-shot recurring transfers.
- FBA matcher activation in secret-pump (matcher contract spec in
  `docs/SPEC.md` §6, but the v0 launch routes through the curve directly).
- Multi-arch arm64 docker image. v2.0.0-devnet-20260502 ships amd64 only.
  Multi-arch follows for mainnet-sigma.

## one-liners for socials

> **staccana is alive.** 1,127,431,899 accounts forked from Solana mainnet at
> slot 417,107,143. 85.6M wallets are airdrop-eligible at genesis. 485,192,075
> SOL pre-allocated to the treasury. Token-22 v8 with confidential transfers
> works on day one — no governance vote needed. https://app.mp.fun

> If you held the based-stacc-0 NFT or proofv3 tokens on 2026-05-02 you have a
> 30M SOL megadrop waiting at https://app.mp.fun/claim. 826 wallets share it.
> Merkle root: `0x4cd7098e...`

> The image is `jrsdunn/solana-classic-validator:latest`. v1 had 332 pulls
> over a year of dormancy. v2 is the chain it was always going to be.

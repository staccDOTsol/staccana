# Megadrop — based_stacc_0 + proofv3 holder claim

**Status**: design v0.2 (revised 2026-05-02). Allocation source + vesting model fixed; remaining open items called out at the bottom.

## Mechanism

Snapshotted holders of two NFT collections on Solana mainnet — `based_stacc_0` and `proofv3` — are eligible to **claim** a per-holder allocation **out of the staccana treasury** (the genesis-time rake from stripped mainnet protocol PDAs and other non-claimable accounts per SPEC §3.1).

This is **not a new token issuance** and **not a unilateral airdrop**. It's a holder-initiated claim against treasury lamports that already exist at slot 0. The user pulls; nothing is pushed.

### Vesting

Each holder's allocation **unlocks in 10 equal monthly tranches**. One tranche per calendar month. After the 10th month, the full allocation is claimable.

A holder can claim:
- Tranche `i` if `current_calendar_month - genesis_month >= i` and the holder has not already claimed tranche `i`
- Multiple back-tranches in one tx if they've been dormant (e.g. claim months 3 + 4 + 5 in a single tx if they wake up in month 5)
- They never lose unclaimed tranches — no expiry; the treasury holds the allocation indefinitely until claimed

### Why this shape

- **No new issuance**: keeps the chain's total supply story clean (SOL-only; no separate airdrop token)
- **Source is the rake**: aligns with the project's narrative that protocol state was expropriated to fund the chain — early supporters get a piece of that rake
- **Vesting kills the dump**: 10 monthly tranches mean the unlock is 12-18 months tail rather than a one-day cliff that craters the price
- **Pull, not push**: holders who don't want to claim never have to interact; treasury isn't drained on holders' behalf

## Mechanism (technical)

A new program: `programs/megadrop/`. Anchor 1.x (matches the rest of the workspace post-Phase-5.5).

### State

```
MegadropConfig PDA  ["megadrop_config"]
    ├── claimable_root: [u8; 32]      // merkle root over (holder_pubkey, total_allocation_lamports)
    ├── genesis_month: u32             // ISO calendar month (e.g. 202605 = 2026-05) at deploy
    ├── total_allocation: u64          // sum of all leaf allocations; sanity check
    └── treasury_authority: Pubkey     // PDA-derived signer that drains treasury

ClaimedMegadrop PDA  ["megadrop_claimed", holder_pubkey]
    ├── total_allocation: u64          // mirror of leaf for fast lookup
    ├── tranches_claimed: u8           // bitmap; bit i set = tranche i+1 claimed
    └── total_claimed_lamports: u64
```

### Claim instruction

```
claim_megadrop(args):
    args.holder_pubkey: [u8; 32]
    args.total_allocation: u64
    args.tranche_indices: Vec<u8>       // which tranches to claim (1..=10)
    args.proof: Vec<[u8; 32]>           // Merkle inclusion proof against claimable_root
    args.proof_flags: Vec<u8>           // packed bit flags (left/right at each level)

Verifies:
    1. Merkle proof against MegadropConfig.claimable_root for (holder_pubkey, total_allocation)
    2. ed25519 sig from holder_pubkey on the message
       b"STACCANA_MEGADROP_V1" || holder_pubkey || total_allocation_le || tranche_indices || program_id
       (signed via prior precompile ix, inspected via Instructions sysvar — mirrors lazy-claim)
    3. Each requested tranche `i` satisfies `current_month >= genesis_month + i`
    4. Each requested tranche has not yet been claimed (bitmap check)

Effects:
    1. Mark tranche bits in ClaimedMegadrop
    2. Compute claim_amount = sum(tranches_claimed_in_this_ix) × (total_allocation / 10)
    3. Transfer claim_amount lamports from the treasury PDA to holder_pubkey
    4. Update ClaimedMegadrop.total_claimed_lamports
```

### Snapshot inputs

- **`based_stacc_0`** — Metaplex NFT collection.
  Verified collection key: `Ej1jbbw7QKgC9XMmWPxKFipMLJY5oVNd3rdbE1TzjNdz`
  Snapshot semantics: walk all NFTs whose `metadata.collection.key` matches this and the verified flag is true; group by `owner`. Per-holder count = number of NFTs.

- **`proofv3`** — Token-22 SPL fungible mint.
  Mint address: `CLWeikxiw8pC9JEtZt14fqDzYfXF7uVwLuvnJPkrE7av`
  Snapshot semantics: walk all Token-22 token accounts where `mint == proofv3`; group by `owner`. Per-holder amount = sum of `balance` across that holder's token accounts (a holder may have multiple ATAs).

- Snapshot slot `S_megadrop` — by default the same slot `S` as the lazy-claim genesis snapshot, so the two cohorts are observed simultaneously and there's no temporal arbitrage.

### Tooling

A new tool: `tools/megadrop-snapshot/`. Walks the two NFT collections at slot `S_megadrop` (via Helius DAS API or by walking the snapshot directly), groups by holder, computes per-holder NFT counts per collection, applies the allocation formula, builds the Merkle tree.

Outputs:
- `megadrop-allocations.json` — `[(holder_pubkey, total_allocation_lamports, breakdown)]`
- `megadrop-merkle-root.hex` — the 32-byte root to embed in `MegadropConfig`
- `megadrop-proofs.json` — per-holder inclusion proof (for the frontend / CLI)

## Allocation parameters (locked)

```
TOTAL_MEGADROP_SOL            = 30_000_000      # 30M SOL — Option A; ~6-8% of expected treasury
COLLECTION_BASED_STACC_WEIGHT = 60              # of 100
COLLECTION_PROOFV3_WEIGHT     = 40              # of 100
ALLOCATION_MODEL              = "linear"        # pro-rata to holdings; whale-favorable
NUM_TRANCHES                  = 10              # one tranche per calendar month
TRANCHE_AMOUNT_SOL            = 3_000_000       # = TOTAL / NUM_TRANCHES; per-month pool size
GENESIS_MONTH                 = 202605          # first tranche unlocks May 2026 (mainnet-sigma)
```

Per-holder weight under `linear`:

```
weight(h) = WEIGHT_BASED_STACC_0 * nfts_held_in_based_stacc_0(h)
          + WEIGHT_PROOFV3       * proofv3_balance(h)
```

Per-holder allocation: `(weight(h) / sum_of_all_weights) * TOTAL_MEGADROP_SOL`. This is a one-shot computation done by the snapshot tool at slot `S_megadrop`; the result is committed in the Merkle root and never recomputed.

Budget fits cleanly against the treasury: 30M megadrop is a small slice of the ~485M treasury, leaving the bulk for the validator-subsidy drawdown plus ops, AMM seeding, and grants. Comfortable.

## Vesting timeline

```
Tranche 1: May 2026 (mainnet-sigma launch month) → 3M SOL claimable across all holders
Tranche 2: Jun 2026 → +3M SOL
Tranche 3: Jul 2026 → +3M SOL
...
Tranche 10: Feb 2027 → final +3M SOL
```

Each tranche unlocks `TRANCHE_AMOUNT_SOL / total_eligible_holders × per_holder_weight_share` for each holder. Holders pull when `current_month >= GENESIS_MONTH + (i - 1)`. Multiple back-tranches in a single tx if dormant. Unclaimed tranches stay unclaimed indefinitely (no expiry).

> **Megadrop scaffold subagent**: started before this option was locked. Its initial defaults will say 300M / sqrt; the program itself is deploy-time configurable so the actual values come from `init_megadrop` args. Will swap defaults in the agent's deliverable when it returns.

## Genesis impact

At deploy time:

- The `megadrop` program is deployed as a regular program (not a builtin)
- The treasury PDA's authority is updated to allow the megadrop program to drain it for valid claims (per SPEC §7.1 — extending the authorized-operations list)
- The `MegadropConfig` PDA is initialized with `claimable_root` and `genesis_month`
- The total `TOTAL_MEGADROP_SOL` is reserved from the treasury accounting (sanity check; lamports stay in the treasury PDA, not moved)

## Frontend

`app.mp.fun/megadrop` — same pattern as `/claim`:

1. Connect wallet
2. Frontend looks up `holder_pubkey` in cached `megadrop-allocations.json`
3. Shows: "You have X SOL allocated. Y tranches unlocked, Z claimed. Click to claim available tranches."
4. Constructs proof + signature + claim ix
5. Submits

## Open items

- Confirm `TOTAL_MEGADROP_SOL` (5M? 10M? 50M? what % of treasury feels right)
- Confirm `ALLOCATION_MODEL` (uniform | linear | sqrt) — lean sqrt
- Confirm collection identifiers exactly (mainnet mint authority pubkey for each of `based_stacc_0` and `proofv3`)
- Snapshot slot — same as the lazy-claim genesis snapshot or a different slot?
- Frontend deploys at `/megadrop` route on `app.mp.fun`
- Calendar month math on-chain — needs a sysvar-clock read; SPEC §10 will add the formula

## What this replaces (deleted from this doc)

The earlier v0 of this doc framed the megadrop as a **separate token** issued **at genesis** (architecture A: extend lazy-claim with a second root, OR architecture B: standalone airdrop program issuing new tokens). Both are wrong. The revised mechanism above is treasury-funded, claim-initiated, vesting-gated. Cleaner narrative, less code, no token issuance.

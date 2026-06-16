# Block Explorer

`https://explorer.mp.fun/` — public block explorer for staccana.

## Decision: fork `solana-explorer`

Ship a fork of Solana Labs' [`solana-labs/explorer`](https://github.com/solana-labs/explorer) (Apache-2.0, Next.js, Vercel-deployable). Most mainnet-sigma launches don't get an explorer at all; we get one cheaply because the upstream is permissively licensed and trivially configurable.

**Why not:**

- **Solscan / Helius XRAY**: closed/proprietary, no public staccana support, can't fork, would require a business deal.
- **Build from scratch**: weeks of frontend work for table stakes (transaction view, account view, block view). Pointless when the upstream does 90% of it.
- **Jupiter Terminal-style**: not an explorer.

## Repo

```
git clone https://github.com/solana-labs/explorer staccana-explorer
cd staccana-explorer
git remote add upstream https://github.com/solana-labs/explorer
# track upstream so we can pull security fixes; rebase forks regularly
```

Lives in its own repo (`staccDOTsol/staccana-explorer`), not in the main `solana-classic` workspace — different framework (Next.js vs Rust crates), different deploy lifecycle.

## Configuration

`staccana-explorer` is a Next.js app. The cluster picker lives in `app/providers/cluster.tsx` (or similar — varies by upstream version). Add staccana as a custom cluster:

```typescript
// app/utils/cluster.ts
export const STACCANA_CLUSTER: Cluster = {
  name: 'Staccana',
  rpc: 'https://rpc.mp.fun/',
  websocket: 'wss://rpc.mp.fun/',
  customLabel: 'mainnet-sigma',
};
```

Make staccana the default; mainnet/devnet/testnet stay accessible via the cluster picker for users who want to compare.

## Branding tweaks (minimum viable)

- Logo / favicon → mp.fun branding
- Color theme → match the `app.mp.fun` frontend (consistent visual identity across surfaces)
- Footer → "Powered by `solana-explorer` (Apache-2.0). Source: github.com/staccDOTsol/staccana-explorer"
- Header link to `app.mp.fun` so users can move from "I just looked at my balance" → "let me bridge / claim / swap"

## Staccana-specific features (post-launch additions)

The vanilla explorer covers tx / account / block views. Staccana wants more:

1. **Lazy-claim status per pubkey** — given a pubkey, show:
   - Mainnet snapshot balance (computed from genesis Merkle root)
   - Whether claimed (`claimed` PDA exists)
   - If claimed, the claim transaction signature
   Easy add: a new tab on the account view that reads the lazy-claim program account + the per-pubkey marker PDA.

2. **Bridge state inspector** — given an asset (stSOL, ssUSDC):
   - Current `R_q64` (decoded from `RatioState` PDA per SPEC §5.2)
   - Last published slot + nonce
   - Mint supply
   - Federation signer set
   New page: `/bridge/<asset_id>`.

3. **Federation signer panel** — list the M-of-N signers, their pubkeys, last attestation slot per signer (degraded ones flagged).

4. **secret-pump bonding curve viewer** — given a pump token, show the curve state, live spot price, time to graduation, recent buys/sells.
   New page: `/pump/<mint>`.

5. **FBA matcher visualization** (post v1.1) — per-slot, show:
   - Number of swap intents
   - Clearing prices per (base, quote) pair
   - Matched volume vs residual volume
   - "MEV-eliminated" estimate (volume that would have been sandwich-able)
   This is the big one — proves the FBA is working and gives staccana a unique narrative artifact. Worth investing in once the FBA enforcement lands.

## Deploy

```bash
# Vercel
vercel --prod
# or
vercel link  # one-time
vercel deploy --prod
```

Domain: `explorer.mp.fun` → CNAME → Vercel deployment. Cloudflare proxies (orange cloud) for caching + DDoS.

Build: ~30s. Fits Vercel free tier easily (low traffic at launch); upgrade as needed.

## Maintenance

- **Upstream sync**: quarterly rebase against `solana-labs/explorer`, resolve conflicts in the cluster picker + branding files. Most upstream changes don't touch our staccana-specific code.
- **Security**: subscribe to upstream releases. Critical patches (XSS, etc.) — pull immediately.
- **Per-feature**: each staccana-specific addition (1-5 above) has its own page/component, easy to remove or refactor.

## v1 launch scope

Soft-launch ships with vanilla forked explorer + staccana cluster default + minimum branding. Staccana-specific tabs (#1, #2) add 1-2 days of frontend work each. The big FBA visualization (#5) is a v1.1+ project.

## v1.1 alternatives to consider

- **Custom explorer in Next.js + tRPC** — full control, but rebuilds 90% of what solana-explorer already does. Skip unless we have specific needs the fork can't satisfy.
- **Helius / SimpleHash for indexing backend** + custom UI — pays for indexing infra rather than running our own. May make sense if explorer traffic gets heavy.
- **A combined `app.mp.fun` + explorer** — merge the user-facing app and the explorer into one Next.js app. Cleaner UX (no jumping between domains) but larger surface to maintain. Defer this decision until post-launch traffic patterns are clearer.

# staccana frontend

Next.js 14 (App Router) workspace for the staccana web frontend, deployable to Vercel at `app.mp.fun`.

## Scope

v0 ships:

- `/` landing page (hero + 3 cards)
- `/claim` claim flow MVP — wallet connect, snapshot fetch, Merkle inclusion proof, ed25519-precompile + claim ix, RPC submit
- `/bridge` and `/pump` are clean TODO stubs pending v1.0 protocol go-live

The Merkle-tree TypeScript port in `lib/merkle.ts` is byte-equal to the Rust reference impl in `genesis/src/merkle.rs` and `tools/claim-cli/src/proof.rs`, verified by fixtures in `tests/merkle.test.ts`.

## Prerequisites

- Node 18.18+ (we tested on 24)
- npm 11+ (or pnpm/yarn — only npm is wired into `vercel.ts`)

## Install + run

```bash
cd frontend
cp .env.local.example .env.local        # edit if you want to override defaults
npm install
npm run dev
```

The dev server starts on http://localhost:3000. Defaults point at the production
RPC (`https://rpc.mp.fun/`) and snapshot endpoint (`https://snapshot.mp.fun/`)
so the claim flow works out of the box once those services are up.

## Test

```bash
npm test
```

Runs the `vitest` suite. The Merkle byte-equality assertions live in
`tests/merkle.test.ts` and pin canonical hex fixtures generated against the
Rust reference impl. To regenerate fixtures after a Rust-side change:

```bash
# from the staccana repo root
mkdir -p /tmp/merkle-fixtures/src
# copy the inline Rust harness from the scaffold report into /tmp/merkle-fixtures/
cd /tmp/merkle-fixtures && cargo run --release
# paste the new hex strings into frontend/tests/merkle.test.ts
```

## Type-check

```bash
npm run type-check
```

## Build

```bash
npm run build
npm run start
```

## Deploy to Vercel

The `vercel.ts` config picks up Next.js automatically. From the `frontend/`
directory:

```bash
vercel link              # first time only
vercel deploy            # preview
vercel deploy --prod     # production (app.mp.fun)
```

You must set the following Vercel env vars (via `vercel env add` or the
dashboard) before promoting to production. They all match
`.env.local.example`:

| Var                             | Purpose                                                   |
|---------------------------------|-----------------------------------------------------------|
| `NEXT_PUBLIC_RPC_URL`           | Staccana RPC endpoint                                     |
| `NEXT_PUBLIC_SNAPSHOT_URL`      | URL of the genesis snapshot JSON                          |
| `NEXT_PUBLIC_EXPLORER_URL`      | Block explorer base URL (used in success toasts)          |
| `NEXT_PUBLIC_CLUSTER_NAME`      | Display label in the cluster banner                       |
| `NEXT_PUBLIC_GENESIS_HASH`      | Optional pinned genesis hash for the cluster banner       |

DNS / TLS for `app.mp.fun` is provisioned via the Cloudflare API integration
in `infra/cloudflare/`.

## Architecture

- **App Router** with React Server Components by default. Only the wallet-aware
  pages (`/claim`) and the connect button are client components.
- **State management**: minimal — `useState` + the wallet-adapter hooks. No
  Redux, Zustand, or similar.
- **Caching**: snapshot is cached in IndexedDB via `idb-keyval` with a content
  key that includes the snapshot URL so republishes invalidate.
- **Styling**: Tailwind + shadcn/ui-style primitives. The wallet-adapter UI
  ships its own CSS bundle that we import in `app/globals.css`.

## Files

```
frontend/
├── app/
│   ├── layout.tsx          root layout: WalletContextProviders + ClusterBanner + Toaster
│   ├── page.tsx            landing
│   ├── claim/page.tsx      v0 MVP — claim flow
│   ├── bridge/page.tsx     TODO stub
│   ├── pump/page.tsx       TODO stub
│   └── globals.css         tailwind base + wallet-adapter styles
├── components/
│   ├── cluster-banner.tsx  staccana cluster info banner
│   ├── site-header.tsx     wordmark + nav + connect button
│   ├── theme-provider.tsx  stub (single dark theme today)
│   ├── wallet-button.tsx   wallet-adapter MultiButton wrapper
│   └── ui/                 shadcn-style primitives (Button, Card, Toast, ...)
├── lib/
│   ├── claim.ts            transaction construction (claim msg + ed25519 ix + claim ix)
│   ├── merkle.ts           SHA-256 Merkle root + inclusion proof, byte-equal to Rust
│   ├── snapshot.ts         snapshot fetch + IndexedDB cache + claimable partition
│   ├── staccana.ts         central config (RPC, program IDs, PDAs, env vars)
│   ├── utils.ts            cn + truncatePubkey + formatSol
│   └── wallet.ts           ConnectionProvider + WalletProvider + WalletModalProvider
├── tests/
│   └── merkle.test.ts      byte-equality fixtures vs. Rust reference impl
├── package.json
├── tsconfig.json
├── next.config.mjs
├── tailwind.config.ts
├── postcss.config.mjs
├── vercel.ts               Vercel framework config (typed)
├── vitest.config.ts
└── .env.local.example
```

## TODOs

- `LAZY_CLAIM_PROGRAM_ID` in `lib/staccana.ts` is the placeholder ASCII-string
  pubkey from the Rust reference. Swap for the real on-chain program ID at
  deploy time.
- `treasuryPda()` in `lib/staccana.ts` derives from the lazy-claim program (same
  shortcut the CLI uses); update once the real `TREASURY_PROGRAM_ID` is
  assigned in SPEC §2.1.
- `NEXT_PUBLIC_SNAPSHOT_URL` defaults to `https://snapshot.mp.fun/` — confirm
  this endpoint is live before going to prod, or override per-environment.
- Bridge + pump UIs are intentionally TODO stubs and land in a follow-up
  scaffold round.

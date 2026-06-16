# Staccana Infrastructure Plan

Deployment topology for `mainnet-sigma` (target 2026-05-16) and onward.

## Provider mix

- **Validators**: Cherryservers Bare Metal Cloud (AMD EPYC tier) — primary; Hetzner AX-line as redundancy / EU diversity.
- **RPC / entrypoints**: Cherryservers (geographic spread); offload to Helius / Triton later if traffic scales.
- **Federation-attestor daemons**: small VPS class on Cherryservers, co-located with non-validator infra.
- **Frontend / explorer / docs**: Vercel (matches the existing Vercel skill patterns in this repo).

## Validator nodes

Three boxes for liveness redundancy across regions.

| Region | Provider | Spec |
|---|---|---|
| US-West (Las Vegas) | Cherryservers | AMD EPYC 7003-series, 128GB RAM, 2× 2TB NVMe RAID-0, 10Gbps |
| EU (Frankfurt or Amsterdam) | Cherryservers | same |
| APAC (Singapore) | Cherryservers | same |

Hardware rationale:
- 128GB RAM is conservative for staccana's expected low launch TPS; mainnet-class Solana wants 256GB but our fork doesn't approach mainnet load. Headroom for snapshot loading, accounts-db cache, Turbine fanout.
- 2× 2TB NVMe RAID-0: ledger on one, accounts on the other; or RAID-0 for lower latency (we accept the rebuild risk because of the fork's lazy-claim-genesis posture — state regenerable from snapshot).
- 10Gbps unmetered: Turbine + RPC chew through bandwidth.

Approximate cost: ~$250/mo each → **$750/mo** for 3 validators.

## RPC / entrypoint nodes

Five boxes covering major user regions. RAM can drop to 64GB; bandwidth is what matters.

| Region | Provider | Spec |
|---|---|---|
| US-West (Las Vegas) | Cherryservers | AMD Ryzen 9 / EPYC, 64GB RAM, 2TB NVMe, 10Gbps |
| US-East (Chicago / NYC-equivalent) | Cherryservers | same |
| EU (Frankfurt) | Cherryservers | same |
| APAC (Singapore) | Cherryservers | same |
| South Asia (Mumbai) | Cherryservers | same |

Each RPC node:
- Runs `agave-validator --no-voting --rpc-port 8899` (RPC-only validator)
- Behind nginx for rate limiting + API key auth
- Restricted ingress at launch; opens up as confidence grows

Approximate cost: ~$150/mo each → **$750/mo** for 5 RPCs.

## Federation-attestor daemons

One per signer for the 5-of-9 federation. Daemon load is light (websocket subs + signing + occasional ix submission); 8GB RAM / 2 vCPU is plenty.

Approximate cost: ~$30/mo each (small VPS class) → **$150/mo** for 5 attestors.

## Frontend / explorer / docs

- Vercel for: claim UI, bridge UI, secret-pump UI, docs site, status page.
- Self-hosted (on one of the RPC boxes initially) or migrated to Vercel: forked solana-explorer pointed at staccana RPC.

Vercel cost: included in existing plan; no marginal expense.

## Total

```
Validators:    3 × $250  = $750/mo
RPCs:          5 × $150  = $750/mo
Attestors:     5 ×  $30  = $150/mo
Vercel/extras: ~          $50/mo
                          ─────────
                          $1,700/mo
```

Comfortably under $20k/year for full v1 infra.

## Geographic coverage

Validators: US-West / EU / APAC — three time zones covered, no single regional outage takes the chain down.

RPCs: 5 regions covering >90% of likely user latency. APAC is doubled (Singapore + Mumbai) to handle staccana's expected demographic skew (degen flow + Solana ecosystem affinity in Asia).

## Operational notes

- **Snapshot redundancy**: each validator periodically uploads snapshots to a shared S3-compatible bucket (Backblaze B2 or Cloudflare R2). Cheap insurance against ledger corruption.
- **Monitoring**: Grafana + Prometheus on a separate small VPS; alerts to Telegram / Discord; see also Vercel Observability for the frontends.
- **Bastion / SSH**: dedicated tiny bastion box; SSH from elsewhere disabled. Operations from approved IPs only.
- **Secrets**: validator keypairs in 1Password / Vault; federation signing keys held by individual signers, never on shared infra.
- **Backups**: daily Cargo.lock + chain config + genesis output (small, ~MB-scale) to a separate provider. Ledger isn't backed up — too big and regenerable from peer snapshots.

## Scaling triggers

When to add infrastructure beyond the v0 footprint:

| Signal | Action |
|---|---|
| RPC sustained > 80% CPU on any node | Add a 6th RPC in the busiest region |
| Bridge TVL > $5M | Move from team-controlled federation to a real 5-of-9 with independent signers |
| Validator stake distribution skews (one operator > 33% effective) | Recruit additional validators; consider stake-cap in genesis defaults |
| Public RPC abuse (single IP > X req/min sustained) | Tighter nginx rules; consider cloudflare in front |

## v1.1 infra additions

- **secret-ray** (forked Raydium AMM/CLMM/CPMM + router) — separate program, no infra change required beyond the 3 validators.
- **Validator subsidy distribution program** — runs on-chain, no off-chain infra.
- **Treasury productive position** (pSYRUP staking via the bridge) — bridge already covers this; no new infra.
- **Multi-validator decentralization** — recruit external validator operators, fund initial stake from treasury.

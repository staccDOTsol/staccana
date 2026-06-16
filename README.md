# Staccana

A Solana fork — secrecy on at genesis, atomic-MEV structurally impossible, treasury-funded ops, federated multi-asset bridge with a non-1:1 accruing peg.

Continuation of [solana-classic v1](docs/LINEAGE.md). Same repo, same docker image, sharper architecture.

**v1 mainnet codename: `mainnet-sigma`. Target: 2026-05-16.**

## What this is

Staccana is a sovereign Solana chain. Genesis is built from a snapshot of mainnet at slot `S`. Five things define it:

1. **Confidential transfer feature gates active at slot 0.** ZK ElGamal Proof program ships activated as a builtin (already a path-dep in classic v1's Cargo.toml). Token-22 mints can opt into the Confidential Transfer extension immediately, while mainnet's gates remain inactive on every public network (see `docs/ARCHITECTURE.md`).
2. **Per-mint frequent batch auction at the validator layer.** Within a slot, swap intents are grouped by their longtail (non-quote) mint, matched against each other in size order, and cleared at a single AMM-anchored price. Sandwich and Jito-bundle MEV are structurally impossible.
3. **No bundle protocol.** The validator binary doesn't compile in or honor Jito-style bundle messages.
4. **Strict genesis partition rule.** Only system-program-owned, zero-data accounts (raw SOL on plain wallets) are claimable via lazy-claim. Everything else — every PDA, every token account, every stake account, every program-owned anything — is zero'd at genesis and the lamports credited to the staccana treasury. **One rule, no allowlists, no per-protocol judgment calls.**
5. **Treasury funds the project, not inflation.** Inflation is disabled (inherited from classic v1). The genesis treasury — sized in the hundreds of millions of SOL given mainnet's stake distribution — funds ops, secret-pump bonding-curve seed liquidity, secret-ray initial pools, validator subsidies, and an insurance fund for the bridge.

## What this is not

- A privacy-by-default chain. Secrecy is opt-in per Token-22 extension semantics; the chain is transparent.
- An L2 / rollup. Staccana is its own L1 with its own consensus, validators, and genesis.
- A 1:1 mainnet replica. Only raw-SOL EOAs survive; protocols don't carry over.
- An inflationary chain. Validator rewards = fees only.

## Repo layout

```
.
├── matcher/                 # FBA library — the core consensus rule
├── genesis/                 # Snapshot ingest, partition, treasury, Merkle root, classic defaults
├── programs/                # Solana programs (lazy-claim, bridge, secret-*)        [planned]
├── tools/                   # CLIs and operator tooling
├── infra/                   # Ansible playbooks + bootstrap scripts + systemd units + Cloudflare LB
├── frontend/                # Next.js 14 + Vercel app (claim/bridge/pump UIs); deploys to app.mp.fun
├── agave/                   # Forked validator (git submodule of classic v2 branch)  [planned]
└── docs/
    ├── ARCHITECTURE.md      # Design overview
    ├── BRIDGE.md            # Multi-asset bridge with non-1:1 accruing peg
    ├── E2E_DEPLOY.md        # Local validator → multi-validator devnet → mainnet-sigma deploy pipeline
    ├── INFRA.md             # Cherryservers + Hetzner + Vercel infra plan (~$1.7k/mo)
    ├── LINEAGE.md           # Classic v1 → staccana v2 narrative
    ├── ROADMAP.md           # Phased ship plan
    ├── SECRET_RAY.md        # v1.1 forked-Raydium integration contract
    └── SPEC.md              # Normative wire formats, invariants, constants
```

## Run a validator

Staccana ships as a single multi-arch Docker image (`linux/amd64` + `linux/arm64`). It's `agave-validator` (solana-core 2.3 line) carrying the staccana confidential-transfer fixes, plus a genesis seed and a run script that wires entrypoint / identity / vote / stake on first boot.

### Minimum requirements

- Linux host (kernel ≥ 5.10 for io_uring; the image gracefully falls back on older kernels and WSL2)
- Docker (multi-arch manifest — pulls native arm64 on Apple Silicon / Graviton, native amd64 elsewhere)
- ~50 GB disk for ledger + accounts (grows; depends on `--limit-ledger-size`)
- Open inbound: TCP/UDP 8001 (gossip), TCP/UDP 8002–8027 (dynamic TVU/TPU range), TCP 8899 (RPC, optional)

### Cluster constants (`mainnet-sigma`, devnet phase)

| Field | Value |
|---|---|
| Genesis hash | `FFwiB5Dq3HshrfzPeQTCWAzVUFgw6r4kJLAmCYdLXLep` |
| Validator binary version | `agave-validator` (solana-core 2.3.x, branch `staccana-ct-fixes`) |
| Bootstrap entrypoint | `84.32.220.211:8001` (val-1, identity `BtTrfSMeHSNJc8cfy3AAXEykjGPEuTFzL53Vfp8dsUcb`) |
| Public RPC | `https://rpc.mp.fun` (or hit any of val-1/2/3/4 directly on `:8899`) |
| Docker image | `jrsdunn/solana-classic-validator:latest` |

### One-line bring-up

```bash
docker pull jrsdunn/solana-classic-validator:latest

docker run -d --name staccana \
  --restart unless-stopped \
  -p 8001:8001/tcp -p 8001:8001/udp \
  -p 8002-8027:8002-8027/udp \
  -p 8899:8899/tcp \
  -v staccana-ledger:/var/lib/staccana/ledger \
  -v staccana-accounts:/var/lib/staccana/accounts \
  -v staccana-keys:/etc/staccana/keys \
  -e STACCANA_PUBLIC_IP=$(curl -s ifconfig.me) \
  jrsdunn/solana-classic-validator:latest
```

The image now defaults `STACCANA_ENTRYPOINT`, `STACCANA_KNOWN_VALIDATOR`,
and `STACCANA_EXPECTED_GENESIS_HASH` to staccana mainnet-sigma values, so
you only need `-e STACCANA_PUBLIC_IP=…` for the gossip-advertise. To run
against a different cluster (your own private fork, etc.), override those
three env vars. To skip snapshot fetch and replay from the embedded
genesis (slow — hours/days, but no peer dependency), add
`-e STACCANA_NO_SNAPSHOT_FETCH=1`.

On first boot the entrypoint script generates fresh `identity.json`, `vote.json`, `stake.json` keypairs into the `staccana-keys` volume and seeds the ledger from the baked-in `genesis.bin` (no external tarball needed — it's inside the image). `docker logs -f staccana` to follow.

### Building the validator from source

The deployed binary is built from the `staccana-ct-fixes` branch on the fork — agave (solana-core 2.3 line) plus the staccana confidential-transfer fixes: the ZK ElGamal proof-program re-enable gate and the `percentage_with_cap` Fiat-Shamir transcript fix. Validated end-to-end on devnet-sigma-v2 (commit `df71095f16`). Build it with:

```bash
git clone https://github.com/staccDOTsol/agave staccana-agave
cd staccana-agave
git checkout staccana-ct-fixes  # agave 2.3 line + staccana confidential-transfer fixes
./cargo build --release          # ~25 min on a beefy box
```

The resulting `target/release/agave-validator` is byte-equivalent to what's running on val-1/2/3/4. Or use the convenience wrapper:

```bash
./infra/scripts/build-agave.sh    # in this (staccana) repo
```

Build from the `staccana-ct-fixes` branch (the validated source); set `STACCANA_AGAVE_BRANCH` to override if you maintain your own fork.

### Joining the validator subsidy

Once your node is in gossip and catching up, register its identity pubkey with the on-chain `validator-subsidy` program so it receives epoch payouts:

```bash
# from the operator (governance key) end:
staccana-subsidy-cli \
  --keypair /path/to/upgrade-authority.json \
  --rpc https://rpc.mp.fun \
  register-validator \
  --validator <your-identity-pubkey>
```

Distributions are pull-free — they land in the identity address whenever `distribute_yield` / `bootstrap_distribute` runs for the epoch (SPEC §7.2). Status visible at <https://app.mp.fun/validators>.

### Troubleshooting

| Symptom | Fix |
|---|---|
| `assertion failed: io_uring_supported()` | Pull a newer image (`:latest` past sha256 `9eb83b…`) — patched to fall back to `std::fs` on WSL2 / older kernels. |
| `Port range is too small` | Pull `:latest` past sha256 `9eb83b…` — bumped from 8002–8020 to 8002–8027 for agave 3.x's allocator. |
| Bootstrap stuck on snapshot fetch | Confirm RPC port is open on the entrypoint (`curl -s -X POST http://84.32.220.211:8899 -H content-type:application/json -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}'`). All four val-1/2/3/4 expose 8899; entrypoint connection alone is enough. |
| `failed to open elf at /lib64/ld-linux-x86-64.so.2` | You pulled an amd64 manifest on an arm64 host. Re-pull — `:latest` is a multi-arch manifest list, Docker should pick the right platform automatically. |

## Status

Pre-alpha scaffold with several live subsystems. The matcher/genesis pipeline,
lazy-claim, bridge/megadrop scaffolding, agent messaging codec, and agent-only
`MSG` faucet all compile in the workspace.

```bash
cargo test -p staccana-matcher
cargo test -p staccana-genesis
cargo test -p staccana-agent-messaging -p staccana-agent-mail -p staccana-agent-faucet
```

## Why

Solana's worst extractive surface is atomic sandwich MEV via Jito bundles. The cleanest fix is to remove the leader's ordering control entirely, batch-match within each slot, and clear at a uniform price. Solana mainnet won't adopt this — too much rent depends on it. So: fork from a snapshot, ship the fix, capture the slow-burn audience that wants Solana without the extractive layer.

Confidential transfers ride along because the ZK ElGamal Proof program's activation gate is **inactive on mainnet, devnet, and testnet** as of this writing — flipping it at staccana's genesis is days of work and gives us multi-year headroom on the secrecy axis alone.

Lineage: this is `solana-classic` v2. Classic v1 (May 2025) tried to deter MEV via a fixed-fee model — clever but blunt. Staccana v2 does it structurally. See `docs/LINEAGE.md`.

## License

Apache-2.0.

# v1 End-to-End Deploy Pipeline

Concrete plan for going from a green-cargo-check workspace to a live staccana chain on Cherryservers infrastructure. Targets `mainnet-sigma` (2026-05-16) for the soft-launch path; v1.1 for the full FBA-at-consensus path.

## Phase A: Local validator deploy (devnet equivalent on a laptop)

The Phase 0 → mainnet-sigma path. Exercises every program against a real BanksClient running our forked agave binary on a single Hetzner box.

### Step 1: Build artifacts

```bash
# Native programs (lazy-claim) — built with cargo build-sbf
cargo build-sbf --manifest-path programs/lazy-claim/Cargo.toml

# Anchor programs (secret-pump, megadrop) — built with anchor build
cd programs/secret-pump && anchor build && cd ../..
cd programs/megadrop && anchor build && cd ../..

# Validator binary — forked agave with classic v1 patches + matcher crate wired
cd agave && cargo build --release --bin agave-validator && cd ..
```

Artifacts produced:
- `target/deploy/staccana_lazy_claim.so`
- `programs/secret-pump/target/deploy/staccana_secret_pump.so`
- `agave/target/release/agave-validator`

### Step 2: Build genesis

```bash
# Pull mainnet snapshot at slot S (or use a known-recent snapshot)
solana-validator --no-voting --rpc-port 0 --ledger /tmp/snapshot-ledger \
  --known-validator <pubkey> --only-known-rpc \
  --snapshot-fetch-only

# Run snapshot-fork to produce GenesisOutput JSON
cargo run --release -p staccana-snapshot-fork -- \
  --snapshot /tmp/snapshot-ledger \
  --output /tmp/genesis-output.json \
  --format json --source solana

# Compose into a Solana genesis config
cargo run --release -p staccana-genesis-emit -- \
  --input /tmp/genesis-output.json \
  --output /tmp/staccana-genesis.json
```

The `genesis-emit` tool's v0 stub writes structured JSON; the agave-side wiring takes that JSON and converts to the actual `genesis.bin` via `solana-genesis` machinery. v1 work is to close that loop.

### Step 3: Boot validator

```bash
mkdir -p /var/lib/staccana
cd /var/lib/staccana

# Initialize ledger from the composed genesis
agave-validator-genesis \
  --bootstrap-validator <validator-pubkey> <vote-pubkey> <stake-pubkey> \
  --staccana-genesis /tmp/staccana-genesis.json \
  --ledger /var/lib/staccana/ledger

# Boot the validator
agave-validator \
  --identity /var/lib/staccana/validator-keypair.json \
  --vote-account /var/lib/staccana/vote-keypair.json \
  --ledger /var/lib/staccana/ledger \
  --rpc-port 8899 \
  --gossip-port 8001 \
  --no-poh-speed-test \
  --no-os-network-limits-test \
  --no-port-check \
  --log /var/log/staccana/validator.log
```

### Step 4: Deploy programs

```bash
solana config set --url http://localhost:8899

# Deploy each program at the well-known address from SPEC §2.1
solana program deploy \
  --program-id /var/lib/staccana/keys/lazy-claim-keypair.json \
  target/deploy/staccana_lazy_claim.so

solana program deploy \
  --program-id /var/lib/staccana/keys/secret-pump-keypair.json \
  programs/secret-pump/target/deploy/staccana_secret_pump.so
```

### Step 5: Initialize program state

```bash
# secret-pump: nothing to init upfront — first `create` ix bootstraps state (SOL-quoted curves)
# No bridge to initialize — value moves in/out via CEX listings.
```

### Step 6: Smoke test

```bash
# Claim flow: pick a known-claimable pubkey from genesis
cargo run --release -p staccana-claim-cli -- \
  --keypair ~/.config/solana/id.json \
  --snapshot /tmp/genesis-output.json \
  --rpc http://localhost:8899

# secret-pump: create a token, buy some, sell some
cargo run --release -p staccana-secret-pump-cli ... # (CLI not yet built — TODO)
```

## Phase B: Multi-validator devnet on Cherryservers

Spin up 3 validators per `docs/INFRA.md`. Same boot sequence per box, with these adjustments:

- All 3 use the SAME genesis config (built once, distributed via secure channel)
- Each has its own validator-keypair.json and vote-keypair.json
- One is the "bootstrap" validator; the other two `--known-validator <bootstrap-pubkey> --only-known-rpc` to find each other

### Networking

- Open UDP 8000-8020 (gossip + Turbine) between validators.
- Open TCP 8899 (RPC) only on the RPC nodes, restricted to API key auth at nginx.
- Validators do NOT expose RPC publicly.

### Stake distribution

Bootstrap validator gets 1 staked-SOL initial stake. Each additional validator gets stake from the treasury. Runtime subsidy is treasury principal drawdown per SPEC §7.2 (hand-distributed from the multisig at launch; no yield, no staking position).

## Phase C: mainnet-sigma cutover

Same boot sequence as Phase B but:
- Fresh genesis from a fresh mainnet snapshot at slot `S_launch`
- Distinct genesis hash from any prior staccana network
- CEX listing(s) lined up as the on/off-ramp (no bridge to deploy)
- Public RPC opens behind a Helius-style provider or our own load-balancer
- Frontend goes live (Vercel)
- Soft-launch announcement via the `solana-classic` Docker Hub channel + STACCoverflow

## What the v1 e2e harness needs to add

The current `e2e-tests/` crate uses `solana-program-test` (in-process BanksClient). v1 needs:

1. **Real validator harness**: spawns `agave-validator` as a subprocess with a synthetic genesis, deploys real `.so` artifacts, submits txs via real `solana-client`.
2. **Anchor program loading**: figure out the dep-conflict that prevents `processor!()` loading of secret-pump in the in-process test (likely fixed by the v1.1 Anchor 0.30 → 1.x upgrade).
3. **Multi-tx scenarios**: claim → secret-pump create → buy → graduate → swap on secret-ray, all in one test, asserting state at each step.

Estimate: 2-4 weeks of focused work post-mainnet-sigma.

## Operational runbook (post-deploy)

- **Monitoring**: Grafana dashboards for validator health (slot lag, vote distance, RPC latency), treasury drawdown balance, RPC latency.
- **Alerts**: Telegram/Discord for slot-lag > 32, vote-distance > 64, RPC error rate > 1%.
- **Rotation**: validator keypair rotation every 90 days.
- **Snapshot uploads**: every validator uploads a snapshot every epoch to a shared B2/R2 bucket; new validators bootstrap from these.
- **Emergency**: documented playbook for chain halt (single-validator network = single point of failure; multi-validator survives one death). Contact list, on-call rotation, recovery snapshot location.

## Out of scope (intentional, beyond v1)

- Public RPC SLA — not committed at v1
- 24/7 on-call — best-effort response at v1
- Slashing program enforcement — design'd in SPEC, implementation lands with the Phase 2 agave fork (v1.1+)
- Validator marketplace / staking dashboard — community-built later

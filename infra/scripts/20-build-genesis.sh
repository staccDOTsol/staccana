#!/usr/bin/env bash
# 20-build-genesis.sh — snapshot → GenesisOutput → ComposedGenesis → genesis.bin
#
# Ties together staccana-snapshot-fork + staccana-genesis-emit, then hands off to the
# (forked) agave's genesis-config tooling.

set -euo pipefail

STACCANA_DIR="${STACCANA_DIR:-/opt/staccana}"  # path to this repo on the box
SNAPSHOT_DIR="${SNAPSHOT_DIR:-/var/lib/staccana/snapshot-cache}"
GENESIS_DIR="${GENESIS_DIR:-/var/lib/staccana/genesis}"

LATEST_SNAPSHOT=$(cat "$SNAPSHOT_DIR/.snapshot-path")
LATEST_SLOT=$(cat "$SNAPSHOT_DIR/.snapshot-slot")

mkdir -p "$GENESIS_DIR"

echo "[genesis] partitioning snapshot at slot $LATEST_SLOT"

# Step 1: snapshot-fork walks the snapshot, applies the partition rule, writes
# GenesisOutput JSON (claimable Merkle root + treasury total + classic defaults).
cargo run --release \
  --manifest-path "$STACCANA_DIR/tools/snapshot-fork/Cargo.toml" \
  -- \
  --snapshot "$LATEST_SNAPSHOT" \
  --output "$GENESIS_DIR/genesis-output.json" \
  --format json \
  --source solana

# Step 2: genesis-emit composes the GenesisOutput into a ComposedGenesis JSON
# (fee governor, inflation, CTE feature gate set, treasury PDA pre-credit, lazy-claim
# config holding the embedded root).
cargo run --release \
  --manifest-path "$STACCANA_DIR/tools/genesis-emit/Cargo.toml" \
  -- \
  --input "$GENESIS_DIR/genesis-output.json" \
  --output "$GENESIS_DIR/composed-genesis.json"

# Step 3 (TODO — v0 stub): convert ComposedGenesis JSON → actual Solana genesis.bin
# using the agave fork's genesis-config tooling. Until that's implemented, the
# integrator runs the agave-validator-genesis subcommand manually with the values
# from composed-genesis.json. See docs/E2E_DEPLOY.md Step 2.
echo "[genesis] composed: $GENESIS_DIR/composed-genesis.json"
echo "[genesis] TODO(v0): finalize as genesis.bin via agave-validator-genesis ..."
echo "[genesis] done. next: ./30-init-validator.sh"

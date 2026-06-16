#!/usr/bin/env bash
# 10-pull-snapshot.sh — fetch a recent Solana mainnet snapshot.
#
# `agave-validator --snapshot-fetch-only` was removed in 2.0.25, so we use the community
# `solana-snapshot-finder` (c29r3/solana-snapshot-finder on GitHub — not published on
# PyPI). It discovers a working mainnet mirror via gossip + downloads from it.
#
# Mainnet snapshots are ~200GB. On Cherryservers' 1Gbps+ link figure 30 min to a couple
# hours wall-clock for the actual download.

set -euo pipefail

SNAPSHOT_DIR="${SNAPSHOT_DIR:-/var/lib/staccana/snapshot-cache}"
MAX_SNAPSHOT_AGE="${MAX_SNAPSHOT_AGE:-1500}"  # slots; ~10 minutes at 400ms slot time
FINDER_DIR="${FINDER_DIR:-/opt/snapshot-finder}"

mkdir -p "$SNAPSHOT_DIR"

# Install solana-snapshot-finder from GitHub (not on PyPI).
if [[ ! -f "$FINDER_DIR/snapshot-finder.py" ]]; then
  echo "[snapshot] installing solana-snapshot-finder from github"
  apt-get install -y --no-install-recommends git python3-pip python3-venv >/dev/null
  rm -rf "$FINDER_DIR"
  git clone --depth 1 https://github.com/staccDOTsol/solana-snapshot-finder.git "$FINDER_DIR"
  pip install --break-system-packages -r "$FINDER_DIR/requirements.txt" >/dev/null
fi

echo "[snapshot] starting fetch (max age ${MAX_SNAPSHOT_AGE} slots)"
python3 "$FINDER_DIR/snapshot-finder.py" \
  --snapshot_path "$SNAPSHOT_DIR" \
  --max_snapshot_age "$MAX_SNAPSHOT_AGE"

# Snapshot lives at $SNAPSHOT_DIR/snapshot-XXXXXXXX-*.tar.zst
LATEST_SNAPSHOT=$(ls -1 "$SNAPSHOT_DIR"/snapshot-*-*.tar.zst 2>/dev/null | sort -V | tail -1)
if [[ -z "$LATEST_SNAPSHOT" ]]; then
  echo "[snapshot] FATAL: snapshot-finder did not produce a snapshot file" >&2
  exit 1
fi

LATEST_SLOT=$(basename "$LATEST_SNAPSHOT" | sed -E 's/snapshot-([0-9]+)-.*/\1/')

echo "[snapshot] downloaded $LATEST_SNAPSHOT (slot $LATEST_SLOT)"
echo "$LATEST_SLOT" > "$SNAPSHOT_DIR/.snapshot-slot"
echo "$LATEST_SNAPSHOT" > "$SNAPSHOT_DIR/.snapshot-path"
echo "[snapshot] done. next: ./20-build-genesis.sh"

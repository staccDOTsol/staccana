#!/usr/bin/env bash
# build-and-push.sh — assemble + publish jrsdunn/solana-classic-validator:v2.x
#
# Run this from val-1 (where the freshly-baked ledger lives + agave-validator
# binary is already at /usr/local/bin). The script materializes a tiny build
# context (just the binaries + ledger seed + run script — no full source tree)
# and hands it to `docker build` so we don't ship 50GB of cargo target dirs
# inside the image.
#
# Required env:
#   DOCKER_USERNAME    Docker Hub user (default: jrsdunn)
#   (run `docker login -u "$DOCKER_USERNAME"` once before invoking — token
#    cached in /root/.docker/config.json from then on)
#
# Optional env:
#   IMAGE_REPO         Docker Hub repo  (default: jrsdunn/solana-classic-validator)
#   VERSION_TAG        version tag      (default: v2.0.0-devnet-YYYYMMDD)
#   PLATFORM           buildx platforms (default: linux/amd64; add ,linux/arm64
#                      for multi-arch — needs qemu emulation set up)
#   PUSH               1 = docker push, 0 = local build only (default: 1)
#   DOCKERFILE         Dockerfile to use (default: ./Dockerfile). Use
#                      ../docker/Dockerfile.multiarch for the multi-stage
#                      source-build that auto-matches glibc on the runtime
#                      and avoids the GLIBC_2.38-not-found class of bugs.
#                      That path requires AGAVE_SRC pointing at an agave
#                      checkout to be copied into the build context as
#                      ./agave-src/.
#   AGAVE_SRC          Path to agave source tree (only when using the
#                      multiarch Dockerfile). Default: /usr/src/agave.

set -euo pipefail

DOCKER_USERNAME="${DOCKER_USERNAME:-jrsdunn}"
IMAGE_REPO="${IMAGE_REPO:-jrsdunn/solana-classic-validator}"
VERSION_TAG="${VERSION_TAG:-v2.0.0-devnet-$(date -u +%Y%m%d)}"
# Accept either `PLATFORM` (singular, original name) or `PLATFORMS` (plural,
# the more natural name for a comma-separated list — and the env var name
# `docker buildx` itself uses). Plural wins if both are set so a CI job that
# exports PLATFORMS=... gets the expected behavior even if a stale PLATFORM
# is left in the env from a prior step.
#
# Default depends on the Dockerfile chosen below:
#   Dockerfile (single-arch, pre-built binaries) → linux/amd64 only
#   Dockerfile.multiarch (in-image build)         → linux/amd64,linux/arm64
# This way `./build-and-push.sh DOCKERFILE=…/Dockerfile.multiarch` produces a
# real multi-arch manifest list by default — which is the whole point of the
# multiarch Dockerfile. If you want a single arch with .multiarch (e.g. for a
# faster smoke build), set PLATFORM=linux/amd64 explicitly.
PUSH="${PUSH:-1}"

LEDGER_SRC="${LEDGER_SRC:-/var/lib/staccana/ledger}"
PROGRAM_IDS="${PROGRAM_IDS:-/etc/staccana/program-ids.json}"
BIN_DIR="${BIN_DIR:-/usr/local/bin}"

DOCKER_DIR="$(cd "$(dirname "$0")" && pwd)"
DOCKERFILE="${DOCKERFILE:-$DOCKER_DIR/Dockerfile}"
AGAVE_SRC="${AGAVE_SRC:-/usr/src/agave}"
CTX_DIR="$(mktemp -d -t staccana-docker-ctx-XXXXXX)"
trap 'rm -rf "$CTX_DIR"' EXIT

# Detect if we're using the multiarch (in-image build) Dockerfile — it expects
# ./agave-src/ in the build context instead of pre-built bin/* binaries.
USE_MULTIARCH=0
if [[ "$(basename "$DOCKERFILE")" == "Dockerfile.multiarch" ]]; then
  USE_MULTIARCH=1
fi

# Pick the default platform list now that USE_MULTIARCH is known. An explicit
# PLATFORM/PLATFORMS env var still wins.
if [[ "$USE_MULTIARCH" == "1" ]]; then
  DEFAULT_PLATFORM="linux/amd64,linux/arm64"
else
  DEFAULT_PLATFORM="linux/amd64"
fi
PLATFORM="${PLATFORMS:-${PLATFORM:-$DEFAULT_PLATFORM}}"

echo "[docker] assembling build context at $CTX_DIR"
echo "[docker] using Dockerfile: $DOCKERFILE (multiarch=$USE_MULTIARCH)"
mkdir -p "$CTX_DIR/ledger"

if [[ "$USE_MULTIARCH" == "1" ]]; then
  # 1. agave source tree (in-image build means we ship source, not binaries)
  if [[ ! -d "$AGAVE_SRC" ]]; then
    echo "[docker] FATAL: AGAVE_SRC=$AGAVE_SRC is not a directory" >&2
    exit 1
  fi
  # Copy with --reflink=auto where supported; fall back to a plain cp.
  cp -a "$AGAVE_SRC" "$CTX_DIR/agave-src" 2>/dev/null || \
    cp -r "$AGAVE_SRC" "$CTX_DIR/agave-src"
  # Drop target/ if present — saves dozens of GB in the build context.
  rm -rf "$CTX_DIR/agave-src/target" 2>/dev/null || true
else
  mkdir -p "$CTX_DIR/bin"
  # 1. agave-validator binaries (pre-built on this host — runtime base image
  #    must provide glibc >= this host's glibc; see docker/Dockerfile header)
  for b in agave-validator agave-ledger-tool solana solana-keygen; do
    if [[ ! -x "$BIN_DIR/$b" ]]; then
      echo "[docker] FATAL: $BIN_DIR/$b not found or not executable" >&2
      exit 1
    fi
    cp -p "$BIN_DIR/$b" "$CTX_DIR/bin/$b"
  done
fi

# 2. Ledger seed.
#
# `genesis.bin` is small + arch-independent — always ship it.
#
# `rocksdb/` is large + LIVE on val-1 (the running validator rotates SST files
# every few seconds), so a naive `cp -r` races the writer and bails with
# `cp: cannot stat '.../001115.sst': No such file or directory`. Three modes:
#
#   INCLUDE_ROCKSDB=skip      (default): ship genesis.bin only. The container
#                              boots from genesis at first run, catches up via
#                              the gossip network, and has a fresh ledger.
#                              Slower first boot but builds reproducibly.
#   INCLUDE_ROCKSDB=snapshot:  run `agave-ledger-tool create-snapshot` first
#                              to produce a point-in-time tarball, then ship
#                              just the tarball. Validator stays running. Best
#                              for offline/airgapped distribution.
#   INCLUDE_ROCKSDB=live:      old behavior — `cp -r` the live rocksdb dir.
#                              ONLY works when the validator is stopped.
#                              Will likely fail on a running val-1.
INCLUDE_ROCKSDB="${INCLUDE_ROCKSDB:-skip}"

if [[ ! -f "$LEDGER_SRC/genesis.bin" ]]; then
  echo "[docker] FATAL: $LEDGER_SRC/genesis.bin not found — run step 30 first" >&2
  exit 1
fi
cp "$LEDGER_SRC/genesis.bin" "$CTX_DIR/ledger/genesis.bin"

case "$INCLUDE_ROCKSDB" in
  skip)
    echo "[docker] INCLUDE_ROCKSDB=skip — shipping genesis.bin only (container boots from genesis + catches up over gossip)"
    ;;
  snapshot)
    echo "[docker] INCLUDE_ROCKSDB=snapshot — creating point-in-time snapshot…"
    if ! command -v agave-ledger-tool >/dev/null 2>&1; then
      echo "[docker] FATAL: agave-ledger-tool not on PATH (needed for INCLUDE_ROCKSDB=snapshot)" >&2
      exit 1
    fi
    SNAP_DIR=$(mktemp -d -t staccana-snap-XXXXXX)
    trap 'rm -rf "$SNAP_DIR"' EXIT
    SNAP_SLOT=$(agave-ledger-tool --ledger "$LEDGER_SRC" slot 2>/dev/null | tail -1)
    if [[ -z "$SNAP_SLOT" ]]; then
      echo "[docker] FATAL: couldn't read root slot from ledger" >&2
      exit 1
    fi
    agave-ledger-tool --ledger "$LEDGER_SRC" \
      create-snapshot "$SNAP_SLOT" "$SNAP_DIR" \
      --snapshot-archive-format zstd >&2
    cp "$SNAP_DIR"/snapshot-*.tar.zst "$CTX_DIR/ledger/" 2>/dev/null
    echo "[docker] snapshot at slot $SNAP_SLOT bundled into context"
    ;;
  live)
    echo "[docker] INCLUDE_ROCKSDB=live — copying live rocksdb (validator must be stopped)"
    if [[ -d "$LEDGER_SRC/rocksdb" ]]; then
      cp -r "$LEDGER_SRC/rocksdb" "$CTX_DIR/ledger/rocksdb"
    fi
    ;;
  *)
    echo "[docker] FATAL: unknown INCLUDE_ROCKSDB=$INCLUDE_ROCKSDB (expected skip|snapshot|live)" >&2
    exit 1
    ;;
esac

# 3. program-ids.json
if [[ -f "$PROGRAM_IDS" ]]; then
  cp "$PROGRAM_IDS" "$CTX_DIR/program-ids.json"
else
  echo '{"_warning":"program-ids.json missing at build time"}' > "$CTX_DIR/program-ids.json"
fi

# 4. Dockerfile + run script
cp "$DOCKERFILE" "$CTX_DIR/Dockerfile"
cp "$DOCKER_DIR/staccana-run.sh" "$CTX_DIR/staccana-run.sh"

CTX_SIZE=$(du -sh "$CTX_DIR" | cut -f1)
echo "[docker] build context size: $CTX_SIZE"
echo "[docker] image: $IMAGE_REPO:$VERSION_TAG"
echo "[docker] image: $IMAGE_REPO:latest"
echo "[docker] platform: $PLATFORM"

# Use buildx if multi-arch was requested OR push=1 (buildx handles --push natively)
USE_BUILDX=0
if [[ "$PLATFORM" == *","* || "$PUSH" == "1" ]]; then
  USE_BUILDX=1
fi

if [[ "$USE_BUILDX" == "1" ]]; then
  if ! docker buildx inspect staccana-builder >/dev/null 2>&1; then
    docker buildx create --name staccana-builder --driver docker-container --use
    docker buildx inspect --bootstrap
  else
    docker buildx use staccana-builder
  fi
  PUSH_FLAG=""
  [[ "$PUSH" == "1" ]] && PUSH_FLAG="--push"
  docker buildx build \
    --platform "$PLATFORM" \
    -t "$IMAGE_REPO:$VERSION_TAG" \
    -t "$IMAGE_REPO:latest" \
    $PUSH_FLAG \
    "$CTX_DIR"
else
  docker build \
    -t "$IMAGE_REPO:$VERSION_TAG" \
    -t "$IMAGE_REPO:latest" \
    "$CTX_DIR"
fi

echo "[docker] done."
[[ "$PUSH" == "1" ]] && echo "[docker] pushed: https://hub.docker.com/r/${IMAGE_REPO}/tags"

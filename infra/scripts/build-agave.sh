#!/usr/bin/env bash
# build-agave.sh — clone + build the patched staccana validator binary.
#
# What it does:
#   1. Clones (or fast-forwards) the staccDOTsol/agave fork at ./agave-src
#   2. Checks out the staccana-ct-fixes branch
#   3. Runs `cargo build --release` to produce target/release/agave-validator
#
# What's in the staccana-ct-fixes branch vs. its agave base (solana-core 2.3.0 line):
#   - zk-sdk percentage_with_cap: append c_max_proof to the Fiat-Shamir transcript
#     (confidential-transfer-with-fee soundness).
#   - feature-set / svm-feature-set / programs/zk-elgamal-proof: declare + honor the
#     disable/reenable_zk_elgamal_proof_program gates so the proof program runs when
#     both are active (staccana full-feature genesis). Validated on devnet-sigma-v2.
#
# Why a separate branch and not a patch file:
#   The fork keeps `git log` clean for downstream contributors (Ooze, etc.)
#   and lets cargo's lockfile pin to a specific revision in case we ever
#   want to vendor agave back into this repo as a submodule.
#
# Output:
#   ./agave-src/target/release/agave-validator  — the binary
#
# Re-run-safe. The git fetch is incremental; cargo build is incremental.

set -euo pipefail

REPO_URL="${STACCANA_AGAVE_REPO:-https://github.com/staccDOTsol/agave}"
BRANCH="${STACCANA_AGAVE_BRANCH:-staccana-ct-fixes}"
DEST_DIR="${STACCANA_AGAVE_DIR:-$(pwd)/agave-src}"

echo "[build-agave] $(date -Iseconds) starting"
echo "[build-agave] repo:   $REPO_URL"
echo "[build-agave] branch: $BRANCH"
echo "[build-agave] dest:   $DEST_DIR"

if [[ ! -d "$DEST_DIR/.git" ]]; then
  echo "[build-agave] cloning fresh"
  git clone --branch "$BRANCH" --depth 1 "$REPO_URL" "$DEST_DIR"
else
  echo "[build-agave] updating existing checkout"
  cd "$DEST_DIR"
  git fetch --depth 1 origin "$BRANCH"
  git checkout "$BRANCH"
  git reset --hard "origin/$BRANCH"
  cd - >/dev/null
fi

cd "$DEST_DIR"

# Sanity check — confirm the staccana confidential-transfer fixes are present,
# so we fail loudly if the wrong branch is checked out.
if ! grep -q 'append_scalar(b"c_max_proof"' zk-sdk/src/sigma_proofs/percentage_with_cap.rs 2>/dev/null; then
  echo "[build-agave] ERROR: percentage_with_cap c_max_proof transcript fix missing."
  echo "             The wrong branch is checked out — expected staccana-ct-fixes."
  echo "             git status:"
  git status --short
  exit 1
fi

echo "[build-agave] cargo build --release (this takes a while; ~20-25 min cold)"
cargo build --release --bin agave-validator

echo "[build-agave] done"
echo "[build-agave] binary: $DEST_DIR/target/release/agave-validator"
"$DEST_DIR/target/release/agave-validator" --version

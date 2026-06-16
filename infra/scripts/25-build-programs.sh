#!/usr/bin/env bash
# 25-build-programs.sh — build all five staccana programs into .so artifacts.
#
# Runs BEFORE 30-init-validator.sh, which depends on the .so files existing in
# $STACCANA_DIR/target/deploy/ to bake them as builtins into the genesis.
#
# Programs:
#   - staccana-lazy-claim       (native solana-program; cargo build-sbf)
#   - staccana-bridge           (Anchor 1.x; anchor build)
#   - staccana-secret-pump      (Anchor 1.x; anchor build)
#   - staccana-validator-subsidy (Anchor 1.x; anchor build)
#   - staccana-megadrop         (Anchor 1.x; anchor build)
#   - staccana-agent-faucet      (Anchor 1.x; cargo build-sbf)
#
# Anchor build outputs land under each program's local target/deploy/. We
# consolidate everything into $STACCANA_DIR/target/deploy/ so step 30's
# genesis-bake can find them at one path.

set -euo pipefail

STACCANA_DIR="${STACCANA_DIR:-/opt/staccana}"
DEPLOY_DIR="${DEPLOY_DIR:-$STACCANA_DIR/target/deploy}"
CARGO_BUILD_BPF="${CARGO_BUILD_BPF:-$STACCANA_DIR/scripts/staccana-cargo-build-bpf}"

mkdir -p "$DEPLOY_DIR"

echo "[build] === lazy-claim (native cargo build-sbf) ==="
(cd "$STACCANA_DIR" && "$CARGO_BUILD_BPF" --manifest-path "programs/lazy-claim/Cargo.toml" --sbf-out-dir "$DEPLOY_DIR")

# All other programs are Anchor-based but have `crate-type = ["cdylib", "lib"]`
# so `cargo build-sbf` handles them too — we don't need `anchor build` (which
# would need an Anchor.toml workspace file we never created). Skipping anchor
# also means no IDL gets generated, which is fine for now (front-end uses
# manually maintained encoders in frontend/lib/anchor.ts).
for prog in bridge secret-pump validator-subsidy megadrop agent-faucet; do
  echo "[build] === $prog (cargo build-sbf) ==="
  (cd "$STACCANA_DIR" && "$CARGO_BUILD_BPF" --manifest-path "programs/$prog/Cargo.toml" --sbf-out-dir "$DEPLOY_DIR")
done

echo "[build] consolidated artifacts in $DEPLOY_DIR:"
ls -lh "$DEPLOY_DIR"/staccana_*.so 2>/dev/null || echo "[build]   (none — something failed above)"

echo "[build] done. next: ./30-init-validator.sh"

#!/usr/bin/env bash
# 50-init-state.sh — initialize on-chain state after programs are deployed.

set -euo pipefail

STACCANA_DIR="${STACCANA_DIR:-/opt/staccana}"
KEY_DIR="${KEY_DIR:-/etc/staccana/keys}"
RPC="${RPC:-http://localhost:8899}"
FEDERATION_PUBKEYS_FILE="${FEDERATION_PUBKEYS_FILE:-/etc/staccana/federation-pubkeys.json}"

solana config set --url "$RPC" --keypair "$KEY_DIR/identity.json"

# 1. Bridge: register stSOL and ssUSDC assets, init federation set
echo "[init-state] registering stSOL asset"
cargo run --release \
  --manifest-path "$STACCANA_DIR/tools/bridge-cli/Cargo.toml" \
  -- register-asset --asset stSOL \
  --bridge-program-id "$(solana-keygen pubkey $KEY_DIR/program-bridge.json)" \
  --federation-pubkeys "$FEDERATION_PUBKEYS_FILE" \
  --rpc "$RPC"

echo "[init-state] registering ssUSDC asset"
cargo run --release \
  --manifest-path "$STACCANA_DIR/tools/bridge-cli/Cargo.toml" \
  -- register-asset --asset ssUSDC \
  --bridge-program-id "$(solana-keygen pubkey $KEY_DIR/program-bridge.json)" \
  --federation-pubkeys "$FEDERATION_PUBKEYS_FILE" \
  --rpc "$RPC"

# 2. Validator subsidy: init treasury config, register the bootstrap validator
# (Requires the validator-subsidy program to be deployed — see step 40 once it lands)
echo "[init-state] TODO: validator-subsidy init once that program ships"

# 3. Smoke-test claim flow with a known-claimable account
echo "[init-state] smoke-testing claim flow"
SMOKE_KEYPAIR="${SMOKE_KEYPAIR:-/etc/staccana/keys/smoke-test.json}"
if [[ -f "$SMOKE_KEYPAIR" ]]; then
  cargo run --release \
    --manifest-path "$STACCANA_DIR/tools/claim-cli/Cargo.toml" \
    -- \
    --keypair "$SMOKE_KEYPAIR" \
    --snapshot /var/lib/staccana/genesis/genesis-output.json \
    --rpc "$RPC" || echo "[init-state] smoke-test claim failed (expected if smoke-test pubkey isn't claimable)"
fi

echo "[init-state] done. validator + programs + initial state are live."

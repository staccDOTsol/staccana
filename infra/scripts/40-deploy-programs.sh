#!/usr/bin/env bash
# 40-deploy-programs.sh — deploy all staccana programs after the chain is live.
#
# Prereqs:
#   - Chain alive (RPC responding, slots rooting)
#   - Step 25 has built all .so files into $STACCANA_DIR/target/deploy/
#   - Identity keypair at $KEY_DIR/identity.json has SOL to pay deploy fees
#     (genesis-bake gives identity 1 SOL — should be plenty for 5 deploys at
#     ~few thousand lamports each, but we top up to 100 SOL via airdrop just
#     in case the deploy is bigger than expected)
#
# Output: /etc/staccana/program-ids.json with the deployed program IDs.

set -euo pipefail

STACCANA_DIR="${STACCANA_DIR:-/opt/staccana}"
KEY_DIR="${KEY_DIR:-/etc/staccana/keys}"
DEPLOY_DIR="${DEPLOY_DIR:-$STACCANA_DIR/target/deploy}"
RPC="${RPC:-http://localhost:8899}"

# Configure solana CLI to talk to local validator with identity as fee payer
solana config set --url "$RPC" --keypair "$KEY_DIR/identity.json" >/dev/null

echo "[deploy] === confirming chain is alive ==="
solana epoch-info | head -4

echo ""
echo "[deploy] === fee payer balance ==="
solana balance

# Generate program-id keypairs if missing. These addresses become the
# well-known PROGRAM_IDs that the frontend + on-chain references will use.
# Mainnet-sigma should swap these for grinded vanity keypairs (Bridge111...
# etc) before launch; for tonight's devnet, randoms are fine.
echo ""
echo "[deploy] === ensuring program-id keypairs exist ==="
for p in lazy-claim bridge secret-pump validator-subsidy megadrop agent-faucet; do
  if [[ ! -f "$KEY_DIR/program-$p.json" ]]; then
    solana-keygen new --no-passphrase --silent --outfile "$KEY_DIR/program-$p.json"
  fi
  echo "  $p → $(solana-keygen pubkey $KEY_DIR/program-$p.json)"
done

# Map program-name → expected .so filename (from step 25's consolidation)
declare -A SO_FILES=(
  [lazy-claim]="staccana_lazy_claim.so"
  [bridge]="staccana_bridge.so"
  [secret-pump]="staccana_secret_pump.so"
  [validator-subsidy]="staccana_validator_subsidy.so"
  [megadrop]="staccana_megadrop.so"
  [agent-faucet]="staccana_agent_faucet.so"
)

# Deploy each program, capturing the program ID on success
declare -A DEPLOYED_IDS
echo ""
echo "[deploy] === deploying programs ==="
for p in lazy-claim bridge secret-pump validator-subsidy megadrop agent-faucet; do
  so_path="$DEPLOY_DIR/${SO_FILES[$p]}"
  if [[ ! -f "$so_path" ]]; then
    echo "  $p: SKIPPED (.so not found at $so_path — run step 25 first)" >&2
    continue
  fi
  echo ""
  echo "[deploy] -- $p ($so_path) --"
  if solana program deploy \
       --program-id "$KEY_DIR/program-$p.json" \
       "$so_path"; then
    pid=$(solana-keygen pubkey "$KEY_DIR/program-$p.json")
    DEPLOYED_IDS[$p]=$pid
    echo "  $p deployed → $pid"
  else
    echo "  $p: DEPLOY FAILED" >&2
  fi
done

# Write program-ids.json for the frontend / state-init scripts to consume
mkdir -p /etc/staccana
{
  echo "{"
  first=1
  for p in lazy-claim bridge secret-pump validator-subsidy megadrop agent-faucet; do
    if [[ -n "${DEPLOYED_IDS[$p]:-}" ]]; then
      if [[ $first -eq 0 ]]; then echo ","; fi
      printf '  "%s": "%s"' "$p" "${DEPLOYED_IDS[$p]}"
      first=0
    fi
  done
  echo ""
  echo "}"
} > /etc/staccana/program-ids.json

echo ""
echo "[deploy] === SUMMARY ==="
cat /etc/staccana/program-ids.json
echo ""
echo "[deploy] wrote /etc/staccana/program-ids.json"
echo "[deploy] next: ./50-init-state.sh"

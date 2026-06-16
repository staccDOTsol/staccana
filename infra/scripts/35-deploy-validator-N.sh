#!/usr/bin/env bash
# 35-deploy-validator-N.sh — push the baked ledger + keys-N to validator-N's box.
#
# Generic version of the validator-2 deploy script. Run on validator-1's box
# AFTER step 30 has baked the genesis with EXTRA_VALIDATORS=N-1 (so keys-2..N
# exist).
#
# Usage:
#   ./35-deploy-validator-N.sh <N> <host> [user] [ssh_port]
#
# Examples:
#   ./35-deploy-validator-N.sh 2 84.32.220.76
#   ./35-deploy-validator-N.sh 3 84.32.220.99
#   ./35-deploy-validator-N.sh 4 84.32.220.123 root 22
#
# Each invocation:
#   - rsyncs /var/lib/staccana/ledger/ to the target box (same baked genesis)
#   - rsyncs /etc/staccana/keys-N/ to the target box
#   - rsyncs /etc/staccana/bank-hash to the target box
#   - generates a per-box systemd unit on-the-fly (with the right keys path,
#     gossip-host, entrypoint = val-1)
#   - daemon-reload + restart on the target box

set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: $0 <validator-number> <host> [user] [ssh_port]" >&2
  echo "  e.g. $0 2 84.32.220.76" >&2
  exit 2
fi

N="$1"
TARGET_HOST="$2"
TARGET_USER="${3:-root}"
TARGET_SSH_PORT="${4:-22}"

KEY_DIR="${KEY_DIR:-/etc/staccana/keys}"
LEDGER_DIR="${LEDGER_DIR:-/var/lib/staccana/ledger}"
STACCANA_DIR="${STACCANA_DIR:-/opt/staccana}"
# val-1's gossip endpoint (entrypoint for joining validators). Override if val-1
# isn't on this box.
VAL1_GOSSIP="${VAL1_GOSSIP:-84.32.220.211:8001}"

KEYS_N_DIR="${KEY_DIR%/keys}/keys-${N}"

if [[ "$N" -lt 2 ]]; then
  echo "[deploy] FATAL: N must be ≥ 2 (val-1 is the bootstrap, runs locally)" >&2
  exit 1
fi
if [[ ! -d "$KEYS_N_DIR" ]]; then
  echo "[deploy] FATAL: $KEYS_N_DIR not found — re-run step 30 with EXTRA_VALIDATORS=$((N-1)) (or higher)" >&2
  exit 1
fi
if [[ ! -f "$LEDGER_DIR/genesis.bin" ]]; then
  echo "[deploy] FATAL: $LEDGER_DIR/genesis.bin not found — run step 30 first" >&2
  exit 1
fi

TARGET_SSH="ssh -p $TARGET_SSH_PORT $TARGET_USER@$TARGET_HOST"
TARGET_RSYNC_RSH="ssh -p $TARGET_SSH_PORT"

echo "[deploy] === validator-$N → $TARGET_USER@$TARGET_HOST:$TARGET_SSH_PORT ==="

# 1. Push ledger
echo "[deploy] pushing ledger"
rsync -avz --delete -e "$TARGET_RSYNC_RSH" \
  "$LEDGER_DIR/" \
  "$TARGET_USER@$TARGET_HOST:$LEDGER_DIR/"

# 2. Push keys-N
echo "[deploy] pushing keys-$N"
$TARGET_SSH "mkdir -p $KEYS_N_DIR && chmod 700 $KEYS_N_DIR"
rsync -avz --delete -e "$TARGET_RSYNC_RSH" \
  "$KEYS_N_DIR/" \
  "$TARGET_USER@$TARGET_HOST:$KEYS_N_DIR/"

# 3. Push bank-hash env file
echo "[deploy] pushing /etc/staccana/bank-hash"
if [[ -f /etc/staccana/bank-hash ]]; then
  rsync -avz -e "$TARGET_RSYNC_RSH" \
    /etc/staccana/bank-hash \
    "$TARGET_USER@$TARGET_HOST:/etc/staccana/bank-hash"
else
  echo "[deploy] WARNING: /etc/staccana/bank-hash missing on val1 — re-run step 30" >&2
fi

# 4. Generate per-box systemd unit. We can't reuse the static
#    staccana-validator-2.service template because the keys path and gossip-host
#    differ per validator. Generate inline and write directly to the target.
echo "[deploy] generating + pushing systemd unit"
$TARGET_SSH "cat > /etc/systemd/system/staccana-validator.service" <<EOF
[Unit]
Description=Staccana validator (joining / val-${N} — discovers val-1 via --entrypoint)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=staccana
Group=staccana
LimitNOFILE=2000000
LimitNPROC=65535
TimeoutStopSec=180
Restart=on-failure
RestartSec=10s

Environment="RUST_LOG=solana=info,solana_metrics=warn"
Environment="SOLANA_METRICS_CONFIG=host=https://metrics.example,db=staccana,u=writer,p=changeme"
Environment="STACCANA_VAL1_GOSSIP=${VAL1_GOSSIP}"
Environment="STACCANA_THIS_PUBLIC_IP=${TARGET_HOST}"
EnvironmentFile=-/etc/staccana/bank-hash

ExecStart=/usr/local/bin/agave-validator \\
  --identity ${KEYS_N_DIR}/identity.json \\
  --vote-account ${KEYS_N_DIR}/vote.json \\
  --authorized-voter ${KEYS_N_DIR}/vote.json \\
  --ledger ${LEDGER_DIR} \\
  --accounts /var/lib/staccana/accounts \\
  --snapshot-interval-slots 200 \\
  --no-incremental-snapshots \\
  --limit-ledger-size 200000000 \\
  --no-poh-speed-test \\
  --no-os-network-limits-test \\
  --no-port-check \\
  --no-snapshot-fetch \\
  --no-genesis-fetch \\
  --no-wait-for-vote-to-start-leader \\
  --gossip-host \${STACCANA_THIS_PUBLIC_IP} \\
  --entrypoint \${STACCANA_VAL1_GOSSIP} \\
  --full-rpc-api \\
  --allow-private-addr \\
  --rpc-port 8899 \\
  --rpc-bind-address 127.0.0.1 \\
  --gossip-port 8001 \\
  --dynamic-port-range 8002-8020 \\
  --log /var/log/staccana/validator-${N}.log

[Install]
WantedBy=multi-user.target
EOF

# 5. Permissions + restart
echo "[deploy] fixing permissions + restarting"
$TARGET_SSH "chown -R staccana:staccana /var/lib/staccana ${KEYS_N_DIR} && \
             mkdir -p /var/log/staccana && chown -R staccana:staccana /var/log/staccana && \
             systemctl daemon-reload && \
             systemctl reset-failed staccana-validator 2>/dev/null || true && \
             truncate -s 0 /var/log/staccana/validator-${N}.log 2>/dev/null || true && \
             systemctl restart staccana-validator"

echo "[deploy] done. tail logs with:"
echo "  ssh -p $TARGET_SSH_PORT $TARGET_USER@$TARGET_HOST 'tail -f /var/log/staccana/validator-${N}.log'"

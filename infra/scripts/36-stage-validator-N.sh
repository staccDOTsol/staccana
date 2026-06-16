#!/usr/bin/env bash
# 36-stage-validator-N.sh — same as 35 but DOES NOT start the service.
#
# Used when you want to pre-deploy all N validators THEN sync-start them all
# at once with parallel SSH. This is critical for multi-validator bootstrap
# from genesis: all validators must start within epoch 0 (~13 seconds in
# warmup mode) or their towers diverge and the cluster never converges.
#
# Sequence:
#   1. ./36-stage-validator-N.sh 2 84.32.220.76
#   2. ./36-stage-validator-N.sh 3 84.32.103.186
#   3. ./36-stage-validator-N.sh 4 84.32.64.19
#   4. ./37-sync-start-all.sh   # fires `systemctl start` on all 4 in parallel
#
# Usage: same as 35-deploy-validator-N.sh

set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: $0 <validator-number> <host> [user] [ssh_port]" >&2
  exit 2
fi

N="$1"
TARGET_HOST="$2"
TARGET_USER="${3:-root}"
TARGET_SSH_PORT="${4:-22}"

KEY_DIR="${KEY_DIR:-/etc/staccana/keys}"
LEDGER_DIR="${LEDGER_DIR:-/var/lib/staccana/ledger}"
STACCANA_DIR="${STACCANA_DIR:-/opt/staccana}"
VAL1_GOSSIP="${VAL1_GOSSIP:-84.32.220.211:8001}"

KEYS_N_DIR="${KEY_DIR%/keys}/keys-${N}"

if [[ "$N" -lt 2 ]]; then
  echo "[stage] FATAL: N must be >= 2" >&2
  exit 1
fi
if [[ ! -d "$KEYS_N_DIR" ]]; then
  echo "[stage] FATAL: $KEYS_N_DIR not found" >&2
  exit 1
fi
if [[ ! -f "$LEDGER_DIR/genesis.bin" ]]; then
  echo "[stage] FATAL: $LEDGER_DIR/genesis.bin not found — run step 30 first" >&2
  exit 1
fi

TARGET_SSH="ssh -p $TARGET_SSH_PORT $TARGET_USER@$TARGET_HOST"
TARGET_RSYNC_RSH="ssh -p $TARGET_SSH_PORT"

echo "[stage] === val-$N → $TARGET_USER@$TARGET_HOST (no start) ==="

# Stop service if running, wipe its old state
$TARGET_SSH "systemctl stop staccana-validator 2>/dev/null || true; \
             rm -rf /var/lib/staccana/ledger /var/lib/staccana/accounts; \
             mkdir -p /var/lib/staccana/ledger /var/lib/staccana/accounts /var/log/staccana"

# Push ledger
rsync -avz --delete -e "$TARGET_RSYNC_RSH" \
  "$LEDGER_DIR/" \
  "$TARGET_USER@$TARGET_HOST:$LEDGER_DIR/" >/dev/null

# Push keys
$TARGET_SSH "mkdir -p $KEYS_N_DIR && chmod 700 $KEYS_N_DIR"
rsync -avz --delete -e "$TARGET_RSYNC_RSH" \
  "$KEYS_N_DIR/" \
  "$TARGET_USER@$TARGET_HOST:$KEYS_N_DIR/" >/dev/null

# Push bank-hash
if [[ -f /etc/staccana/bank-hash ]]; then
  rsync -avz -e "$TARGET_RSYNC_RSH" \
    /etc/staccana/bank-hash \
    "$TARGET_USER@$TARGET_HOST:/etc/staccana/bank-hash" >/dev/null
fi

# Generate + push systemd unit
$TARGET_SSH "cat > /etc/systemd/system/staccana-validator.service" <<EOF
[Unit]
Description=Staccana validator (val-${N})
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=staccana
Group=staccana
LimitNOFILE=2000000
LimitNPROC=65535
LimitMEMLOCK=infinity
TimeoutStopSec=180
Restart=on-failure
RestartSec=10s

Environment="RUST_LOG=solana=info,solana_metrics=warn"
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
  --bind-address \${STACCANA_THIS_PUBLIC_IP} \\
  --entrypoint \${STACCANA_VAL1_GOSSIP} \\
  --full-rpc-api \\
  --enable-rpc-transaction-history \\
  --enable-extended-tx-metadata-storage \\
  --rpc-pubsub-enable-block-subscription \\
  --allow-private-addr \\
  --rpc-port 8899 \\
  --rpc-bind-address 127.0.0.1 \\
  --gossip-port 8001 \\
  --dynamic-port-range 8002-8027 \\
  --log /var/log/staccana/validator-${N}.log

[Install]
WantedBy=multi-user.target
EOF

# Permissions + daemon-reload (but DO NOT start)
$TARGET_SSH "chown -R staccana:staccana /var/lib/staccana ${KEYS_N_DIR} /var/log/staccana && \
             touch /var/log/staccana/validator-${N}.log && \
             chown staccana:staccana /var/log/staccana/validator-${N}.log && \
             systemctl daemon-reload && \
             systemctl reset-failed staccana-validator 2>/dev/null || true"

echo "[stage] val-$N staged. Will start when 37-sync-start-all.sh fires."

#!/usr/bin/env bash
# 35-deploy-validator-2.sh — push the baked ledger + keys-2 to validator-2's box.
#
# Run this on validator-1's box AFTER step 30 has baked the genesis. It rsyncs:
#   - /var/lib/staccana/ledger/         → val2:/var/lib/staccana/ledger/
#   - /etc/staccana/keys-2/             → val2:/etc/staccana/keys-2/
#   - infra/systemd/staccana-validator-2.service → val2:/etc/systemd/system/staccana-validator.service
#
# Then triggers a `systemctl daemon-reload && systemctl restart staccana-validator`
# on val2 so it picks up the new ledger and keys.
#
# Prerequisites on val2 (run prep commands manually first):
#   - agave-validator 2.0.25 in /usr/local/bin
#   - staccana user + /var/lib/staccana, /etc/staccana, /var/log/staccana dirs
#   - SSH access from val1 to val2 (root) — set up via authorized_keys
#
# Override the destination via env vars:
#   VAL2_HOST=84.32.220.76      # default
#   VAL2_USER=root              # default
#   VAL2_SSH_PORT=22            # default

set -euo pipefail

VAL2_HOST="${VAL2_HOST:-84.32.220.76}"
VAL2_USER="${VAL2_USER:-root}"
VAL2_SSH_PORT="${VAL2_SSH_PORT:-22}"
KEY_DIR="${KEY_DIR:-/etc/staccana/keys}"
LEDGER_DIR="${LEDGER_DIR:-/var/lib/staccana/ledger}"
STACCANA_DIR="${STACCANA_DIR:-/opt/staccana}"

KEYS_2_DIR="${KEY_DIR%/keys}/keys-2"

if [[ ! -d "$KEYS_2_DIR" ]]; then
  echo "[deploy] FATAL: $KEYS_2_DIR not found — did you run step 30 with EXTRA_VALIDATORS=1 (default)?" >&2
  exit 1
fi
if [[ ! -f "$LEDGER_DIR/genesis.bin" ]]; then
  echo "[deploy] FATAL: $LEDGER_DIR/genesis.bin not found — run step 30 first" >&2
  exit 1
fi

VAL2_SSH="ssh -p $VAL2_SSH_PORT $VAL2_USER@$VAL2_HOST"
VAL2_RSYNC_RSH="ssh -p $VAL2_SSH_PORT"

echo "[deploy] pushing ledger to $VAL2_USER@$VAL2_HOST:$LEDGER_DIR"
rsync -avz --delete -e "$VAL2_RSYNC_RSH" \
  "$LEDGER_DIR/" \
  "$VAL2_USER@$VAL2_HOST:$LEDGER_DIR/"

echo "[deploy] pushing keys-2 to $VAL2_USER@$VAL2_HOST:$KEYS_2_DIR"
$VAL2_SSH "mkdir -p $KEYS_2_DIR && chmod 700 $KEYS_2_DIR"
rsync -avz --delete -e "$VAL2_RSYNC_RSH" \
  "$KEYS_2_DIR/" \
  "$VAL2_USER@$VAL2_HOST:$KEYS_2_DIR/"

echo "[deploy] pushing /etc/staccana/bank-hash (BANK_HASH, SHRED_VERSION env file)"
if [[ -f /etc/staccana/bank-hash ]]; then
  rsync -avz -e "$VAL2_RSYNC_RSH" \
    /etc/staccana/bank-hash \
    "$VAL2_USER@$VAL2_HOST:/etc/staccana/bank-hash"
else
  echo "[deploy] WARNING: /etc/staccana/bank-hash missing on val1 — re-run step 30" >&2
fi

echo "[deploy] pushing validator-2 systemd unit"
rsync -avz -e "$VAL2_RSYNC_RSH" \
  "$STACCANA_DIR/infra/systemd/staccana-validator-2.service" \
  "$VAL2_USER@$VAL2_HOST:/etc/systemd/system/staccana-validator.service"

echo "[deploy] fixing permissions and restarting on val2"
$VAL2_SSH "chown -R staccana:staccana /var/lib/staccana /etc/staccana/keys-2 && \
           systemctl daemon-reload && \
           systemctl reset-failed staccana-validator && \
           truncate -s 0 /var/log/staccana/validator-2.log 2>/dev/null || true && \
           systemctl restart staccana-validator"

echo "[deploy] done. tail logs on val2 with:"
echo "  ssh -p $VAL2_SSH_PORT $VAL2_USER@$VAL2_HOST 'tail -f /var/log/staccana/validator-2.log'"
echo "[deploy] both validators should now gossip and start rooting slots within ~30s."

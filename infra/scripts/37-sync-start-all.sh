#!/usr/bin/env bash
# 37-sync-start-all.sh — fire `systemctl start staccana-validator` on all 4 boxes
# in parallel within ~100ms of each other.
#
# Critical for multi-validator bootstrap from genesis: all validators MUST start
# within epoch 0 (~13 sec for warmup epoch with 32 slots) so their PoH ranges
# overlap and their towers can converge on the same fork. Sequential starts
# cause the first validator to produce slots solo, then later joiners see a
# divergent fork they can't catch up on.
#
# Pre-reqs:
#   - Box 1 staged (genesis baked, ledger ready, systemd unit installed but stopped)
#   - Boxes 2/3/4 staged via ./36-stage-validator-N.sh
#
# Hosts can be overridden via env var:
#   HOSTS="84.32.220.211 84.32.220.76 84.32.103.186 84.32.64.19"

set -euo pipefail

HOSTS="${HOSTS:-84.32.220.211 84.32.220.76 84.32.103.186 84.32.64.19}"
SELF_IP="${SELF_IP:-84.32.220.211}"

# Truncate logs first so we see only this run's output
echo "[sync] truncating log files on all boxes"
truncate -s 0 /var/log/staccana/validator.log 2>/dev/null || true
for IP in $HOSTS; do
  if [[ "$IP" != "$SELF_IP" ]]; then
    ssh root@$IP 'truncate -s 0 /var/log/staccana/validator-*.log 2>/dev/null || true' &
  fi
done
wait

# Pre-stop everything to ensure clean parallel start
echo "[sync] pre-stopping all validators"
systemctl stop staccana-validator 2>/dev/null || true
for IP in $HOSTS; do
  if [[ "$IP" != "$SELF_IP" ]]; then
    ssh root@$IP 'systemctl stop staccana-validator 2>/dev/null || true' &
  fi
done
wait

echo "[sync] firing parallel start across all boxes NOW..."
START_TS=$(date +%s.%N)

# Start local first (no ssh latency), then parallel ssh-start the rest
systemctl start staccana-validator &
LOCAL_PID=$!

for IP in $HOSTS; do
  if [[ "$IP" != "$SELF_IP" ]]; then
    ssh root@$IP 'systemctl start staccana-validator' &
  fi
done

wait
END_TS=$(date +%s.%N)
SPAN=$(echo "$END_TS - $START_TS" | bc)
echo "[sync] all start commands fired in ${SPAN}s"
echo "[sync] giving everyone 30s to bootstrap..."
sleep 30

echo "[sync] === STATUS ==="
for IP in $HOSTS; do
  if [[ "$IP" == "$SELF_IP" ]]; then
    STATUS=$(systemctl is-active staccana-validator)
  else
    STATUS=$(ssh root@$IP 'systemctl is-active staccana-validator' 2>&1)
  fi
  echo "  $IP: $STATUS"
done

echo ""
echo "[sync] === EPOCH-INFO ==="
solana --url http://localhost:8899 epoch-info 2>&1 | head -8 || echo "RPC not yet up"

echo ""
echo "[sync] === GOSSIP ==="
solana --url http://localhost:8899 gossip 2>&1 | head -10 || echo "RPC not yet up"

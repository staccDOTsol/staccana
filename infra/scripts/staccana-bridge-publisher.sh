#!/usr/bin/env bash
# staccana-bridge-publisher.sh — fired every 60s by systemd timer. Polls the
# mainnet bridge-vault for recent deposit signatures and runs the federation
# mint-relay over each.
#
# **Self-healing by design.** No cursor file. We always look at the last LIMIT
# (default 50) signatures and run the relay over every one. The relay is fully
# idempotent — re-running on:
#   - a non-deposit sig (init_vault, authorize) → "no DepositEvent" → skip
#   - an already-minted nonce → on-chain `init` of `nonce_in` PDA reverts with
#     "address already in use" → skip
# So missed deposits get retried automatically next minute. No state to corrupt,
# no cursor to advance, no leader to elect. If the validator was offline or the
# RPC choked on the previous run, the next minute's run sweeps the same window
# and any unprocessed deposit gets picked up.
#
# Tunables via env: SOLANA_RPC, BRIDGE_VAULT, RELAY, PAYER, LIMIT.
set -euo pipefail

SOLANA_RPC="${SOLANA_RPC:-https://api.mainnet-beta.solana.com}"
STACCANA_RPC="${STACCANA_RPC:-http://localhost:8899}"
BRIDGE_VAULT="${BRIDGE_VAULT:-BwimCCoPP5of41ukG1wA1gLz5wXQ4mmbcmjdFT9M1mBL}"
STACCANA_BRIDGE="${STACCANA_BRIDGE:-Bridge1111111111111111111111111111111111111}"
MINT_RELAY="${MINT_RELAY:-/usr/local/bin/staccana-bridge-mint-relay}"
RELEASE_RELAY="${RELEASE_RELAY:-/usr/local/bin/staccana-bridge-release-relay}"
PAYER_INBOUND="${PAYER_INBOUND:-/etc/staccana/keys/claim-relay-sponsor.json}"
# Mainnet payer for the release leg — needs SOL on Solana mainnet (the
# inbound `claim-relay-sponsor` only has staccana SOL). Defaults to the
# upgrade-authority key which we keep funded for program upgrades.
PAYER_OUTBOUND="${PAYER_OUTBOUND:-/etc/staccana/keys/upgrade-authority.json}"
LIMIT="${LIMIT:-50}"

# --- Inbound leg: mainnet vault DepositEvents → staccana mint -----------------
# Walk the last LIMIT mainnet sigs touching the vault, oldest-first. Idempotent
# via the on-chain `nonce_in` PDA — already-consumed nonces fail cleanly.
inbound_sigs=$(curl -s -m 20 "$SOLANA_RPC" \
  -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSignaturesForAddress\",\"params\":[\"$BRIDGE_VAULT\",{\"limit\":$LIMIT}]}" \
  | python3 -c '
import json, sys
o = json.load(sys.stdin)
for r in reversed(o.get("result", [])):
    if not r.get("err"):
        print(r["signature"])
')

inbound_minted=0
inbound_skipped=0
inbound_errors=0
if [[ -n "$inbound_sigs" ]]; then
  while read -r sig; do
    [[ -z "$sig" ]] && continue
    set +e
    out=$("$MINT_RELAY" --payer "$PAYER_INBOUND" --deposit-sig "$sig" --solana-rpc "$SOLANA_RPC" --bridge-vault "$BRIDGE_VAULT" 2>&1)
    rc=$?
    set -e
    if [[ $rc -eq 0 ]]; then
      sig_line=$(echo "$out" | grep "minted:" | head -1)
      [[ -n "$sig_line" ]] && echo "[bridge-publisher] inbound[$sig] $sig_line"
      inbound_minted=$((inbound_minted + 1))
    elif echo "$out" | grep -qE "no DepositEvent|already in use|already been processed|custom program error: 0x0"; then
      inbound_skipped=$((inbound_skipped + 1))
    else
      echo "[bridge-publisher] inbound[$sig] UNEXPECTED FAILURE:"
      echo "$out" | tail -5
      inbound_errors=$((inbound_errors + 1))
    fi
  done <<< "$inbound_sigs"
else
  echo "[bridge-publisher] inbound RPC returned 0 sigs — likely transient, will retry next minute"
fi

# --- Outbound leg: staccana bridge BurnEvents → mainnet vault release ---------
# Same shape, but we walk the staccana bridge program's recent sigs. The
# mainnet `nonce_out` PDA is the dedupe guard.
outbound_sigs=$(curl -s -m 20 "$STACCANA_RPC" \
  -H "Content-Type: application/json" \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSignaturesForAddress\",\"params\":[\"$STACCANA_BRIDGE\",{\"limit\":$LIMIT}]}" \
  | python3 -c '
import json, sys
o = json.load(sys.stdin)
for r in reversed(o.get("result", [])):
    if not r.get("err"):
        print(r["signature"])
')

outbound_released=0
outbound_skipped=0
outbound_errors=0
if [[ -n "$outbound_sigs" ]]; then
  while read -r sig; do
    [[ -z "$sig" ]] && continue
    set +e
    out=$("$RELEASE_RELAY" --payer "$PAYER_OUTBOUND" --burn-sig "$sig" --staccana-rpc "$STACCANA_RPC" --staccana-bridge "$STACCANA_BRIDGE" --solana-rpc "$SOLANA_RPC" --bridge-vault "$BRIDGE_VAULT" 2>&1)
    rc=$?
    set -e
    if [[ $rc -eq 0 ]]; then
      sig_line=$(echo "$out" | grep "released:" | head -1)
      [[ -n "$sig_line" ]] && echo "[bridge-publisher] outbound[$sig] $sig_line"
      outbound_released=$((outbound_released + 1))
    elif echo "$out" | grep -qE "no BurnEvent|already in use|already been processed|custom program error: 0x0"; then
      outbound_skipped=$((outbound_skipped + 1))
    else
      echo "[bridge-publisher] outbound[$sig] UNEXPECTED FAILURE:"
      echo "$out" | tail -5
      outbound_errors=$((outbound_errors + 1))
    fi
  done <<< "$outbound_sigs"
fi

echo "[bridge-publisher] inbound: $inbound_minted minted, $inbound_skipped skipped, $inbound_errors errored | outbound: $outbound_released released, $outbound_skipped skipped, $outbound_errors errored"

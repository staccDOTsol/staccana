#!/usr/bin/env bash
# 45-deploy-bridge-vault-devnet.sh — deploy `staccana-bridge-vault` to Solana devnet.
#
# Counterpart to step 40 (which deploys the staccana-side programs to the local
# staccana validator). The bridge VAULT runs on Solana mainnet (or devnet for testing
# end-to-end bridge flows) and is the custody + settlement layer for inbound deposits
# and outbound releases under M-of-N federation attestation.
#
# Prereqs:
#   - $HOME/.config/solana/id.json exists and is the OPERATOR's mainnet/devnet wallet
#     (NOT the staccana validator identity — devnet airdrops to a fresh keypair would
#     just spam the faucet, and we want the operator's deploy authority to own this
#     program from day one)
#   - cargo-build-sbf available on PATH (we add /opt/agave-build/solana-release/bin)
#   - Internet access to api.devnet.solana.com
#
# Output:
#   - /etc/staccana/keys/program-bridge-vault-devnet.json (program-id keypair, fresh
#     if not already present — KEEP THIS SAFE, it's the upgrade authority key)
#   - /etc/staccana/bridge-vault-devnet-id.txt (the deployed program ID, ASCII)

set -euo pipefail

STACCANA_DIR="${STACCANA_DIR:-/opt/staccana}"
KEY_DIR="${KEY_DIR:-/etc/staccana/keys}"
PROGRAM_KEY="$KEY_DIR/program-bridge-vault-devnet.json"
DEPLOY_DIR="${DEPLOY_DIR:-$STACCANA_DIR/target/deploy}"
SO_NAME="staccana_bridge_vault.so"
ID_OUT="/etc/staccana/bridge-vault-devnet-id.txt"
OPERATOR_KEY="${OPERATOR_KEY:-$HOME/.config/solana/id.json}"

# cargo-build-sbf ships with the agave/solana toolchain; add its bin to PATH.
export PATH="/opt/agave-build/solana-release/bin:$PATH"

mkdir -p "$KEY_DIR"
mkdir -p "$(dirname "$ID_OUT")"

echo "[deploy-vault] === sanity ==="
which cargo-build-sbf || { echo "cargo-build-sbf not found on PATH" >&2; exit 1; }
which solana          || { echo "solana CLI not found on PATH" >&2; exit 1; }

if [[ ! -f "$OPERATOR_KEY" ]]; then
  echo "[deploy-vault] ERROR: operator key not found at $OPERATOR_KEY" >&2
  echo "[deploy-vault] Generate one with:  solana-keygen new --outfile $OPERATOR_KEY" >&2
  exit 1
fi

echo ""
echo "[deploy-vault] === building staccana_bridge_vault.so ==="
cd "$STACCANA_DIR"
# `cargo build-sbf` discovers the bridge-vault crate via the workspace; pass the
# package explicitly so we don't rebuild every program.
cargo build-sbf --manifest-path "$STACCANA_DIR/programs/bridge-vault/Cargo.toml"

if [[ ! -f "$DEPLOY_DIR/$SO_NAME" ]]; then
  echo "[deploy-vault] ERROR: expected $DEPLOY_DIR/$SO_NAME after build but didn't find it" >&2
  echo "[deploy-vault] target/deploy contents:" >&2
  ls -la "$DEPLOY_DIR" >&2 || true
  exit 1
fi

echo ""
echo "[deploy-vault] === ensuring program-id keypair exists ==="
if [[ ! -f "$PROGRAM_KEY" ]]; then
  echo "[deploy-vault] generating fresh program-id keypair at $PROGRAM_KEY"
  solana-keygen new --no-passphrase --silent --outfile "$PROGRAM_KEY"
fi
PROGRAM_ID=$(solana-keygen pubkey "$PROGRAM_KEY")
echo "[deploy-vault] program-id will be: $PROGRAM_ID"

echo ""
echo "[deploy-vault] === configuring solana CLI for devnet (operator wallet) ==="
# Explicitly use the operator's wallet, NOT the staccana validator identity.
solana config set --url devnet --keypair "$OPERATOR_KEY" >/dev/null
solana config get

echo ""
echo "[deploy-vault] === topping up devnet SOL for deploy fees ==="
# Devnet airdrop is 5 SOL/24h per address; one airdrop should cover any program
# deploy. If this fails (rate limit, faucet down) the deploy will surface a clear
# insufficient-funds error and the operator can retry / fund manually.
solana airdrop 5 || echo "[deploy-vault] WARN: airdrop failed — proceeding anyway, balance is:"
solana balance

echo ""
echo "[deploy-vault] === deploying ==="
solana program deploy \
  --program-id "$PROGRAM_KEY" \
  "$DEPLOY_DIR/$SO_NAME"

# Persist the program ID so downstream scripts (federation-attestor, frontend) can
# read it without re-deriving from the keypair.
echo "$PROGRAM_ID" > "$ID_OUT"
echo ""
echo "[deploy-vault] === SUMMARY ==="
echo "[deploy-vault] program-id: $PROGRAM_ID"
echo "[deploy-vault] wrote      : $ID_OUT"
echo "[deploy-vault] verify on  : https://explorer.solana.com/address/$PROGRAM_ID?cluster=devnet"

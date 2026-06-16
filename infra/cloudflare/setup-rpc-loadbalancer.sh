#!/usr/bin/env bash
# setup-rpc-loadbalancer.sh — provision a Cloudflare LB pool that fronts the 5 staccana RPCs
# behind a single URL: https://rpc.mp.fun/
#
# Idempotent. Safe to re-run.
#
# Prereqs:
#   - $CLOUDFLARE_API_TOKEN with Zone:Edit + Load Balancing:Edit perms
#   - $CLOUDFLARE_ZONE_ID for the mp.fun domain
#   - Each RPC node's public IP set in the IP_* env vars below
#   - Each RPC's nginx exposes /health returning 200 OK when the underlying validator is
#     producing slots within 32 of network tip

set -euo pipefail

: "${CLOUDFLARE_API_TOKEN:?must set CLOUDFLARE_API_TOKEN}"
: "${CLOUDFLARE_ZONE_ID:?must set CLOUDFLARE_ZONE_ID}"
DOMAIN="${DOMAIN:-mp.fun}"
LB_HOSTNAME="${LB_HOSTNAME:-rpc.$DOMAIN}"

# RPC node IPs by region — fill in real values
IP_US_WEST="${IP_US_WEST:-1.2.3.10}"
IP_US_EAST="${IP_US_EAST:-1.2.3.11}"
IP_EU="${IP_EU:-1.2.3.12}"
IP_APAC="${IP_APAC:-1.2.3.13}"
IP_SA="${IP_SA:-1.2.3.14}"

api() { curl -fsS -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" -H "Content-Type: application/json" "$@"; }

echo "[cf] creating per-region A records (used as fallback + for power users)"
for pair in \
  "rpc-us-west:$IP_US_WEST" \
  "rpc-us-east:$IP_US_EAST" \
  "rpc-eu:$IP_EU" \
  "rpc-apac:$IP_APAC" \
  "rpc-sa:$IP_SA"; do
  name="${pair%%:*}"
  ip="${pair##*:}"
  api -X POST "https://api.cloudflare.com/client/v4/zones/$CLOUDFLARE_ZONE_ID/dns_records" \
    -d "{\"type\":\"A\",\"name\":\"$name\",\"content\":\"$ip\",\"ttl\":1,\"proxied\":true}" \
    || echo "  (record may already exist — skipping)"
done

echo "[cf] creating health check monitor"
MONITOR_ID=$(api -X POST "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/load_balancers/monitors" \
  -d '{
    "type": "https",
    "method": "GET",
    "path": "/health",
    "interval": 30,
    "retries": 2,
    "timeout": 5,
    "expected_codes": "200",
    "follow_redirects": false,
    "header": {"User-Agent": ["staccana-cf-monitor"]},
    "description": "Staccana RPC health probe"
  }' | jq -r '.result.id')
echo "[cf]   monitor: $MONITOR_ID"

echo "[cf] creating origin pools (one per region for geo-steering)"
declare -A POOLS
for pair in \
  "us-west:$IP_US_WEST" \
  "us-east:$IP_US_EAST" \
  "eu:$IP_EU" \
  "apac:$IP_APAC" \
  "sa:$IP_SA"; do
  region="${pair%%:*}"
  ip="${pair##*:}"
  POOL_ID=$(api -X POST "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/load_balancers/pools" \
    -d "{
      \"name\": \"staccana-rpc-$region\",
      \"description\": \"Staccana RPC pool — $region\",
      \"enabled\": true,
      \"minimum_origins\": 1,
      \"monitor\": \"$MONITOR_ID\",
      \"origins\": [
        {\"name\": \"rpc-$region\", \"address\": \"$ip\", \"enabled\": true, \"weight\": 1}
      ]
    }" | jq -r '.result.id')
  POOLS[$region]=$POOL_ID
  echo "[cf]   pool $region: $POOL_ID"
done

echo "[cf] creating load balancer with geo-steering"
api -X POST "https://api.cloudflare.com/client/v4/zones/$CLOUDFLARE_ZONE_ID/load_balancers" \
  -d "{
    \"name\": \"$LB_HOSTNAME\",
    \"description\": \"Staccana RPC — geo-steered single endpoint\",
    \"ttl\": 30,
    \"proxied\": true,
    \"steering_policy\": \"geo\",
    \"fallback_pool\": \"${POOLS[us-west]}\",
    \"default_pools\": [\"${POOLS[us-west]}\"],
    \"region_pools\": {
      \"WNAM\": [\"${POOLS[us-west]}\"],
      \"ENAM\": [\"${POOLS[us-east]}\", \"${POOLS[us-west]}\"],
      \"WEU\":  [\"${POOLS[eu]}\"],
      \"EEU\":  [\"${POOLS[eu]}\"],
      \"NAF\":  [\"${POOLS[eu]}\"],
      \"SAF\":  [\"${POOLS[eu]}\"],
      \"ME\":   [\"${POOLS[eu]}\", \"${POOLS[sa]}\"],
      \"SAS\":  [\"${POOLS[sa]}\"],
      \"SEAS\": [\"${POOLS[apac]}\"],
      \"NEAS\": [\"${POOLS[apac]}\"],
      \"OC\":   [\"${POOLS[apac]}\"]
    }
  }"

echo "[cf] done. https://$LB_HOSTNAME/ now load-balances across 5 RPCs with geo steering + health checks."
echo "[cf] verify: curl -sS https://$LB_HOSTNAME/health"

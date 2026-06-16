# Staccana Infrastructure Operations

Phase 1 execution scripts + Ansible playbook for going from "fresh Cherryservers/Hetzner box" → "live staccana validator producing slots."

See `docs/INFRA.md` for the deployment topology (3 validators + 5 RPCs + 5 attestors) and `docs/E2E_DEPLOY.md` for the full Phase A/B/C deploy pipeline narrative.

## Layout

```
infra/
├── ansible/
│   ├── inventory.yml.example   # template — copy to inventory.yml + fill IPs
│   └── site.yml                # top-level playbook (bootstrap / validator / rpc / attestor / update)
├── scripts/
│   ├── 00-bootstrap-box.sh     # fresh box → ready (apt + sysctls + ulimits + NVMe + ufw + user)
│   ├── 10-pull-snapshot.sh     # download a recent mainnet snapshot
│   ├── 20-build-genesis.sh     # snapshot → GenesisOutput → ComposedGenesis → genesis.bin
│   ├── 30-init-validator.sh    # generate keypairs + init ledger from staccana genesis
│   ├── 40-deploy-programs.sh   # build .so artifacts + deploy lazy-claim, bridge, secret-pump
│   └── 50-init-state.sh        # bridge register-asset, federation init, smoke-test claim
├── systemd/
│   ├── staccana-validator.service
│   └── staccana-attestor.service
└── README.md (this file)
```

## Quickstart — first validator on a Hetzner box

```bash
# From your laptop (one-time):
cp infra/ansible/inventory.yml.example infra/ansible/inventory.yml
vim infra/ansible/inventory.yml   # fill in real IP for val-us-west

# Provision the box
ansible-playbook -i infra/ansible/inventory.yml infra/ansible/site.yml \
  --limit val-us-west --tags bootstrap,validator

# SSH in for the chain-specific steps (one-time per genesis)
ssh root@val-us-west.internal.mp.fun

cd /opt/staccana
infra/scripts/10-pull-snapshot.sh                      # ~10-15 min
infra/scripts/20-build-genesis.sh                      # ~5 min
infra/scripts/30-init-validator.sh                     # ~1 min
systemctl enable --now staccana-validator              # validator boots
journalctl -u staccana-validator -f                    # tail logs

# Once validator is producing slots:
infra/scripts/40-deploy-programs.sh                    # deploy .so files
infra/scripts/50-init-state.sh                         # init on-chain state
```

## Adding a new validator (post-launch, post-genesis)

```bash
# Add to inventory.yml under `validators:`
ansible-playbook -i infra/ansible/inventory.yml infra/ansible/site.yml \
  --limit val-tokyo --tags bootstrap,validator

ssh root@val-tokyo.internal.mp.fun
cd /opt/staccana
# Copy genesis.bin from an existing validator
scp val-us-west.internal.mp.fun:/var/lib/staccana/ledger/genesis.bin /var/lib/staccana/ledger/
infra/scripts/30-init-validator.sh                     # generate per-box keypairs
systemctl enable --now staccana-validator
```

## Adding an RPC node

```bash
ansible-playbook -i infra/ansible/inventory.yml infra/ansible/site.yml \
  --limit rpc-eu --tags bootstrap,rpc
ssh root@rpc-eu.internal.mp.fun
cd /opt/staccana
scp val-us-west.internal.mp.fun:/var/lib/staccana/ledger/genesis.bin /var/lib/staccana/ledger/
infra/scripts/30-init-validator.sh                     # same init flow; runs in --no-voting via systemd override
systemctl enable --now staccana-validator

# Install nginx snippet for /health + rate limit + TLS termination
cp infra/cloudflare/nginx-rpc-snippet.conf /etc/nginx/conf.d/staccana-rpc.conf
nginx -t && systemctl reload nginx
```

## Cluster URL — single load-balanced endpoint

The 5 RPC nodes sit behind Cloudflare so users hit one URL: `https://rpc.mp.fun/`. Cloudflare:

- Terminates TLS
- Geo-steers requests to the nearest healthy region (US-West, US-East, EU, APAC, South Asia)
- Health-checks each origin every 30s via the nginx `/health` endpoint
- Auto-failover when an RPC goes degraded
- DDoS protection + rate limiting at the edge

Power users can pin to a specific region via the per-region subdomain (`https://rpc-eu.mp.fun/`) — Ansible creates these A records alongside the LB.

```bash
# After RPC nodes are up and nginx is serving /health:
export CLOUDFLARE_API_TOKEN=...
export CLOUDFLARE_ZONE_ID=...
export CLOUDFLARE_ACCOUNT_ID=...
# IPs of each RPC node
export IP_US_WEST=1.2.3.10
export IP_US_EAST=1.2.3.11
export IP_EU=1.2.3.12
export IP_APAC=1.2.3.13
export IP_SA=1.2.3.14

bash infra/cloudflare/setup-rpc-loadbalancer.sh
```

This creates: 5 per-region A records + 1 health check monitor + 5 origin pools + 1 LB with geo steering. Idempotent; safe to re-run when adding/removing nodes.

Cost: Cloudflare free tier covers TLS + caching + DDoS; the LB pool itself is ~$5/mo. Total ~$5/mo for global single-URL RPC.

## Adding a federation attestor

```bash
# Distribute member keypair to the signer ahead of time (out of band, not via Ansible)
ansible-playbook -i infra/ansible/inventory.yml infra/ansible/site.yml \
  --limit attestor-0 --tags bootstrap,attestor
ssh root@attestor-0.internal.mp.fun
# Place /etc/staccana/keys/federation-member.json (the private key)
chmod 600 /etc/staccana/keys/federation-member.json
chown staccana:staccana /etc/staccana/keys/federation-member.json
systemctl enable --now staccana-attestor
journalctl -u staccana-attestor -f
```

## Updating staccana on every box

```bash
ansible-playbook -i infra/ansible/inventory.yml infra/ansible/site.yml --tags update
```

## v1.1 evolution → k3s + Helm

Per `docs/INFRA.md`, mainnet-sigma ships with this Ansible setup. Post-launch, migrate to `k3s` + Helm chart at `infra/helm/staccana/` for centralized monitoring, GitOps deploys, and easier scale-out. The Ansible bootstrap of `00-bootstrap-box.sh` becomes the prep for `curl https://get.k3s.io | sh -` instead of installing systemd units directly.

## Operational playbooks

- **Validator stuck / slot-lag growing**: see `journalctl -u staccana-validator -n 200`. Common causes: snapshot too old (delete and re-fetch), ledger corruption (rebuild from peer snapshot), out of disk (extend NVMe).
- **Federation signer offline > 5 min**: alert via Telegram. Failover by recruiting the on-call engineer to bring the box back up.
- **Bridge withdrawal stuck**: check the federation-attestor logs for sig collection. Bridge requires M signatures; if M-1 are received it stalls.
- **Mainnet RPC rate-limited (snapshot pull fails)**: rotate `KNOWN_VALIDATOR` env to a different mainnet validator; or pull from one of the public snapshot mirrors.

#!/usr/bin/env bash
# 00-bootstrap-box.sh — fresh Cherryservers / Hetzner box → ready-for-staccana-validator.
#
# Idempotent. Safe to re-run. Tested on Ubuntu 22.04 LTS.
#
# Prereqs: root SSH access, NVMe drive(s) attached.

set -euo pipefail

LEDGER_MOUNT="${LEDGER_MOUNT:-/var/lib/staccana}"
ACCOUNTS_MOUNT="${ACCOUNTS_MOUNT:-/var/lib/staccana/accounts}"
NVME_DEVICE="${NVME_DEVICE:-/dev/nvme1n1}"  # second NVMe; first is OS

echo "[bootstrap] $(date -Iseconds) starting on $(hostname)"

# 1. System packages
apt-get update -qq
apt-get install -y --no-install-recommends \
  build-essential pkg-config libssl-dev libudev-dev curl jq \
  htop iotop iftop tcpdump \
  prometheus-node-exporter \
  ufw fail2ban

# 2. Solana CLI (matches the workspace solana-sdk = "2.0" pin)
if ! command -v solana >/dev/null; then
  sh -c "$(curl -sSfL https://release.anza.xyz/v2.0.25/install)"
  echo 'export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"' >> /root/.bashrc
fi

# 2b. Copy Solana binaries into /usr/local/bin so systemd units (which run as the
# unprivileged `staccana` user) can find AND execute them. We *copy* rather than symlink
# because /root/ is mode 700; symlinks into it work for root but the staccana user can't
# traverse the path. Costs ~500MB of disk; trivial vs the 1.7TB+ NVMe.
SOLANA_BIN_DIR="/root/.local/share/solana/install/active_release/bin"
if [[ -d "$SOLANA_BIN_DIR" ]]; then
  for bin in agave-validator agave-validator-genesis agave-ledger-tool \
             solana solana-keygen solana-genesis solana-ledger-tool \
             solana-test-validator; do
    if [[ -x "$SOLANA_BIN_DIR/$bin" ]]; then
      # rm first so a stale symlink from an earlier run doesn't trip cp's
      # "source and destination are the same file" detection.
      rm -f "/usr/local/bin/$bin"
      cp -p "$SOLANA_BIN_DIR/$bin" "/usr/local/bin/$bin"
      chmod 755 "/usr/local/bin/$bin"
    fi
  done
fi

# 3. Rust toolchain (only if building from source on this box)
if ! command -v cargo >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  source "$HOME/.cargo/env"
fi

# 4. NVMe mount for ledger + accounts.
#
# Three cases handled:
#   (a) $NVME_DEVICE exists, is NOT in use, is not part of the system → mkfs + mount.
#   (b) $NVME_DEVICE exists but IS already in use (single-drive box where nvme1n1 is the
#       OS drive — common on Cherryservers). Skip mkfs; $LEDGER_MOUNT lives on the root fs.
#   (c) $NVME_DEVICE doesn't exist → skip; $LEDGER_MOUNT lives on the root fs.
device_in_use() {
  # Returns 0 if any partition of the device is mounted anywhere
  lsblk -no MOUNTPOINTS "$1" 2>/dev/null | grep -q .
}

if [[ -b "$NVME_DEVICE" ]]; then
  if device_in_use "$NVME_DEVICE"; then
    echo "[bootstrap] $NVME_DEVICE is already in use (likely the system drive); skipping mkfs."
    echo "[bootstrap] $LEDGER_MOUNT will live on the root filesystem ($(df -h / | awk 'NR==2 {print $4}') free)."
  elif ! mountpoint -q "$LEDGER_MOUNT"; then
    echo "[bootstrap] formatting + mounting $NVME_DEVICE → $LEDGER_MOUNT"
    mkfs.ext4 -F "$NVME_DEVICE"
    mkdir -p "$LEDGER_MOUNT"
    mount "$NVME_DEVICE" "$LEDGER_MOUNT"
    echo "$NVME_DEVICE $LEDGER_MOUNT ext4 defaults,noatime,nodiratime 0 0" >> /etc/fstab
  fi
else
  echo "[bootstrap] $NVME_DEVICE does not exist; using root filesystem for $LEDGER_MOUNT."
fi
mkdir -p "$LEDGER_MOUNT" "$ACCOUNTS_MOUNT" /var/log/staccana /etc/staccana

# 5. Sysctls — Solana validator recommendations
cat > /etc/sysctl.d/99-staccana.conf <<EOF
# UDP buffer sizes for Turbine + gossip
net.core.rmem_default = 134217728
net.core.rmem_max     = 134217728
net.core.wmem_default = 134217728
net.core.wmem_max     = 134217728
net.ipv4.udp_rmem_min = 8192
net.ipv4.udp_wmem_min = 8192

# Increase max open file descriptors
fs.file-max = 2000000
fs.nr_open  = 2000000

# Disable swap behavior; validators want predictable RAM access
vm.swappiness = 1
vm.max_map_count = 2000000
EOF
sysctl --system

# 6. ulimits
cat > /etc/security/limits.d/99-staccana.conf <<EOF
*  soft  nofile  2000000
*  hard  nofile  2000000
*  soft  nproc   65535
*  hard  nproc   65535
EOF

# 7. Firewall — gossip + Turbine + (selectively) RPC
ufw default deny incoming
ufw default allow outgoing
ufw allow ssh
ufw allow 8000:8020/udp comment 'staccana gossip + turbine'
ufw allow 8000:8020/tcp comment 'staccana TPU + repair'
ufw allow 9100/tcp comment 'prometheus node-exporter'
# RPC port stays closed by default; opens only on RPC-role boxes via:
#   ufw allow 8899/tcp comment 'staccana RPC'

# 8. Dedicated user for the validator daemon
if ! id staccana >/dev/null 2>&1; then
  useradd --system --home-dir "$LEDGER_MOUNT" --shell /usr/sbin/nologin staccana
fi
chown -R staccana:staccana "$LEDGER_MOUNT" /var/log/staccana /etc/staccana

echo "[bootstrap] done. next: ./10-pull-snapshot.sh"

#!/usr/bin/env bash
# Host prerequisites + k3s (single node). Run with sudo:
#   sudo bash scripts/setup-host.sh
set -euo pipefail

pacman -S --needed --noconfirm curl git mosquitto

# Time sync: ingester is the timestamp authority, so NTP must be on.
systemctl enable --now systemd-timesyncd || true
timedatectl set-ntp true || true

if ! command -v k3s >/dev/null; then
  curl -sfL https://get.k3s.io | sh -s - server --write-kubeconfig-mode 644
fi
systemctl enable --now k3s

echo "k3s version: $(k3s --version | head -1)"
echo "Waiting for node Ready..."
until k3s kubectl get nodes 2>/dev/null | grep -q ' Ready'; do sleep 3; done
k3s kubectl get nodes -o wide

#!/usr/bin/env bash
# ホスト (k3s ノード) で Ingress の .local ホスト名を mDNS 広告する。
# LAN の各端末で /etc/hosts を書かずに http://dashboard.local/ を開けるようにするため。
#
# - systemd-resolved の mDNS 応答は止め、avahi に一本化する
#   (両方が 5353 で応答すると衝突する。resolved は自ホスト名しか広告できず
#    dashboard.local のような別名を出せないため avahi を採用)
# - 別名ごとに avahi-publish を常駐させる systemd unit を置く
# - IP は起動時に LAN IF から取得する (DHCP で変わっても再起動で追従)
#
# 冪等。名前を足すときは ALIASES に追加して再実行する。
set -euo pipefail

IFACE=${IFACE:-enp1s0}
ALIASES=(dashboard.local)

sudo pacman -S --needed --noconfirm avahi >/dev/null

sudo install -Dm644 /dev/stdin /etc/systemd/resolved.conf.d/10-no-mdns.conf <<CONF
# avahi に mDNS を任せるため resolved 側の応答を止める (scripts/host/setup-mdns.sh)
[Resolve]
MulticastDNS=no
CONF
sudo systemctl restart systemd-resolved

sudo systemctl enable --now avahi-daemon

sudo install -Dm644 /dev/stdin /etc/systemd/system/avahi-alias@.service <<UNIT
# %i の名前をこのホストの ${IFACE} の IPv4 で mDNS 広告する (scripts/host/setup-mdns.sh)
[Unit]
Description=mDNS alias %i
Requires=avahi-daemon.service
After=avahi-daemon.service network-online.target
Wants=network-online.target

[Service]
ExecStart=/bin/sh -c 'exec /usr/bin/avahi-publish -a -R %i "\$(ip -4 -o addr show ${IFACE} | awk "{print \\\$4}" | cut -d/ -f1 | head -1)"'
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload

for a in "${ALIASES[@]}"; do
  sudo systemctl enable --now "avahi-alias@${a}.service"
  sudo systemctl restart "avahi-alias@${a}.service"
done
echo "done: ${ALIASES[*]}"

#!/usr/bin/env bash
# ホストへの IPv6 の新規接続を LAN からのみに制限する。
#
# このホストの IPv6 グローバルアドレスはインターネットから直接届く。IPv4 では
# ルーターの NAT の内側だが、IPv6 で外から届くかどうかはルーターの IPv6 フィルタ
# だけに依存している。sshd / k3s API (6443) / kubelet (10250) / node_exporter
# (9100, 認証なし) / Minecraft とその RCON / dashboard の IPv6 中継 (80) などが
# [::] で待ち受けているので、ホスト側にも壁を1枚置く (多層防御)。
#
# - ip6 専用テーブル lan_guard だけを作って入れ替える。他のテーブル (k3s /
#   Docker が iptables-nft で作るもの) には触れない。nftables.service は
#   有効化しないこと (/etc/nftables.conf の既定は flush ruleset で k3s の
#   ルールを消す)
# - LAN IF から入る通信のうち、オンリンクのプレフィックス・リンクローカル以外
#   からの「TCP の接続開始 (SYN)」と「待ち受け中の UDP (mDNS 5353 / LLMNR
#   5355)」を落とす。ICMPv6 (近隣探索・PMTU) とマルチキャスト宛ては通す。
#   こちらから張った接続の戻りは SYN ではないので通る。他の IF (Docker /
#   CNI / flannel のブリッジ) から入る通信は対象外
# - conntrack (ct state) を使わないステートレスな判定にしている。カーネルを
#   更新して再起動していない Arch では稼働中カーネルのモジュールがディスク
#   から消えており、未ロードの nft_ct を読めずルール全体の投入が失敗した
#   (2026-10-12 に実際に踏んだ)。モジュール読み込みに依存しない形にした
# - プレフィックスは RA で配られるので ISP 側で変わり得る。タイマーで5分ごとに
#   読み直してルールを入れ直す
#
# 冪等。外から届いてほしいポートが出たら PUBLIC_TCP_PORTS に足して再実行する。
set -euo pipefail

IFACE=${IFACE:-enp1s0}
PUBLIC_TCP_PORTS=${PUBLIC_TCP_PORTS:-}   # 例: "25565" (空 = 外部公開なし)

sudo install -Dm755 /dev/stdin /usr/local/libexec/ipv6-lan-guard <<'SH'
#!/bin/sh
# usage: ipv6-lan-guard <iface> [public-tcp-ports...]  (scripts/host/setup-ipv6-guard.sh が配置)
# ip6 lan_guard テーブルを現在のオンリンクプレフィックスで作り直す
set -eu
ifc=$1; shift
prefixes=$(ip -6 route show dev "$ifc" | awk '$1 ~ /\// && $1 !~ /^fe80/ && $0 !~ / via / {print $1}' | sort -u | paste -sd, -)
allow="fe80::/10${prefixes:+, $prefixes}"
public=""
[ "$#" -gt 0 ] && public="tcp dport { $(echo "$@" | tr ' ' ',') } accept"
nft -f - <<NFT
table ip6 lan_guard
delete table ip6 lan_guard
table ip6 lan_guard {
  chain input {
    # iptables-nft の filter (priority 0) より先に評価する。ここでの drop は
    # 最終だが accept は他のテーブルの判定を上書きしない (守りを弱めない)
    type filter hook input priority filter - 10; policy accept;
    iifname != "$ifc" accept
    meta l4proto ipv6-icmp accept
    ip6 daddr ff00::/8 accept
    ip6 saddr { $allow } accept
    $public
    tcp flags & (fin | syn | rst | ack) == syn counter drop comment "LAN 外からの新規 TCP 接続"
    udp dport { 5353, 5355 } counter drop comment "LAN 外からの mDNS / LLMNR"
  }
}
NFT
echo "lan_guard: iface=$ifc allow={ $allow } public=[$*]"
SH

sudo install -Dm644 /dev/stdin /etc/systemd/system/ipv6-lan-guard.service <<UNIT
# LAN 外からの新規 IPv6 接続を落とす nftables テーブルを作り直す (scripts/host/setup-ipv6-guard.sh)
[Unit]
Description=IPv6 LAN-only guard (nft table ip6 lan_guard)
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/libexec/ipv6-lan-guard ${IFACE} ${PUBLIC_TCP_PORTS}
UNIT

sudo install -Dm644 /dev/stdin /etc/systemd/system/ipv6-lan-guard.timer <<UNIT
# プレフィックス変更に追従するため定期的に作り直す (scripts/host/setup-ipv6-guard.sh)
[Unit]
Description=Refresh IPv6 LAN-only guard

[Timer]
OnBootSec=30s
OnUnitActiveSec=5min

[Install]
WantedBy=timers.target
UNIT

sudo systemctl daemon-reload
sudo systemctl start ipv6-lan-guard.service
sudo systemctl enable --now ipv6-lan-guard.timer
journalctl -u ipv6-lan-guard.service -n 1 --no-pager -o cat

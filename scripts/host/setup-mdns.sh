#!/usr/bin/env bash
# ホスト (k3s ノード) で Ingress の .local ホスト名を mDNS 広告し、
# IPv4 / IPv6 のどちらで引いても dashboard に繋がるようにする。
# LAN の各端末で /etc/hosts を書かずに http://dashboard.local/ を開くため。
#
# - systemd-resolved の mDNS 応答は止め、avahi に一本化する
#   (両方が 5353 で応答すると衝突する。resolved は自ホスト名しか広告できず
#    dashboard.local のような別名を出せないため avahi を採用)
# - avahi の広告は LAN IF だけに絞る。絞らないと flannel/veth のリンク
#   ローカルアドレス (LAN から到達不能) まで広告される
# - 別名ごとに A (avahi-alias4@) と AAAA (avahi-alias6@) を常駐で広告する。
#   アドレスは起動時に IF から取り、変わったら (DHCP / ISP のプレフィックス
#   変更) 自分で終了して systemd に再起動させ、新しいアドレスで広告し直す
# - k3s は IPv4 シングルスタックで traefik も IPv4 のみなので、IPv6 の :80 を
#   systemd-socket-proxyd で受けて 127.0.0.1:80 (traefik) に中継する。
#   クラスタの dual-stack 化はクラスタ再構築が要るため採らない
#
# 冪等。名前を足すときは ALIASES に追加して再実行する。
set -euo pipefail

IFACE=${IFACE:-enp1s0}
ALIASES=(dashboard.local)

sudo pacman -S --needed --noconfirm avahi >/dev/null

# --- resolved の mDNS を止める ------------------------------------------
sudo install -Dm644 /dev/stdin /etc/systemd/resolved.conf.d/10-no-mdns.conf <<CONF
# avahi に mDNS を任せるため resolved 側の応答を止める (scripts/host/setup-mdns.sh)
[Resolve]
MulticastDNS=no
CONF
sudo systemctl restart systemd-resolved

# --- avahi を LAN IF だけに絞る ------------------------------------------
if grep -qE '^\s*#?\s*allow-interfaces=' /etc/avahi/avahi-daemon.conf; then
  sudo sed -i -E "s|^\s*#?\s*allow-interfaces=.*|allow-interfaces=${IFACE}|" /etc/avahi/avahi-daemon.conf
else
  sudo sed -i -E "/^\[server\]/a allow-interfaces=${IFACE}" /etc/avahi/avahi-daemon.conf
fi
sudo sed -i -E 's|^\s*#?\s*use-ipv6=.*|use-ipv6=yes|' /etc/avahi/avahi-daemon.conf
sudo systemctl enable avahi-daemon
sudo systemctl restart avahi-daemon

# --- 別名の広告 -----------------------------------------------------------
sudo install -Dm755 /dev/stdin /usr/local/libexec/avahi-alias <<'SH'
#!/bin/sh
# usage: avahi-alias <name> <4|6> <iface>   (scripts/host/setup-mdns.sh が配置)
# name をその IF の IPv4/IPv6 グローバルアドレスで広告し続ける。
# アドレスが変わったら終了し、systemd の再起動で新アドレスを広告し直す。
name=$1 fam=$2 ifc=$3
addr() {
  ip -"$fam" -o addr show "$ifc" scope global \
    | grep -vE 'temporary|deprecated|tentative' \
    | awk '{print $4}' | cut -d/ -f1 | head -1
}
a=$(addr)
[ -n "$a" ] || { echo "IPv$fam のグローバルアドレスが $ifc に無い" >&2; exit 1; }
/usr/bin/avahi-publish -a -R "$name" "$a" &
p=$!
trap 'kill $p 2>/dev/null' EXIT INT TERM
while sleep 60; do
  kill -0 "$p" 2>/dev/null || exit 1
  b=$(addr)
  [ "$b" = "$a" ] || { echo "アドレス変更 $a -> $b、広告し直す"; exit 0; }
done
SH

for fam in 4 6; do
  sudo install -Dm644 /dev/stdin "/etc/systemd/system/avahi-alias${fam}@.service" <<UNIT
# %i を ${IFACE} の IPv${fam} アドレスで mDNS 広告する (scripts/host/setup-mdns.sh)
[Unit]
Description=mDNS alias %i (IPv${fam})
Requires=avahi-daemon.service
After=avahi-daemon.service network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/libexec/avahi-alias %i ${fam} ${IFACE}
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
UNIT
done

# 旧版 (IPv4 のみの avahi-alias@) からの移行
for a in "${ALIASES[@]}"; do
  sudo systemctl disable --now "avahi-alias@${a}.service" 2>/dev/null || true
done
sudo rm -f /etc/systemd/system/avahi-alias@.service

# --- IPv6 の :80 を traefik (IPv4) に中継する ------------------------------
sudo install -Dm644 /dev/stdin /etc/systemd/system/http-v6-proxy.socket <<UNIT
# IPv6 で来た HTTP を受ける。k3s/traefik は IPv4 のみのため (scripts/host/setup-mdns.sh)
# ipv6-only にして IPv4 の :80 (klipper の hostPort) とは衝突させない
[Socket]
ListenStream=[::]:80
BindIPv6Only=ipv6-only

[Install]
WantedBy=sockets.target
UNIT
sudo install -Dm644 /dev/stdin /etc/systemd/system/http-v6-proxy.service <<UNIT
# http-v6-proxy.socket で受けた接続を traefik (127.0.0.1:80) に中継する
[Unit]
Requires=http-v6-proxy.socket
After=http-v6-proxy.socket

[Service]
ExecStart=/usr/lib/systemd/systemd-socket-proxyd 127.0.0.1:80
UNIT

# IPv6 のグローバルアドレスはインターネットから直接届く。IPv4 の dashboard は
# ルーターの NAT の内側だったが、IPv6 で 80 番を開くと外から見えるかどうかは
# ルーターの IPv6 フィルタ次第になってしまう。LAN のオンリンクプレフィックスと
# リンクローカル、localhost (中継先の 127.0.0.1:80) 以外は拒否する。
# プレフィックスは RA で配られたもの。ISP 側で変わると LAN 端末からも IPv6 では
# 繋がらなくなるが、閉じる側に倒れるだけで、ブラウザは Happy Eyeballs で IPv4 に
# 落ちて開ける。その場合はこのスクリプトを再実行する。
PREFIX6=$(ip -6 route show dev "$IFACE" | awk '$1 !~ /^fe80/ && $1 ~ /\// {print $1; exit}')
[ -n "$PREFIX6" ] || { echo "$IFACE に IPv6 のオンリンクプレフィックスが無い" >&2; exit 1; }
# socket 側にだけ掛ける。accept された接続も待ち受けソケットの設定を引き継ぐ
# ので、これで外からの接続は落ちる (LAN を許可リストから外すと遮断されることを
# 確認済み)。service 側に掛けると、中継先 127.0.0.1:80 が iptables で traefik の
# Pod IP (10.42.0.x) に書き換えられた後の宛先で弾かれ、LAN からも繋がらなくなる。
sudo rm -rf /etc/systemd/system/http-v6-proxy.service.d
sudo install -Dm644 /dev/stdin /etc/systemd/system/http-v6-proxy.socket.d/10-lan-only.conf <<UNIT
# LAN (${PREFIX6}) とリンクローカル以外からの IPv6 接続を拒否する (scripts/host/setup-mdns.sh)
[Socket]
IPAddressDeny=any
IPAddressAllow=localhost link-local ${PREFIX6}
UNIT

sudo systemctl daemon-reload
sudo systemctl restart http-v6-proxy.socket
sudo systemctl enable --now http-v6-proxy.socket
for a in "${ALIASES[@]}"; do
  for fam in 4 6; do
    sudo systemctl enable "avahi-alias${fam}@${a}.service"
    sudo systemctl restart "avahi-alias${fam}@${a}.service"
  done
done
echo "done: ${ALIASES[*]} (IPv4 + IPv6)"

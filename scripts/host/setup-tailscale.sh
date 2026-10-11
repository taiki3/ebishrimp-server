#!/usr/bin/env bash
# ホスト (k3s ノード) を Tailscale の tailnet に参加させ、ダッシュボードを出先から
# https://<マシン名>.<tailnet>.ts.net/ で開けるようにする。開けるのは同じ tailnet に
# ログインした自分の端末だけ。
#
# - 経路: tailnet → tailscale serve (HTTPS 終端。証明書は Tailscale が自動で取得・更新)
#   → Traefik (127.0.0.1:80) → Ingress の catch-all → dashboard
# - tailscaled は userspace-networking で動かす。TUN を作らず、ホストの iptables・
#   ルーティング・DNS に一切触れないので、k3s (flannel / kube-proxy / kube-router)
#   のネットワークと干渉しない。tailnet から来た接続は tailscaled が受けてホストの
#   localhost に繋ぐ。ホストから tailnet へ出ていく通信はできないが、この用途では不要
# - Funnel (インターネットへの公開) は使わない。ダッシュボードには認証がない
#
# 初回だけブラウザでの承認が要る (スクリプトが URL を表示して待つ):
#   1. tailscale up のログイン URL (Tailscale アカウントが無ければここで作る)
#   2. serve の初回で tailnet の HTTPS が無効なら、その有効化 URL
#
# 冪等。tailscaled は設定が変わったときだけ再起動する (出先から tailnet 経由で
# 再実行しても、自分の接続を切らないため)。
set -euo pipefail

sudo pacman -S --needed --noconfirm tailscale >/dev/null

# パッケージの backup 対象なので、pacman の更新で上書きされない。
want='# scripts/host/setup-tailscale.sh が管理する
PORT="41641"
FLAGS="--tun=userspace-networking"'
if [ "$(cat /etc/default/tailscaled 2>/dev/null)" != "$want" ]; then
  printf '%s\n' "$want" | sudo install -Dm644 /dev/stdin /etc/default/tailscaled
  sudo systemctl restart tailscaled
fi
sudo systemctl enable --now tailscaled

sudo tailscale up
sudo tailscale serve --bg http://127.0.0.1:80
tailscale serve status

# サーバーの鍵が期限切れ (既定 180 日) になると tailnet から外れ、出先からは
# 直せない。期限は管理画面でしか無効にできないので、残っていれば知らせる。
if tailscale status --json | python3 -c '
import json, sys
sys.exit(0 if json.load(sys.stdin)["Self"].get("KeyExpiry") else 1)'; then
  echo
  echo "!! このマシンの鍵に有効期限が残っています。"
  echo "   https://login.tailscale.com/admin/machines で $(tailscale status --json |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["Self"]["HostName"])')"
  echo "   の「…」→ Disable key expiry を選んでください。"
fi

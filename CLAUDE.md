# ebishrimp-server

室内IoTセンシング基盤のモノレポ。正典は docs/iot-sensing-platform-spec-v1.md(確定判断が多数あるので変更前に必読)。

## コマンド

- `cargo build --release --workspace` / `cargo test --workspace` — Rust 3サービス (services/)
- `make lint` — fmt --check + clippy -D warnings
- `docker compose up -d --build` — ローカルE2E (rumqttd/ClickHouse/ingester/dashboard/mock)
- `bash scripts/deploy-local.sh` — k3sへのローカルデプロイ (GitHub不要版)
- `bash scripts/mock-e2e-check.sh` — ClickHouseの行数確認
- `bash scripts/host/setup-mdns.sh` — ホストで Ingress の `.local` 名 (dashboard.local 等) を avahi で IPv4/IPv6 両方 mDNS 広告し、IPv6 の :80 を traefik (IPv4 のみ) へ中継する。名前を増やすときは ALIASES に追加して再実行。systemd-resolved の mDNS は衝突回避のため無効化している
- `bash scripts/host/setup-ipv6-guard.sh` — ホストへの IPv6 の新規接続を LAN (オンリンクのプレフィックスとリンクローカル) からのみに制限する nftables テーブル `ip6 lan_guard`。プレフィックス変更にはタイマー (5分) で追従。外部公開したいポートは `PUBLIC_TCP_PORTS="25565"` のように指定して再実行

## 重要な確定事項 (spec より)

- タイムスタンプは ingester 受信時刻。デバイスは時刻を持たない
- MQTT は静的認証のみ。TLS はやらない (恒久判断)。補償策は NetworkPolicy
- バッチは 60秒 or 5000行の早い方。クラッシュ時最大1分欠損は許容
- トピック: `sensors/<room>/<device_id>[/status]`。ペイロードはJSONまとめ売り
- 縦持ち sensor_raw が既定。AS7341 (f1_415キーで判定) と status のみ専用テーブル

## 罠

- rumqttd 0.20.0 は rust:1.81 でしかビルドできない (metrics crate の借用エラーが新rustcでハードエラー)
- k8s の Secret は全て SOPS+age 暗号化 (`*.enc.yaml`)。鍵は ~/.config/sops/age/keys.txt
- clickhouse-schema Job は immutable。スキーマ変更時は Job を削除して再適用
- ClickHouse は users.d 直下しか読まない (サブディレクトリ不可)
- CHI の `configuration.files` で system ログに `<ttl>` を書くと ClickHouse が起動不能になる (Code: 36)。operator が同テーブルに `<engine>...TTL...</engine>` を生成済みのため。保持期間を変えたいなら engine 句の中に書く
- **マニフェストの `--dry-run=server` は CHI のスキーマしか見ない**。operator が生成する ClickHouse 設定の妥当性は Pod を起動するまで分からない。CHI を触ったら reconcile 後に Pod が Ready になるまで必ず確認すること
- マニフェスト変更時は `make lint-manifests` (CI: manifests.yml)
- rumqttd は publish された全メッセージを**トピックフィルタごと**のコミットログにメモリ保持し、`max_segment_size × max_segment_count` を超えるまで捨てない (購読者が消えたフィルタも残る)。upstream 既定の 100MiB×10 は Pod の上限 512Mi を超えており月1回 OOMKilled していた。値を変えるなら `ログが残る全フィルタ数 (購読者が切断済みのものも含む) × size × count < メモリ上限 − ログ以外の使用量 (実測約20 MiB) − 安全余裕` を保つこと。現行の 10 MiB/フィルタなら25本程度まで
- 負荷試験で `mosquitto_pub -l -q 1` に高速で流すと大半が届かずに捨てられる。速度を抑えて流し、console `/router` (ログに出力される) の `total_publishes` で届いた件数を必ず確認すること
- ホストで `nftables.service` を有効化しないこと。Arch 既定の `/etc/nftables.conf` は `flush ruleset` で k3s / Docker のルールを消す。IPv6 の壁は専用ユニット `ipv6-lan-guard` が自分のテーブルだけを入れ替える
- Arch でカーネルを更新して再起動していないと、稼働中カーネルのモジュールがディスクから消えており、未ロードのモジュールは使えない (nft の `ct state` が読めず、ファイアウォールの投入が失敗した)。`uname -r` と `/lib/modules/` を比べて確認する
- MQTT パスワード (`apps/rumqttd/secret-auth.enc.yaml`) をローテーションしたら、rumqttd / ingester / mock-publisher の3つの Deployment の `ebishrimp.io/credential-generation` を**同じコミットで**増やすこと。Secret 単独の変更では Pod がロールせず、後で1つだけ再起動したときに新旧が混在して認証が壊れる

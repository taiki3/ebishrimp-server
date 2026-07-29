# ebishrimp-server

室内IoTセンシング基盤のモノレポ。正典は docs/iot-sensing-platform-spec-v1.md(確定判断が多数あるので変更前に必読)。

## コマンド

- `cargo build --release --workspace` / `cargo test --workspace` — Rust 3サービス (services/)
- `make lint` — fmt --check + clippy -D warnings
- `docker compose up -d --build` — ローカルE2E (rumqttd/ClickHouse/ingester/dashboard/mock)
- `bash scripts/deploy-local.sh` — k3sへのローカルデプロイ (GitHub不要版)
- `bash scripts/mock-e2e-check.sh` — ClickHouseの行数確認

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

# ebishrimp-server — 室内IoTセンシング基盤 (Phase 1)

室内センシングデータ (10秒間隔) を溜め込み、自作ダッシュボードで可視化する基盤。
仕様書: [docs/iot-sensing-platform-spec-v1.md](docs/iot-sensing-platform-spec-v1.md)

```
mock-publisher ──MQTT(QoS1)──▶ rumqttd ──▶ ingester ──batch──▶ ClickHouse ──▶ dashboard
 (Phase 2: ESP32-C6)          (自前build)  (Rust/tokio)        (operator管理)   (SSR+htmx+ECharts)
```

## モノレポ構成

| パス | 内容 |
|---|---|
| `services/ingester` | MQTT→ClickHouse 取り込み (Rust / tokio / rumqttc / clickhouse crate) |
| `services/dashboard` | ダッシュボード3画面 (SSR + htmx + Apache ECharts) |
| `services/mock-publisher` | モックセンサー群 (3部屋×2台、AS7341含む、Last Will付き) |
| `docker/rumqttd` | rumqttd を上流タグからビルドする Dockerfile (公式イメージは古く panic するため) |
| `apps/` | k8s マニフェスト (rumqttd / clickhouse+スキーマ / ingester / dashboard) |
| `infrastructure/` | clickhouse-operator・kube-prometheus-stack (HelmRelease) ・namespace |
| `clusters/n150/` | Flux エントリポイント (Kustomization CR, SOPS復号設定) |
| `scripts/` | ホストセットアップ・デプロイ・E2E確認 |
| `.github/workflows/` | CI (fmt/clippy/test) とイメージビルド (GHCR) |

## ローカル開発 (docker compose)

```bash
docker compose up -d --build
# ダッシュボード: http://localhost:8080
# MQTT: localhost:1883 (dev認証: docker/rumqttd/rumqttd-dev.toml)
# ClickHouse: http://localhost:8123 (dev認証: docker/clickhouse/users.d/)
```

## k3s デプロイ

```bash
sudo bash scripts/setup-host.sh   # pacman + k3s (single node)
bash scripts/deploy-local.sh      # イメージbuild→k3s取込→flux install→全マニフェスト適用
bash scripts/mock-e2e-check.sh    # ClickHouse に行が入っているか確認
```

ダッシュボードは Traefik Ingress で `dashboard.local` に公開。閲覧する端末の
`/etc/hosts` に `<ノードIP> dashboard.local` を追加する。

## GitOps (Flux) 本運用

GitHub にリポジトリを作成後:

```bash
export GITHUB_TOKEN=<PAT>
bash scripts/bootstrap-flux.sh
```

以降、k3s と Flux 自身以外の全リソースは Git push でのみ反映される
(`clusters/n150/` がエントリポイント)。手動 `kubectl apply` はデバッグ用途のみ。

## Secrets (SOPS + age)

- 鍵: `~/.config/sops/age/keys.txt` (**Gitに入れない。バックアップ必須**)
- `*.enc.yaml` は SOPS で暗号化済み。編集: `sops apps/ingester/secret-env.enc.yaml`
- クラスタ側復号: `flux-system/sops-age` Secret (bootstrap-flux.sh が作成)

## MQTT 契約 (デバイス側)

```
sensors/<room>/<device_id>          測定値 JSON (10秒, QoS1)  例 {"temperature":25.3,"humidity":48.2,"co2":640}
sensors/<room>/<device_id>/status   死活 (起動時+5分毎, retain LWT {"online":0})
```

タイムスタンプはペイロードに含めない (ingester 受信時刻が authority)。

## 検証項目 (Phase 1 完了条件)

仕様書 §9 参照。`scripts/mock-e2e-check.sh` と `kubectl -n iot get pods` で消化する。

## 実バージョン記録欄

- k3s: `v1.36.2+k3s1` (2026-07-21 構築)
- rumqttd: `0.20.0` (rust:1.81 でビルド。新しめの rustc では依存 metrics crate がコンパイル不能)

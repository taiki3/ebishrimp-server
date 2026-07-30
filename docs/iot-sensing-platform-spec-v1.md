# 室内IoTセンシング基盤 構築仕様書 v1

**Phase 1: シングルノード一括構築**

| 項目 | 内容 |
|---|---|
| 版 | v1.0 (2026-07-20) |
| フェーズ | Phase 1 — N150 シングルノードで全コンポーネントを一気に構築し、E2Eでデータが流れる状態まで |
| 対象読者 | 構築者本人 (動画収録の台本ベースを兼ねる) |

---

## 1. 目的とスコープ

### 目的

室内センシングデータ (10秒間隔) を溜め込み、自作ダッシュボードで可視化する基盤を、N150 ミニPC 1台の上に宣言的 (GitOps) に構築する。モックpublisherを使い、MQTT → ingester → ClickHouse → ダッシュボード表示までのE2E疎通を完了条件とする。

### スコープ内

- Arch Linux + k3s (シングルノード) のセットアップ
- Flux による GitOps 管理 (以降の全リソースは Git 経由で反映)
- rumqttd (MQTTブローカー、自前ビルドイメージ)
- ClickHouse (Altinity clickhouse-operator、スキーマ投入込み)
- 自作 ingester (Rust / tokio)
- 自作ダッシュボード (Topcoat + Apache ECharts)
- 監視 (kube-prometheus-stack)
- モックpublisher によるE2E検証

### スコープ外 (Phase 2 以降)

- **ESP32-C6 ファームウェア** (本仕様ではモックpublisherで代替。トピック/ペイロード仕様のみ本書で確定)
- HX370 マシンの agent 参加、ワークロード移動
- Synology NAS + RustFS による階層ストレージ (S3 cold tier)
- Longhorn
- rumqttd の TLS / mTLS (Phase 1〜恒久的に静的認証のみの方針)

---

## 2. 全体構成

### データフロー

```
モックpublisher (Phase 2で ESP32-C6 に置換)
      │  MQTT publish (10秒ごと, QoS 1)
      ▼
rumqttd  ─ Pod (自前ビルドイメージ)
      │  subscribe: sensors/#
      ▼
ingester ─ Pod (Rust / tokio / rumqttc / clickhouse crate)
      │  1分 or N行 の二段バッチで bulk insert
      ▼
ClickHouse ─ clickhouse-operator 管理
      │  MergeTree (生) → MV (1分) → MV (日次)
      ▼
Topcoat ダッシュボード ─ Pod (SSR + htmx + ECharts)
```

### ノード構成 (Phase 1)

| マシン | 役割 |
|---|---|
| N150 ミニPC (4C / 16GB) | k3s server (control plane + ワーカー兼任)。全ワークロードが載る |

- k3s のデータストアは組み込み SQLite (シングルノードのため etcd 不要)
- PV は k3s 同梱の local-path-provisioner
- HA ではないことを自覚した構成。マシン再起動後に Flux が全リソースを復元できることを検証項目とする

---

## 3. OS / ホストセットアップ

| 項目 | 指定 |
|---|---|
| OS | Arch Linux (最新) |
| ファイルシステム | 任意 (btrfs 採用時はスナップショット設定は本仕様のスコープ外) |
| ホスト名 | `n150` (例。クラスタ内ノード名になる) |
| 固定IP | 必須。ルーターのDHCP予約 or 静的設定 (例: `192.168.x.10`) |
| 必須パッケージ | `curl`, `git`, `age`, `sops`, `kubectl`, `flux-bin` (AUR), `mosquitto` (mosquitto_pub/sub を疎通確認に使用) |
| swap | 8GB 程度確保推奨 (16GBメモリで監視スタック同居のため) |
| 時刻同期 | systemd-timesyncd 有効化 (ingester がタイムスタンプ authority になるため必須) |

**注意**: Arch はローリングリリースのため、k3s 構築後はカーネル更新のタイミングに注意 (更新→再起動で検証項目の「再起動復元」を兼ねられる)。

---

## 4. k3s セットアップ

```bash
curl -sfL https://get.k3s.io | sh -s - server \
  --write-kubeconfig-mode 644
```

- Traefik / local-path-provisioner / metrics-server は k3s 同梱のデフォルトのまま有効
- バージョンは構築時の最新安定版を使用し、**構築後に本書へ実バージョンを追記する** (記入欄: `k3s v____`)
- kubeconfig を `~/.kube/config` に配置し、`kubectl get nodes` で Ready を確認

---

## 5. GitOps (Flux)

### 方針

**k3s と Flux 自身以外のすべてのリソースは Git 経由でのみ反映する。** `kubectl apply` の手動実行は検証・デバッグ用途に限る。

### ブートストラップ

```bash
flux bootstrap github \
  --owner=<github-user> \
  --repository=iot-platform \
  --branch=main \
  --path=clusters/n150 \
  --personal
```

### リポジトリ構成

```
iot-platform/
├── clusters/n150/            # Flux エントリポイント (Kustomization 群)
├── infrastructure/
│   ├── clickhouse-operator/  # Altinity operator (HelmRelease)
│   ├── monitoring/           # kube-prometheus-stack (HelmRelease)
│   └── namespaces.yaml
├── apps/
│   ├── rumqttd/              # Deployment, Service, ConfigMap, NetworkPolicy
│   ├── clickhouse/           # ClickHouseInstallation CR + スキーマ投入 Job
│   ├── ingester/             # Deployment, Secret参照, ServiceMonitor
│   └── dashboard/            # Topcoat Deployment, Service, Ingress
└── .sops.yaml
```

### Secrets

- SOPS + age で暗号化した Secret を Git にコミット。Flux の kustomize-controller に age 秘密鍵を渡して復号
- 対象: rumqttd の静的認証パスワード、ClickHouse の ingester 用ユーザーパスワード

---

## 6. コンポーネント仕様

### 6.1 rumqttd (MQTTブローカー)

| 項目 | 指定 |
|---|---|
| イメージ | **自前ビルド必須** (Docker Hub 公式イメージは古く config 指定で panic する既知報告あり)。GitHub Actions で bytebeamio/rumqtt の最新タグからビルドし GHCR へ push、Flux でデプロイ |
| 認証 | 静的ユーザー名/パスワードのみ (rumqttd.toml のリスナー設定)。TLS/mTLS はやらない (宅内LAN の脅威モデルに対し費用対効果が合わないという確定判断) |
| ポート公開 | MQTT 1883 のみ LAN へ露出 (Service type: NodePort または LoadBalancer)。コンソール/メトリクスポートはクラスタ内限定 |
| NetworkPolicy | ingester Pod からのみブローカーのクラスタ内ポートへ接続可とする (認証が薄いぶん到達経路を絞る補償策) |
| 監視 | Prometheus メトリクスを ServiceMonitor で収集 |
| レプリカ | 1 (ブローカーHAはk8sの自己修復に委ねる。Pod再起動時、デバイスは再接続ループで復帰、QoS 1 で未ack分再送) |
| 設定 | ConfigMap で rumqttd.toml を注入。max_payload_size 等はデフォルトで開始 |

### 6.2 ClickHouse

| 項目 | 指定 |
|---|---|
| デプロイ | Altinity clickhouse-operator。`ClickHouseInstallation` CR で 1 レプリカ |
| ストレージ | local-path PV (Phase 1)。サイズ 50Gi で開始 |
| ユーザー | `ingester` (INSERT + SELECT)、`dashboard` (SELECT のみ)。パスワードは SOPS Secret |
| スキーマ投入 | DDL を ConfigMap 化し、初期化 Job (clickhouse-client) で投入。DDL ファイル自体も Git 管理 |

#### スキーマ DDL (Phase 1 版)

**注意**: 確定スキーマ (Notion 参照) から `TO VOLUME 'cold'` 句と `storage_policy` を除いた版。RustFS 導入時 (Phase 2) に `ALTER TABLE ... MODIFY TTL` で階層化を追加する。

```sql
-- ① 縦持ち共通テーブル: 1行 = 1デバイス・1メトリクス・1時点
CREATE TABLE sensor_raw (
    ts        DateTime('Asia/Tokyo'),
    room      LowCardinality(String),
    device_id LowCardinality(String),
    metric    LowCardinality(String),
    value     Float64
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (metric, device_id, ts)
TTL ts + INTERVAL 5 YEAR DELETE;

-- ② 1分集計 MV
CREATE MATERIALIZED VIEW sensor_1m
ENGINE = AggregatingMergeTree
ORDER BY (metric, device_id, ts_min) AS
SELECT toStartOfMinute(ts) AS ts_min, room, device_id, metric,
       avgState(value) AS avg_v, minState(value) AS min_v, maxState(value) AS max_v
FROM sensor_raw
GROUP BY ts_min, room, device_id, metric;

-- ③ 日次集計 MV
CREATE MATERIALIZED VIEW sensor_1d
ENGINE = AggregatingMergeTree
ORDER BY (metric, device_id, ts_day) AS
SELECT toStartOfDay(ts) AS ts_day, room, device_id, metric,
       avgState(value) AS avg_v, minState(value) AS min_v, maxState(value) AS max_v
FROM sensor_raw
GROUP BY ts_day, room, device_id, metric;

-- ④ 測定イベント型デバイス専用 (AS7341, 横持ち)
CREATE TABLE as7341_raw (
    ts        DateTime('Asia/Tokyo'),
    device_id LowCardinality(String),
    gain      UInt8,
    atime     UInt16,
    astep     UInt16,
    f1_415 UInt16, f2_445 UInt16, f3_480 UInt16, f4_515 UInt16,
    f5_555 UInt16, f6_590 UInt16, f7_630 UInt16, f8_680 UInt16,
    clear UInt16, nir UInt16
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (device_id, ts);

-- ⑤ デバイス死活
CREATE TABLE device_status (
    ts        DateTime('Asia/Tokyo'),
    device_id LowCardinality(String),
    rssi      Int16,
    uptime_s  UInt32,
    online    Bool
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (device_id, ts)
TTL ts + INTERVAL 1 YEAR DELETE;
```

**設計原則**: 単独時系列のセンサーは縦持ち共通テーブル (センサー追加時 DDL 変更ゼロ)。測定イベント型 (チャネル群が不可分 + 測定条件が行に付随) は専用横持ちテーブル。

### 6.3 ingester (自作 Rust サービス)

| 項目 | 指定 |
|---|---|
| ランタイム | tokio |
| MQTT クライアント | rumqttc。`sensors/#` を QoS 1 で購読 |
| ClickHouse クライアント | clickhouse crate |
| タイムスタンプ | **ingester の受信時刻を採用** (デバイス側は時刻を持たない。確定判断) |
| バッチ | **二段構え: 60秒経過 or 5,000行到達 のいずれか早い方でフラッシュ** (tokio::select でタイマー + チャネルを待つ)。バッチ内は `(metric, device_id, ts)` 順にソートしてから insert |
| 振り分け | トピック末尾 `/status` → `device_status`、ペイロードに `f1_415` キーあり → `as7341_raw`、それ以外 → `sensor_raw` に JSON をメトリクス数ぶんの行に展開 |
| 耐障害 | ブローカー切断時: 指数バックオフで再接続 → 再subscribe。ClickHouse insert 失敗時: リトライ (上限あり)、超過分は捨ててエラーカウンタを increment。クラッシュ時の最大1分欠損は仕様として許容 (確定判断) |
| 監視 | `/metrics` (Prometheus形式) を expose: 受信メッセージ数、insert 行数、バッチフラッシュ回数、エラー数。ServiceMonitor で収集 |
| 未知ペイロード | JSON パース不能・数値でない値はスキップし、警告ログ + エラーカウンタ。落ちない |
| イメージ | GitHub Actions でビルド → GHCR → Flux (rumqttd と同一パイプライン構成) |

### 6.4 ダッシュボード (Topcoat)

| 項目 | 指定 |
|---|---|
| フレームワーク | tokio-rs/topcoat (超初期段階。破壊的変更前提で Cargo.lock を固定し、更新は意図的に行う) |
| チャート | **Apache ECharts** (確定判断)。チャート構成データ (系列・軸・目盛) は Rust 側で option JSON として全部組み、SSR した HTML に埋めて返す。描画の最後の一歩だけ ECharts。charming (Rust製ECharts ラッパー) の採用は実装時に判断 |
| 更新 | htmx ポーリング (10〜30秒) でチャート部分をスワップ。スワップ内 script は実行されるため ECharts 再描画と両立 |
| クエリ | 直近24h は `sensor_1m` (avgMerge 等で確定値化)、それより長期は `sensor_1d`、直近数分の生値は `sensor_raw`。`dashboard` ユーザー (SELECT のみ) で接続 |
| Phase 1 の画面 | ①メトリクス一覧 (metric × device の最新値グリッド) ②時系列チャート (metric/device/期間選択) ③デバイス死活パネル (device_status + Last Will 由来の online フラグ) の3画面まで |
| 公開 | Traefik Ingress で LAN 内に HTTP 公開 (`dashboard.local` 等の hosts 運用で可) |

### 6.5 監視

| 項目 | 指定 |
|---|---|
| スタック | kube-prometheus-stack (HelmRelease / Flux 管理) |
| メモリ条件 | 導入後に N150 の実メモリ使用を確認し、**合計 80% 超過なら** Grafana 無効化・retention 短縮等の軽量化、それでも厳しければ metrics-server + 自前 Prometheus 単体構成へ縮退 (未決事項として継続) |
| 収集対象 | ノード / k8s 標準に加え、rumqttd・ingester の ServiceMonitor |

---

## 7. MQTT トピック / ペイロード仕様 (デバイス側との契約)

ESP32 実装は Phase 2 だが、モックpublisher と ingester は本仕様に従う。

### 接続 (確定)

| 用途 | エンドポイント | プロトコル |
|---|---|---|
| ESP32 実機 | `192.168.10.25:1884` | **MQTT 5.0** |
| ingester / モックpublisher | `rumqttd:1883` (クラスタ内) | MQTT 3.1.1 |

認証はユーザー名 + パスワードの静的認証のみ。TLS は恒久的に不採用 (§1)、補償策は NetworkPolicy。

ESP32 側は `rust-mqtt` (no_std) を使う前提で、同クレートは **MQTT 5.0 のみ実装**しているため v5 リスナーを 1884 に別途用意している。rumqttd はリスナーごとにポートを分ける設計で 1883 に v4/v5 を相乗りできないが、router はリスナー間で共有されるため v5 で publish したメッセージは v4 で購読している ingester に届く (実測確認済み)。

### トピック (確定)

```
sensors/<room>/<device_id>          測定値 (JSON, 10秒ごと, QoS 1)
sensors/<room>/<device_id>/status   死活・低頻度 (起動時 + 5分ごと)。Last Will もここに設定
```

- 小文字統一、先頭 `/` なし、トピックに値を埋め込まない
- 方式は「デバイスごと1トピック + JSON まとめ売り」(メトリクス別トピック分割は不採用の確定判断)

### ペイロード

測定値 (キー = metric 名、値 = 数値。ブールは 0/1):

```json
{"temperature": 25.3, "humidity": 48.2, "co2": 640}
```

AS7341 (キー f1_415〜nir + 測定条件):

```json
{"gain": 8, "atime": 29, "astep": 599,
 "f1_415": 1023, "f2_445": 980, "f3_480": 1100, "f4_515": 1500,
 "f5_555": 1800, "f6_590": 1600, "f7_630": 1400, "f8_680": 1200,
 "clear": 4000, "nir": 800}
```

status:

```json
{"rssi": -61, "uptime_s": 86400, "online": 1}
```

Last Will: `{"online": 0}` を status トピックに設定 (retain 推奨)。

- **タイムスタンプはペイロードに含めない** (ingester 受信時刻を採用する確定判断)

### モックpublisher

Phase 1 の検証用に、上記仕様どおりのメッセージを吐く簡易スクリプト (Rust or Python + mosquitto_pub ループでも可) を用意する。3部屋 × 2デバイス程度、sin波 + ノイズで値を生成。

---

## 8. 構築手順 (章立て)

1. Arch インストール、固定IP、必須パッケージ、時刻同期
2. k3s server 導入、`kubectl get nodes` で Ready 確認
3. Flux bootstrap、リポジトリ構成の骨組み作成、SOPS + age 設定
4. rumqttd: GitHub Actions イメージビルド → Flux デプロイ → `mosquitto_pub/sub` で疎通
5. clickhouse-operator → ClickHouseInstallation → スキーマ投入 Job → `clickhouse-client` で確認
6. ingester デプロイ → モックpublisher 起動 → `sensor_raw` に行が増えることを確認
7. kube-prometheus-stack 導入、rumqttd / ingester のメトリクス確認、メモリ実測
8. Topcoat ダッシュボード (3画面) → LAN の別端末から閲覧
9. 検証項目の消化 (次章)

---

## 9. 検証項目 (Phase 1 完了条件)

- [ ] `mosquitto_sub -t 'sensors/#'` でモックpublisher の全メッセージが見える (認証込み)
- [ ] `sensor_raw` の行数がモックの publish 数と一致する (許容誤差: フラッシュ待ち1分ぶん)
- [ ] `sensor_1m` / `sensor_1d` が自動的に埋まる
- [ ] AS7341 形式のペイロードが `as7341_raw` に、status が `device_status` に振り分けられる
- [ ] モックpublisher を kill → Last Will により `device_status` に `online=0` が入り、ダッシュボードの死活パネルに反映される
- [ ] ダッシュボード3画面が LAN 内の別端末から閲覧でき、チャートがポーリングで更新される
- [ ] rumqttd Pod を delete → 自動再起動 → モックが再接続して欠損が QoS 1 再送の範囲に収まる
- [ ] ingester Pod を delete → 再起動後に取り込みが再開する
- [ ] **N150 を OS ごと再起動 → 全コンポーネントが人手なしで復旧し、データ取り込みが再開する** (Flux + k3s の宣言的管理の実証。動画のクライマックス)
- [ ] Git にマニフェスト変更を push → Flux が自動反映する (例: ingester のレプリカ数変更)
- [ ] メモリ使用量の実測値を記録 (監視スタック軽量化判断の材料)

---

## 10. Phase 2 以降 (本仕様のスコープ外、参考)

1. ESP32-C6 実機 (esp-hal + Embassy + rust-mqtt) をモックと差し替え
2. HX370 を k3s agent として参加、nodeSelector でワークロード移動
3. Synology NAS + RustFS → ClickHouse に storage_policy 追加、`ALTER TABLE ... MODIFY TTL` で cold 階層化。TTL 移動後のパーティションが S3 側から読めることの検証を最優先
4. clickhouse-backup の導入 (バックアップ先 RustFS)
5. (任意) Longhorn 導入 (2ノード化でレプリカ2に意味が出る)

## 11. 未決事項

- 監視スタックの最終構成 (フル kube-prometheus-stack か軽量構成か。§6.5 の実測後に判断)
- 実センサーの選定 (Phase 2)
- TTL 日数の本決め (生データ削除 5年 / cold 移行 90日 はいずれも仮置き)

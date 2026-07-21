# Phase 1 構築ログ — 室内IoTセンシング基盤をk3s上に一気に建てた記録

**実施日**: 2026-07-21(1セッションで完遂)
**環境**: taiki3-n100 — Intel N100 4C / 32GB RAM / Arch Linux (kernel 7.1.4-arch1-1) / 固定IP 192.168.10.25
**成果**: モノレポ実装 + ローカルE2E + k3sデプロイ + 検証項目11個中9個消化

> 仕様書 [iot-sensing-platform-spec-v1.md](iot-sensing-platform-spec-v1.md) を正典として、
> 「仕様書1枚から動くE2E基盤まで」を一気通貫でやった記録。動画の台本ベースを兼ねる。

---

## 1. 完成したもののダイジェスト

```
mock-publisher (6デバイス) ──MQTT QoS1──▶ rumqttd ──▶ ingester ──60s/5000行バッチ──▶ ClickHouse ──▶ dashboard
   Phase 2でESP32-C6に置換           自前ビルド     Rust/tokio                     operator管理      topcoat SSR
                                        │                │                            │                + htmx
                                        └─── Prometheus (kube-prometheus-stack) が両者を scrape ────┘  + ECharts
```

- **すべてk8sマニフェスト+Gitで宣言的に管理**(Secretは SOPS + age 暗号化でコミット可能)
- ダッシュボードは Traefik Ingress で `dashboard.local` として LAN 公開
- MQTT は LoadBalancer(klipper)でノードIP `192.168.10.25:1883` に露出、静的認証つき

### 最終的な稼働状態

```
$ kubectl -n iot get pods
chi-iot-main-0-0-0                1/1  Running     # ClickHouse (operator管理)
clickhouse-schema-bsjkk           0/1  Completed   # スキーマ投入Job
dashboard-xxx                     1/1  Running
ingester-xxx                      1/1  Running
mock-publisher-xxx                1/1  Running
rumqttd-xxx                       1/1  Running
```

---

## 2. リポジトリ構成(モノレポ)

| パス | 内容 |
|---|---|
| `services/ingester` | MQTT→ClickHouse 取り込み。rumqttc + clickhouse crate |
| `services/dashboard` | SSR ダッシュボード3画面。**topcoat 0.3.1** + htmx + Apache ECharts |
| `services/mock-publisher` | モックセンサー群(3部屋×2台、AS7341含む、Last Will付き) |
| `docker/rumqttd` | rumqttd をソースからビルドする Dockerfile + dev用設定 |
| `docker/clickhouse` | compose用のdevユーザー定義 |
| `apps/` | アプリのk8sマニフェスト(rumqttd / clickhouse / ingester / dashboard / mock) |
| `infrastructure/` | clickhouse-operator・kube-prometheus-stack の HelmRelease、namespace |
| `clusters/n150/` | Flux エントリポイント(Kustomization CR、SOPS復号設定) |
| `scripts/` | setup-host / deploy-local / bootstrap-flux / mock-e2e-check |
| `.github/workflows/` | CI(fmt/clippy/test)+ GHCR イメージビルド |
| `docker-compose.yml` | k3s と同型のローカルE2E開発環境 |

**ワークスペース戦略**: Rust 3サービスは1つの cargo workspace。イメージは各サービスの
Dockerfile がリポジトリルートをビルドコンテキストにして `cargo build -p <svc>` する方式。

---

## 3. タイムライン(実施順)

### 3.1 環境確認とツール導入

ホストには git / cargo / docker しか無かったので、root不要なものは `~/.local/bin` に直接導入:

- kubectl v1.36.2 / flux v2.9.2 / sops 3.10.2 / age 1.2.1(すべて静的バイナリをcurlで)
- k3s と mosquitto だけが root 必須 → 後述の sudo 問題を挟んで後半で導入

### 3.2 Rustサービス実装

**ingester**(仕様 §6.3 の確定判断をそのまま実装):

- タイムスタンプは**受信時刻**(`OffsetDateTime::now_utc()`)。デバイスは時刻を持たない
- 二段バッチ: `tokio::select!` で「60秒タイマー or sensor_raw 5000行到達」の早い方でフラッシュ
- 振り分け: トピック末尾 `/status` → `device_status` / ペイロードに `f1_415` → `as7341_raw` / それ以外 → JSONの数値キーを1行ずつ `sensor_raw` へ縦持ち展開
- insert失敗はリトライ3回→捨ててカウンタ increment(最大1分欠損は仕様として許容)
- `/metrics` は依存を増やさず AtomicU64 + 手書きテキストフォーマットで実装
- ユニットテスト6件(振り分け・LWT・bool→0/1・不正ペイロード拒否)

**mock-publisher**:

- 6デバイスがそれぞれ**独立したMQTT接続**を持つ(デバイスごとの Last Will を成立させるため)
- Last Will: `{"online": 0}` を status トピックに retain で設定 → プロセスkillで発火
- 値は sin波 + ノイズ。AS7341 は10チャネル+測定条件つきの横持ちペイロード

**dashboard**(並行でサブエージェントに委任):

- 仕様指定の **topcoat 0.3.1**(tokio-rs製、超初期段階)を実APIを調査した上で採用。
  `view!` マクロでSSR、`#[page]`/`#[component]`、`topcoat::serve` で任意アドレスにバインド
- ECharts の option JSON(系列・軸・目盛)は**全部Rust側で構築**し、SSRしたHTMLに埋め込む。
  描画の最後の一歩だけ ECharts(仕様の確定判断どおり)
- htmx ポーリング(10〜30秒)でフラグメントをスワップ。スワップ内 `<script>` で ECharts を dispose→再init
- 期間→テーブルの出し分け: 10m→`sensor_raw` / 1h・6h・24h→`sensor_1m`(avgMerge)/ 7d・30d→`sensor_1d`
- SQLはメトリクス名・デバイスIDを**必ず bind**(文字列連結しない)
- echarts.min.js / htmx.min.js はリポジトリにベンダリングし、バイナリに `include_bytes!` で埋込 → ランタイムイメージはバイナリ1個
- テスト25件、clippy 0警告

### 3.3 ClickHouse スキーマ

仕様 §6.2 の DDL を `IF NOT EXISTS` 付きの冪等版にして `apps/clickhouse/schema/01_schema.sql` に一本化。
compose では `/docker-entrypoint-initdb.d` マウント、k8s では ConfigMap + 投入Job が同じファイルを使う。

- `sensor_raw`(縦持ち、月パーティション、TTL 5年)
- `sensor_1m` / `sensor_1d`(AggregatingMergeTree の MV)
- `as7341_raw`(横持ち・測定条件つき)/ `device_status`(TTL 1年)

### 3.4 Secrets(SOPS + age)

- age鍵を生成(`~/.config/sops/age/keys.txt`)し、`.sops.yaml` に公開鍵を設定
- 本番系パスワードは openssl rand で生成し、`apps/*/secret-*.enc.yaml` として**暗号化した状態でGitにコミット**
- dev系(compose)は意図的に平文の固定値(`docker/` 配下)で分離
- rumqttd は設定ファイル自体にパスワードが入るため、**rumqttd.toml 丸ごとSecret**にして暗号化

### 3.5 ローカルE2E(docker compose)

k3s に行く前に、同型構成の compose で先に疎通を取った。ここで **rumqttd がビルド不能**という
最初の罠を踏む(§4-1)。解決後の結果:

- `sensor_raw` 312行 / `as7341_raw` 24行 / `device_status` 6行が流入
- MV が自動集計(`sensor_1m` 104行 / `sensor_1d` 52行)
- dashboard ユーザーで INSERT を打つと `READONLY` エラー(権限分離の確認)
- ダッシュボード3画面すべて 200、実デバイスIDがレンダリングされる
- ingester メトリクス: received 198 / inserted 432 / parse_errors 0 / dropped 0

### 3.6 k3s 構築とデプロイ

```bash
curl -sfL https://get.k3s.io | sudo sh -s - server --write-kubeconfig-mode 644
# → k3s v1.36.2+k3s1、ノード Ready まで数十秒
```

- `flux install` でコントローラのみ導入(GitHub接続は後日。HelmRelease の reconcile はこれで動く)
- `kubectl apply -k infrastructure` → clickhouse-operator 0.27.1 と kube-prometheus-stack 87.17.0 が Helm で入る
- イメージは **GHCRに押さずローカルビルド→`docker save | k3s ctr images import`** で持ち込み
  (マニフェストのイメージ名はGHCRのまま、`imagePullPolicy: IfNotPresent` で解決)
- Secretは「リポジトリを一時ディレクトリにコピー→ `sops -d -i` で復号→ `kubectl apply -k`」方式
  (`scripts/deploy-local.sh` に自動化)

ここで operator が CHI を無視する問題(§4-2)、プローブのログ汚染(§4-3)、
Prometheus 3.x の scrape 拒否(§4-4)を順に解決。

### 3.7 検証(spec §9)

詳細は [phase1-verification.md](phase1-verification.md)。ハイライト:

- **Last Will テスト**: `kubectl delete pod --grace-period=0 --force` で mock を強制kill
  → 全6デバイスの `online=false` が `device_status` に記録され、新Podの再接続で `true` に復帰
- **自己修復テスト**: rumqttd / ingester の Pod を削除 → 自動再起動 → mock 6台が再接続し、
  行数が676→728と取り込み継続を確認
- **メモリ実測**: IoTスタック分は ClickHouse 307Mi + Prometheus 375Mi + Grafana 318Mi + アプリ計~10Mi。
  監視スタックの軽量化は**不要**と判断(仕様 §6.5 の未決事項に決着)

---

## 4. 踏んだ罠と対処(動画の見どころ候補)

### 4-1. rumqttd 0.20.0 が最新Rustでビルドできない

- **症状**: `cargo build -p rumqttd` が依存 `metrics` crate の E0521(borrowed data escapes)で失敗
- **原因**: 昔は警告だった借用パターンが新しい rustc でハードエラー化(rust#141402)。
  公式Dockerイメージは古くて config 指定で panic する既知問題があるため自前ビルド一択
- **対処**: ビルダーをタグ当時のツールチェーン `rust:1.81-bookworm` に固定
- **教訓**: 「最新タグをソースビルド」戦略はツールチェーンのバージョンも一緒にピン留めする

### 4-2. clickhouse-operator が CHI を完全に無視する

- **症状**: `ClickHouseInstallation` を作っても STATUS が空のまま。operator ログにも一切登場しない
- **切り分け**: operator 自身の namespace に最小 CHI を置いたら**即 reconcile された**
  → 「iot namespace を見ていない」ことが確定
- **原因**: チャート 0.27.1 のデフォルトは `watch.namespaces.include: []` =
  **operator自身のnamespaceのみ監視**(values.yaml のコメントに明記されていた)
- **対処**: HelmRelease の values に `include: [".*"]` を追加
- **教訓**: 「動かない」ときは最小再現をoperatorの足元に置くと watch 範囲の問題を数分で切り分けられる

### 4-3. kubelet の TCP プローブが rumqttd の ERROR ログを量産

- **症状**: 5秒ごとに `Error while handling MQTT connect packet ... connection closed by peer`
- **原因**: tcpSocket プローブは「TCP接続して即切断」するため、MQTTブローカーには
  不正な接続試行に見える
- **対処**: readiness/liveness を console ポート(3030、HTTP)の tcpSocket に変更

### 4-4. Prometheus 3.x が rumqttd の /metrics を拒否する

- **症状**: ターゲットが down。エラーは
  `non-compliant scrape target sending blank Content-Type and no fallback_scrape_protocol specified`
- **原因**: rumqttd は Content-Type ヘッダを返さず、Prometheus 3.x はデフォルトで拒否する
- **対処**: ServiceMonitor に `fallbackScrapeProtocol: PrometheusText0.0.4` を追加 → up に

### 4-5. ポート1883の二重化(compose × k3s)

- **症状**: compose の docker-proxy と k3s の svclb(klipper LoadBalancer)が同じノードで 1883 を扱う。
  hostPort は iptables ベースなので bind 衝突エラーは**出ない**まま経路が曖昧になる
- **対処**: k3s 検証完了後に compose を停止。以後、ローカル開発で compose を上げるときは
  k3s 側と同時起動しない運用ルールに
- **教訓**: 「エラーが出ない衝突」が一番怖い。LoadBalancer/hostPort と docker publish の同居は要注意

### 4-6. その他小ネタ

- `docker build ... | tail` は **tail の終了コードしか見えない**ため、失敗ビルドを「成功」と誤認した
  (rumqttd の件はこれで一度見逃した)。パイプで包むなら `pipefail` 必須
- `.dockerignore` を忘れて `target/` がビルドコンテキストに入り、コンテキスト転送が数十MB級に膨張
- sudoers は**最後にマッチしたルールが勝つ**。NOPASSWD 行の後に `%wheel ALL=(ALL:ALL) ALL` が
  評価されて無効化されていた → `/etc/sudoers.d/99-*`(読み込み順が最後)に置いて解決
- ClickHouse は `users.d` 直下しか読まない(サブディレクトリにマウントしても無視される)
- SOPS暗号化ファイルを含むディレクトリに `kubectl apply -k` を直接打たない
  (暗号化Secretがそのまま適用されようとする)。必ず「コピー→復号→apply」の手順を踏む

---

## 5. アーキテクチャ上の確定判断(仕様から引き継いだもの)

実装中に揺らがせなかったポイント。動画で「なぜこうしたか」を語る材料:

1. **タイムスタンプはingester受信時刻** — デバイスに時計を持たせない。NTPはホスト側の責務
2. **TLSはやらない(恒久)** — 宅内LANの脅威モデルに対して費用対効果が合わない。
   代わりに NetworkPolicy で「1883は全許可 / console・metricsポートは monitoring namespace のみ」
3. **縦持ち sensor_raw が既定** — センサー追加時にDDL変更ゼロ。
   横持ち専用テーブルは「チャネル群が不可分+測定条件が行に付随する」AS7341 と status のみ
4. **バッチは60秒 or 5000行** — クラッシュ時の最大1分欠損は明示的に許容した仕様
5. **チャートのoption JSONはRustで組む** — フロントは「描画の最後の一歩」だけ。
   SSR + htmx でSPAを持たない

---

## 6. 使ったバージョン一覧

| コンポーネント | バージョン |
|---|---|
| k3s | v1.36.2+k3s1 |
| Flux CLI / controllers | v2.9.2 |
| clickhouse-operator (chart) | 0.27.1 |
| kube-prometheus-stack (chart) | 87.17.0 |
| ClickHouse | 25.3 |
| rumqttd | 0.20.0(rust:1.81 でビルド) |
| topcoat | =0.3.1(ピン留め) |
| rumqttc / clickhouse crate | 0.25.1 / 0.15.1 |
| ホストRust | 1.97.1 |
| sops / age | 3.10.2 / 1.2.1 |

---

## 7. 残タスク(次回の撮れ高)

1. **OS再起動テスト(検証項目9、クライマックス)** — 電源断からの全自動復旧。
   k3s(systemd)→ Flux/operator → 全Pod → データ取り込み再開、を人手ゼロで
2. **GitOps化(検証項目10)** — GitHub リポジトリ作成 → push → `scripts/bootstrap-flux.sh`
   (flux bootstrap + sops-age Secret投入)。以降は「git push すると環境が変わる」画が撮れる
3. GHCR への実push(CI は書いてあるので GitHub 接続だけで動く)
4. age 鍵のバックアップ(これを失うと暗号化Secretが誰にも読めなくなる)
5. Phase 2: ESP32-C6 実機、HX370 の agent 参加、RustFS cold tier、clickhouse-backup

---

## 8. ふりかえり

- **仕様書に確定判断を書き切っておいたのが効いた**。実装中の意思決定がほぼゼロで、
  詰まったのは全部「外部コンポーネントの想定外の挙動」(§4)だった
- compose で先にE2Eを取ってから k3s に行く二段構えは正解。アプリ起因とk8s起因の問題を
  分離できたので、k3s 側で追ったのは純粋にインフラの問題だけ
- 未知のフレームワーク(topcoat)は「crates のソースを読んでAPIを確定させてから書く」
  方針でフォールバック無しの一発採用に成功

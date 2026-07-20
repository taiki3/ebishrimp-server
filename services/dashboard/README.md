# dashboard

センサーデータの SSR ダッシュボード。仕様 (§6.4) どおり
[tokio-rs/topcoat](https://crates.io/crates/topcoat) v0.3.1 で実装している
(バージョンは `=0.3.1` で固定。超初期段階のため更新は意図的に行う)。
topcoat のルーティング (`#[page]` / `#[route]` / `#[layout]`)・`view!` テンプレート・
app context をそのまま利用でき、fallback (axum) は不要だった。

- チャートは Apache ECharts。option JSON (系列・軸・目盛) はすべて Rust 側で組み立て、
  SSR した HTML のインライン `<script>` に埋め込む。
- 更新は htmx ポーリング (最新値/死活 10 秒、チャート 30 秒) でフラグメントをスワップ。
  スワップ内の `<script>` が実行されるため ECharts は都度 dispose → init し直す。
- ClickHouse へは HTTP (`clickhouse` crate v0.15, 読み取り専用 `dashboard` ユーザー) で接続。
  `metric` / `device` はすべて `query().bind()` でバインドし、SQL に文字列連結しない。
- htmx / ECharts / CSS はバイナリに埋め込み (`include_bytes!`)、`/static/` から自己ホスト。CDN 不使用。
- ClickHouse に接続できない場合はフラグメント内にエラーボックスを描画する (500 にはしない)。

## ルート

| パス | 内容 |
|---|---|
| `GET /` | 最新値グリッド (metric × device、直近1時間の `argMax`) |
| `GET /chart?metric=&device=<id\|all>&range=<10m\|1h\|6h\|24h\|7d\|30d>` | 時系列チャート。10m→`sensor_raw`、1h/6h/24h→`sensor_1m` (avgMerge、単一デバイス時は min/max バンド付き)、7d/30d→`sensor_1d` |
| `GET /devices` | デバイス死活 (online フラグ false または 15 分以上未受信でオフライン) |
| `GET /fragments/overview` `/fragments/chart` `/fragments/devices` | htmx ポーリング用フラグメント |
| `GET /healthz` | ヘルスチェック (`ok`) |
| `GET /static/echarts.min.js` `/static/htmx.min.js` | 埋め込み静的アセット |

## 環境変数

| 変数 | デフォルト |
|---|---|
| `CLICKHOUSE_URL` | `http://localhost:8123` |
| `CLICKHOUSE_USER` | `dashboard` |
| `CLICKHOUSE_PASSWORD` | (空) |
| `CLICKHOUSE_DATABASE` | `default` |
| `LISTEN_ADDR` | `0.0.0.0:8080` |

## 開発

```sh
cargo build --release -p dashboard
cargo test -p dashboard
# イメージビルド (コンテキストはリポジトリルート)
docker build -f services/dashboard/Dockerfile .
```

備考: チャートの時刻軸/ツールチップはブラウザのローカルタイムゾーンで描画される
(LAN 内 JST 前提)。テーブル表示の時刻はサーバー側で JST (+09:00) に整形している。

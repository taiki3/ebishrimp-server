# Phase 1 検証記録 (spec §9)

実施日: 2026-07-21 / 環境: taiki3-n100 (N100 4C/32GB, Arch Linux, k3s v1.36.2+k3s1)

| # | 項目 | 結果 | 備考 |
|---|---|---|---|
| 1 | `mosquitto_sub -t 'sensors/#'` で全メッセージが見える (認証込み) | ✅ | LAN側 (LoadBalancer 192.168.10.25:1883)。誤パスワードは接続拒否も確認 |
| 2 | `sensor_raw` 行数がpublish数と一致 | ✅ | 定常増加を確認 (フラッシュ待ち≦1分の誤差内)。parse_errors=0, rows_dropped=0 |
| 3 | `sensor_1m` / `sensor_1d` が自動で埋まる | ✅ | MV稼働 (compose/k3s両方で確認) |
| 4 | AS7341→`as7341_raw`、status→`device_status` 振り分け | ✅ | |
| 5 | mock kill → Last Will で `online=0` → 死活反映 | ✅ | `--grace-period=0 --force` でLWT発火、全6台の `online=false` 記録 |
| 6 | ダッシュボード3画面がLANから閲覧・ポーリング更新 | ✅ | Traefik Ingress (`dashboard.local`) で3画面200。別端末は hosts 設定後に要目視 |
| 7 | rumqttd Pod delete → 自動再起動 → 再接続 | ✅ | probe変更ロールアウトで実施。mock 6台再接続、データ継続 |
| 8 | ingester Pod delete → 取込再開 | ✅ | 676行 → 再起動後 728行 |
| 9 | **OS再起動 → 全自動復旧** | ⬜ 未実施 | ホストが他ワークロード同居のため実施タイミングは要調整 |
| 10 | Git push → Flux 自動反映 | ✅ | GitHub public リポジトリ作成→`flux bootstrap github` 完了。dashboard replicas 1→2→1 を push だけで反映確認 (revision b2db74e / d5d5f61) |
| 11 | メモリ実測 | ✅ | ノード全体 23.4Gi/31Gi (73%、他ワークロード含む)。IoTスタック分: ClickHouse 307Mi + Prometheus 375Mi + Grafana 318Mi + アプリ計~10Mi。**軽量化不要と判断** |

## 構築中に踏んだ問題と対処

1. **rumqttd 0.20.0 が最新 rustc でビルド不能** — 依存 `metrics` crate の借用パターンが rust#141402 でハードエラー化。→ ビルダーを `rust:1.81-bookworm` に固定
2. **clickhouse-operator が CHI を無視** — チャート 0.27.1 のデフォルトは operator 自身の namespace のみ監視。→ values で `watch.namespaces.include: [".*"]`
3. **kubelet の TCP プローブで rumqttd が毎回 ERROR ログ** — 1883 への素の TCP open/close を接続異常として記録。→ プローブを console ポート (3030) へ
4. **Prometheus 3.x が rumqttd の /metrics を拒否** — Content-Type ヘッダ無しのため。→ ServiceMonitor に `fallbackScrapeProtocol: PrometheusText0.0.4`
5. **ポート1883の二重化** — compose (docker-proxy) と k3s (svclb hostPort) が同居。k3s 検証完了後に compose 停止。ローカル開発時は k3s 側と同時起動しないこと
6. **ClickHouse が自分のシステムログで自滅 (2026-07-27 発見)** — 内蔵システムログがデフォルト全有効のため、6日間で `system.trace_log` が 11億行/15.8 GiB (実センサーデータは全部で 6.3 MiB)。さらに超横長の `system.metric_log` のマージが 4Gi メモリ limit を超えて失敗し、無限リトライで CPU 2コアを常時消費。巻き添えで ingester の `sensor_raw` insert も `MEMORY_LIMIT_EXCEEDED` で 1回目失敗するようになった (リトライで救済されデータ欠損は無し)。→ chi.yaml の `configuration.files` でプロファイリング系ログを `remove="1"`、残りは 3日 TTL。既存パーツは `TRUNCATE TABLE system.trace_log` 等で手動除去が必要 (設定から外してもディスク上のテーブルはアタッチされ続けマージ対象のまま)

## ディスク枯渇のガードレール (2026-07-27 追加)

上記6の暴走を受けて、ClickHouse が SSD を食い尽くさないための多層防御を入れた。

| 層 | 実装 | 効く相手 |
|---|---|---|
| 増加を止める | system ログを `remove="1"` / 3日 TTL (chi.yaml) | 今回の trace_log 型の暴走 |
| 保持を切る | `as7341_raw` 5年 / `sensor_1m` 1年 TTL を追加 | 通常データの無限増加 |
| 書き込みを拒否 | `keep_free_space_bytes` = 100 GiB (chi.yaml) | 原因を問わずディスク満杯 |
| 気付く | PrometheusRule 3本 (prometheusrule.yaml) | 上記が発動する前の早期検知 |

**PVC の 50Gi は効いていない** — local-path provisioner は単なる hostPath バインドで容量を強制しないため、宣言上の 50Gi に対して実際は SSD 全体 (931G) を使えてしまう。本物のハード上限が要るなら btrfs qgroup か loop イメージでの隔離が必要だが、前者は FS 全体の quota 有効化 (rescan + 恒常オーバーヘッド)、後者は起動時マウント依存の追加 (検証項目9 のリスク増) を伴うため Phase 1 では見送った。

閾値の根拠 (実測ベース):

- `sensor_raw` の増加は 124,154行/日 × 7バイト/行 = **0.83 MiB/日**。5年TTLでの定常サイズは約 1.5 GB
- 今回の trace_log 暴走ですら 110 MiB/h だったので、増加アラートは 50 MiB/h (平常の約1000倍) に設定
- kube-prometheus-stack 標準の `NodeFilesystemAlmostOutOfSpace` は空き5% = 約46GB で発火し、`keep_free_space_bytes` の 100 GiB より**後**なので手遅れ。独自に 150 GB で critical を立てている

**TTL 変更時の手順** — `CREATE ... IF NOT EXISTS` は既存テーブルの TTL を書き換えないため 01_schema.sql に `ALTER TABLE ... MODIFY TTL` を併記してある。反映には Job の削除→再作成が必要 (Job は immutable):

```
kubectl -n iot delete job clickhouse-schema && flux reconcile ks apps
```

なお `sensor_1m` の TTL は MV 本体には当てられない (`Engine MaterializedView doesn't support TTL clause`)。内部テーブル名が `.inner_id.<uuid>` で環境ごとに違うため、schema-job.yaml 側で UUID を解決して ALTER している。

## 既知の軽微な挙動

- mock を kill→即再起動すると LWT の `online=0` と復帰の `online=1` が同一秒に入り、`argMax(online, ts)` の勝敗が不定になり得る (実機 ESP32 では起こらない)
- rumqttd は retain 配信が不完全なため、死活判定は5分周期の status 発行に依存 (ダッシュボード側は last seen > 15分 でもオフライン判定)

-- IoT sensing platform schema (Phase 1).
-- Spec: docs/iot-sensing-platform-spec-v1.md §6.2
-- Phase 2 adds storage_policy / TO VOLUME 'cold' via ALTER TABLE ... MODIFY TTL.
-- All statements are idempotent (IF NOT EXISTS) so the schema Job can re-run.

-- ① 縦持ち共通テーブル: 1行 = 1デバイス・1メトリクス・1時点
CREATE TABLE IF NOT EXISTS sensor_raw (
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
-- TTL は新規構築時のみここで効く。既存テーブルには CREATE ... IF NOT EXISTS が
-- 無視されるため、Job 側で内部テーブルに ALTER を当てている (schema-job.yaml 参照)。
CREATE MATERIALIZED VIEW IF NOT EXISTS sensor_1m
ENGINE = AggregatingMergeTree
ORDER BY (metric, device_id, ts_min)
TTL ts_min + INTERVAL 1 YEAR DELETE AS
SELECT toStartOfMinute(ts) AS ts_min, room, device_id, metric,
       avgState(value) AS avg_v, minState(value) AS min_v, maxState(value) AS max_v
FROM sensor_raw
GROUP BY ts_min, room, device_id, metric;

-- ③ 日次集計 MV (日次集約は極小のため TTL なし = 無期限保持)
CREATE MATERIALIZED VIEW IF NOT EXISTS sensor_1d
ENGINE = AggregatingMergeTree
ORDER BY (metric, device_id, ts_day) AS
SELECT toStartOfDay(ts) AS ts_day, room, device_id, metric,
       avgState(value) AS avg_v, minState(value) AS min_v, maxState(value) AS max_v
FROM sensor_raw
GROUP BY ts_day, room, device_id, metric;

-- ④ 測定イベント型デバイス専用 (AS7341, 横持ち)
CREATE TABLE IF NOT EXISTS as7341_raw (
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
ORDER BY (device_id, ts)
TTL ts + INTERVAL 5 YEAR DELETE;

-- ⑤ デバイス死活
CREATE TABLE IF NOT EXISTS device_status (
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

-- ⑥ 既存テーブルへの TTL 追随
-- CREATE ... IF NOT EXISTS は既にあるテーブルの TTL を書き換えないので、
-- TTL を変更したとき Job 再実行で反映されるよう ALTER も併記する (冪等)。
-- sensor_1m の内部テーブルだけは名前に UUID を含み静的に書けないため
-- schema-job.yaml 側で解決している。
ALTER TABLE sensor_raw    MODIFY TTL ts + INTERVAL 5 YEAR DELETE;
ALTER TABLE as7341_raw    MODIFY TTL ts + INTERVAL 5 YEAR DELETE;
ALTER TABLE device_status MODIFY TTL ts + INTERVAL 1 YEAR DELETE;

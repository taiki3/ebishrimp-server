//! ClickHouse access (HTTP, read-only `dashboard` user).
//!
//! Every query binds user-supplied values (`metric`, `device_id`) with
//! `query().bind()`; only static SQL fragments are ever interpolated.

use clickhouse::{Client, Row};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::config::Config;
use crate::echarts::DeviceSeries;
use crate::range::{series_sql, Range};
use crate::util::epoch_ms;

pub type DbResult<T> = Result<T, clickhouse::error::Error>;

/// Shared ClickHouse client, registered in the router's app context.
pub struct Db {
    client: Client,
}

/// Latest value of one (metric, device) pair.
#[derive(Debug, Row, Deserialize)]
pub struct OverviewRow {
    pub metric: String,
    pub device_id: String,
    pub room: String,
    pub value: f64,
    #[serde(with = "clickhouse::serde::time::datetime")]
    pub last_ts: OffsetDateTime,
}

/// One time-series point (min/avg/max collapse to the same value for raw data).
#[derive(Debug, Row, Deserialize)]
pub struct SeriesRow {
    pub device_id: String,
    #[serde(with = "clickhouse::serde::time::datetime")]
    pub ts: OffsetDateTime,
    pub avg_v: f64,
    pub min_v: f64,
    pub max_v: f64,
}

/// Latest status of one device.
#[derive(Debug, Row, Deserialize)]
pub struct DeviceStatusRow {
    pub device_id: String,
    pub rssi: i16,
    pub uptime_s: u32,
    pub online: bool,
    #[serde(with = "clickhouse::serde::time::datetime")]
    pub last_ts: OffsetDateTime,
}

impl Db {
    pub fn new(config: &Config) -> Self {
        let client = Client::default()
            .with_url(&config.clickhouse_url)
            .with_user(&config.clickhouse_user)
            .with_password(&config.clickhouse_password)
            .with_database(&config.clickhouse_database);
        Self { client }
    }

    /// Latest value per (metric, device) over the last hour.
    pub async fn overview(&self) -> DbResult<Vec<OverviewRow>> {
        self.client
            .query(
                "SELECT metric, device_id, any(room) AS room, \
                 argMax(value, ts) AS value, max(ts) AS last_ts \
                 FROM sensor_raw \
                 WHERE ts > now() - INTERVAL 1 HOUR \
                 GROUP BY metric, device_id \
                 ORDER BY metric, device_id",
            )
            .fetch_all()
            .await
    }

    /// Metrics seen in the last 24 h (chart selector options).
    pub async fn metrics(&self) -> DbResult<Vec<String>> {
        self.client
            .query(
                "SELECT DISTINCT metric FROM sensor_raw \
                 WHERE ts > now() - INTERVAL 24 HOUR \
                 ORDER BY metric",
            )
            .fetch_all()
            .await
    }

    /// Devices that reported `metric` in the last 24 h (chart selector options).
    pub async fn devices_for_metric(&self, metric: &str) -> DbResult<Vec<String>> {
        self.client
            .query(
                "SELECT DISTINCT device_id FROM sensor_raw \
                 WHERE metric = ? AND ts > now() - INTERVAL 24 HOUR \
                 ORDER BY device_id",
            )
            .bind(metric)
            .fetch_all()
            .await
    }

    /// Time-series points for the chart, from the table `range` maps to.
    /// `device: None` plots every device.
    pub async fn series(
        &self,
        metric: &str,
        device: Option<&str>,
        range: Range,
    ) -> DbResult<Vec<SeriesRow>> {
        let sql = series_sql(range, device.is_some());
        let mut query = self.client.query(&sql).bind(metric);
        if let Some(device) = device {
            query = query.bind(device);
        }
        query.fetch_all().await
    }

    /// Latest status per device.
    pub async fn device_statuses(&self) -> DbResult<Vec<DeviceStatusRow>> {
        self.client
            .query(
                "SELECT device_id, argMax(rssi, ts) AS rssi, \
                 argMax(uptime_s, ts) AS uptime_s, argMax(online, ts) AS online, \
                 max(ts) AS last_ts \
                 FROM device_status \
                 GROUP BY device_id \
                 ORDER BY device_id",
            )
            .fetch_all()
            .await
    }
}

/// Groups rows (already sorted by `device_id, ts`) into one series per device.
pub fn group_series(rows: &[SeriesRow]) -> Vec<DeviceSeries> {
    let mut out: Vec<DeviceSeries> = Vec::new();
    for row in rows {
        let ms = epoch_ms(row.ts);
        match out.last_mut() {
            Some(series) if series.device_id == row.device_id => {
                series.avg.push((ms, row.avg_v));
                series.min.push((ms, row.min_v));
                series.max.push((ms, row.max_v));
            }
            _ => out.push(DeviceSeries {
                device_id: row.device_id.clone(),
                avg: vec![(ms, row.avg_v)],
                min: vec![(ms, row.min_v)],
                max: vec![(ms, row.max_v)],
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn row(device: &str, ts: OffsetDateTime, v: f64) -> SeriesRow {
        SeriesRow {
            device_id: device.to_string(),
            ts,
            avg_v: v,
            min_v: v - 1.0,
            max_v: v + 1.0,
        }
    }

    #[test]
    fn group_series_splits_by_device() {
        let t0 = datetime!(2026-07-20 00:00:00 UTC);
        let t1 = datetime!(2026-07-20 00:01:00 UTC);
        let rows = vec![row("a", t0, 1.0), row("a", t1, 2.0), row("b", t0, 3.0)];
        let series = group_series(&rows);
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].device_id, "a");
        assert_eq!(series[0].avg.len(), 2);
        assert_eq!(series[0].min[0].1, 0.0);
        assert_eq!(series[0].max[1].1, 3.0);
        assert_eq!(series[1].device_id, "b");
        assert_eq!(series[1].avg, vec![(epoch_ms(t0), 3.0)]);
    }

    #[test]
    fn group_series_empty() {
        assert!(group_series(&[]).is_empty());
    }
}

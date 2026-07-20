//! Time-range selection for the chart screen and the mapping from a range to
//! the ClickHouse table it is served from.

/// Which table a range reads from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `sensor_raw`: raw 10 s cadence points.
    Raw,
    /// `sensor_1m`: 1-minute aggregates (`avgMerge`/`minMerge`/`maxMerge`).
    Agg1m,
    /// `sensor_1d`: daily aggregates.
    Agg1d,
}

/// A selectable chart time range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Min10,
    Hour1,
    Hour6,
    Hour24,
    Day7,
    Day30,
}

impl Range {
    pub const ALL: [Range; 6] = [
        Range::Min10,
        Range::Hour1,
        Range::Hour6,
        Range::Hour24,
        Range::Day7,
        Range::Day30,
    ];

    /// Parses the `range` query parameter. Unknown values yield `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "10m" => Some(Range::Min10),
            "1h" => Some(Range::Hour1),
            "6h" => Some(Range::Hour6),
            "24h" => Some(Range::Hour24),
            "7d" => Some(Range::Day7),
            "30d" => Some(Range::Day30),
            _ => None,
        }
    }

    /// The value used in URLs.
    pub fn code(self) -> &'static str {
        match self {
            Range::Min10 => "10m",
            Range::Hour1 => "1h",
            Range::Hour6 => "6h",
            Range::Hour24 => "24h",
            Range::Day7 => "7d",
            Range::Day30 => "30d",
        }
    }

    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            Range::Min10 => "10分",
            Range::Hour1 => "1時間",
            Range::Hour6 => "6時間",
            Range::Hour24 => "24時間",
            Range::Day7 => "7日",
            Range::Day30 => "30日",
        }
    }

    /// Which table serves this range: raw points for the last few minutes,
    /// 1-minute aggregates up to 24 h, daily aggregates beyond that.
    pub fn source(self) -> Source {
        match self {
            Range::Min10 => Source::Raw,
            Range::Hour1 | Range::Hour6 | Range::Hour24 => Source::Agg1m,
            Range::Day7 | Range::Day30 => Source::Agg1d,
        }
    }

    /// The SQL `INTERVAL` expression bounding the range. Static strings only;
    /// never derived from user input.
    pub fn interval_sql(self) -> &'static str {
        match self {
            Range::Min10 => "INTERVAL 10 MINUTE",
            Range::Hour1 => "INTERVAL 1 HOUR",
            Range::Hour6 => "INTERVAL 6 HOUR",
            Range::Hour24 => "INTERVAL 24 HOUR",
            Range::Day7 => "INTERVAL 7 DAY",
            Range::Day30 => "INTERVAL 30 DAY",
        }
    }
}

/// Builds the series query for a range. `metric` is always bound as the first
/// `?`; when `with_device` is set a second `?` binds the device id. Only
/// static SQL fragments are interpolated here.
pub fn series_sql(range: Range, with_device: bool) -> String {
    let device_filter = if with_device { " AND device_id = ?" } else { "" };
    let interval = range.interval_sql();
    match range.source() {
        Source::Raw => format!(
            "SELECT device_id, ts, value AS avg_v, value AS min_v, value AS max_v \
             FROM sensor_raw \
             WHERE metric = ?{device_filter} AND ts > now() - {interval} \
             ORDER BY device_id, ts"
        ),
        Source::Agg1m => format!(
            "SELECT device_id, ts_min AS ts, avgMerge(avg_v) AS avg_v, \
             minMerge(min_v) AS min_v, maxMerge(max_v) AS max_v \
             FROM sensor_1m \
             WHERE metric = ?{device_filter} AND ts_min > now() - {interval} \
             GROUP BY device_id, ts_min \
             ORDER BY device_id, ts_min"
        ),
        Source::Agg1d => format!(
            "SELECT device_id, ts_day AS ts, avgMerge(avg_v) AS avg_v, \
             minMerge(min_v) AS min_v, maxMerge(max_v) AS max_v \
             FROM sensor_1d \
             WHERE metric = ?{device_filter} AND ts_day > now() - {interval} \
             GROUP BY device_id, ts_day \
             ORDER BY device_id, ts_day"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip() {
        for range in Range::ALL {
            assert_eq!(Range::parse(range.code()), Some(range));
        }
        assert_eq!(Range::parse("2h"), None);
        assert_eq!(Range::parse(""), None);
        assert_eq!(Range::parse("1h; DROP TABLE"), None);
    }

    #[test]
    fn source_selection_follows_spec() {
        assert_eq!(Range::Min10.source(), Source::Raw);
        assert_eq!(Range::Hour1.source(), Source::Agg1m);
        assert_eq!(Range::Hour6.source(), Source::Agg1m);
        assert_eq!(Range::Hour24.source(), Source::Agg1m);
        assert_eq!(Range::Day7.source(), Source::Agg1d);
        assert_eq!(Range::Day30.source(), Source::Agg1d);
    }

    #[test]
    fn raw_sql_reads_sensor_raw_with_bound_metric() {
        let sql = series_sql(Range::Min10, false);
        assert!(sql.contains("FROM sensor_raw"));
        assert!(sql.contains("metric = ?"));
        assert!(sql.contains("INTERVAL 10 MINUTE"));
        assert!(!sql.contains("device_id = ?"));
    }

    #[test]
    fn agg1m_sql_uses_merge_functions_and_group_by() {
        let sql = series_sql(Range::Hour6, true);
        assert!(sql.contains("FROM sensor_1m"));
        assert!(sql.contains("avgMerge(avg_v)"));
        assert!(sql.contains("minMerge(min_v)"));
        assert!(sql.contains("maxMerge(max_v)"));
        assert!(sql.contains("GROUP BY device_id, ts_min"));
        assert!(sql.contains("AND device_id = ?"));
        assert!(sql.contains("INTERVAL 6 HOUR"));
    }

    #[test]
    fn agg1d_sql_uses_ts_day() {
        let sql = series_sql(Range::Day30, false);
        assert!(sql.contains("FROM sensor_1d"));
        assert!(sql.contains("ts_day"));
        assert!(sql.contains("INTERVAL 30 DAY"));
    }
}

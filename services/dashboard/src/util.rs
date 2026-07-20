//! Formatting helpers: JST timestamps, relative times, uptime, values, URLs.

use time::macros::{format_description, offset};
use time::{Duration, OffsetDateTime, UtcOffset};

pub const JST: UtcOffset = offset!(+9);

/// Formats a timestamp as JST `MM-DD HH:MM:SS`.
pub fn fmt_jst(ts: OffsetDateTime) -> String {
    let format = format_description!("[month]-[day] [hour]:[minute]:[second]");
    ts.to_offset(JST)
        .format(&format)
        .unwrap_or_else(|_| "-".to_string())
}

/// Humanizes how long ago `ts` was, relative to `now`.
pub fn ago(now: OffsetDateTime, ts: OffsetDateTime) -> String {
    let elapsed = now - ts;
    if elapsed < Duration::seconds(5) {
        return "たった今".to_string();
    }
    let seconds = elapsed.whole_seconds();
    if seconds < 60 {
        format!("{seconds}秒前")
    } else if seconds < 3600 {
        format!("{}分前", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}時間前", seconds / 3600)
    } else {
        format!("{}日前", seconds / 86_400)
    }
}

/// Humanizes an uptime in seconds, e.g. `1日 2時間` / `3時間 4分` / `42秒`.
pub fn humanize_uptime(uptime_s: u32) -> String {
    let s = u64::from(uptime_s);
    let (days, hours, minutes, seconds) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60, s % 60);
    if days > 0 {
        format!("{days}日 {hours}時間")
    } else if hours > 0 {
        format!("{hours}時間 {minutes}分")
    } else if minutes > 0 {
        format!("{minutes}分")
    } else {
        format!("{seconds}秒")
    }
}

/// Formats a sensor value with precision that scales with magnitude.
pub fn fmt_value(v: f64) -> String {
    if !v.is_finite() {
        return "-".to_string();
    }
    let a = v.abs();
    if a >= 1000.0 {
        format!("{v:.0}")
    } else if a >= 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// A device is shown online only when its `online` flag is set AND it has been
/// seen within the last 15 minutes.
pub fn is_online(flag: bool, last_seen: OffsetDateTime, now: OffsetDateTime) -> bool {
    flag && now - last_seen <= Duration::minutes(15)
}

/// Milliseconds since the Unix epoch, for ECharts time-axis data points.
pub fn epoch_ms(ts: OffsetDateTime) -> i64 {
    (ts.unix_timestamp_nanos() / 1_000_000) as i64
}

/// Builds the chart fragment URL with properly encoded query parameters.
pub fn chart_fragment_url(metric: &str, device: &str, range_code: &str) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("metric", metric)
        .append_pair("device", device)
        .append_pair("range", range_code);
    format!("/fragments/chart?{}", query.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn fmt_jst_converts_utc_to_jst() {
        // 2026-07-20T00:30:00Z = 09:30 JST
        let ts = datetime!(2026-07-20 00:30:00 UTC);
        assert_eq!(fmt_jst(ts), "07-20 09:30:00");
    }

    #[test]
    fn ago_buckets() {
        let now = datetime!(2026-07-20 12:00:00 UTC);
        assert_eq!(ago(now, now), "たった今");
        assert_eq!(ago(now, now - Duration::seconds(42)), "42秒前");
        assert_eq!(ago(now, now - Duration::minutes(3)), "3分前");
        assert_eq!(ago(now, now - Duration::hours(2)), "2時間前");
        assert_eq!(ago(now, now - Duration::days(5)), "5日前");
    }

    #[test]
    fn humanize_uptime_buckets() {
        assert_eq!(humanize_uptime(42), "42秒");
        assert_eq!(humanize_uptime(5 * 60), "5分");
        assert_eq!(humanize_uptime(3 * 3600 + 4 * 60), "3時間 4分");
        assert_eq!(humanize_uptime(86_400 + 2 * 3600), "1日 2時間");
    }

    #[test]
    fn fmt_value_precision() {
        assert_eq!(fmt_value(25.345), "25.35");
        assert_eq!(fmt_value(101.34), "101.3");
        assert_eq!(fmt_value(1234.5), "1235");
        assert_eq!(fmt_value(f64::NAN), "-");
    }

    #[test]
    fn is_online_requires_flag_and_recency() {
        let now = datetime!(2026-07-20 12:00:00 UTC);
        let recent = now - Duration::minutes(5);
        let stale = now - Duration::minutes(16);
        assert!(is_online(true, recent, now));
        assert!(!is_online(false, recent, now));
        assert!(!is_online(true, stale, now));
    }

    #[test]
    fn chart_fragment_url_encodes_params() {
        let url = chart_fragment_url("temp&hum", "dev 01", "1h");
        assert_eq!(url, "/fragments/chart?metric=temp%26hum&device=dev+01&range=1h");
    }
}

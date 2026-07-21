//! Routes, layout, and all SSR views (topcoat).
//!
//! Screen structure (spec §6.4):
//! - `/`        最新値グリッド (metric × device), htmx 10 s poll
//! - `/chart`   時系列チャート (ECharts, option JSON built server-side), 30 s poll
//! - `/devices` デバイス死活パネル, 10 s poll
//!
//! Each screen embeds its fragment component directly for the first render and
//! then swaps it via `/fragments/*` with htmx polling. Errors from ClickHouse
//! render an error box inside the fragment; they never take the page down.

use time::OffsetDateTime;
use topcoat::{
    context::{app_context, Cx},
    router::{layout, page, parse_query_params, route, uri, Router, Slot},
    view::{component, view, Unescaped, View},
    Result,
};

use crate::db::{group_series, Db, OverviewRow};
use crate::echarts::{chart_option, script_safe_json};
use crate::range::Range;
use crate::util::{ago, chart_fragment_url, fmt_jst, fmt_value, humanize_uptime, is_online};

const CSS: &str = include_str!("style.css");
const ECHARTS_JS: &[u8] = include_bytes!("../static/echarts.min.js");
const HTMX_JS: &[u8] = include_bytes!("../static/htmx.min.js");

const ALL_DEVICES: &str = "all";

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

#[layout("/")]
async fn root_layout(cx: &Cx, slot: Slot<'_>) -> Result {
    let path = uri(cx).path().to_string();
    let nav = [
        ("/", "最新値"),
        ("/chart", "時系列"),
        ("/devices", "デバイス死活"),
    ];
    view! {
        <!DOCTYPE html>
        <html lang="ja">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"ebishrimp sensor dashboard"</title>
                <style>(Unescaped::new_unchecked(CSS))</style>
                <script src="/static/htmx.min.js"></script>
                <script src="/static/echarts.min.js"></script>
            </head>
            <body>
                <header class="topbar">
                    <span class="brand">"ebishrimp"</span>
                    <nav>
                        for (href, label) in nav {
                            <a href=(href) class=(if href == path { "nav-link active" } else { "nav-link" })>
                                (label)
                            </a>
                        }
                    </nav>
                </header>
                <main>(slot.await?)</main>
            </body>
        </html>
    }
}

// ---------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------

#[component]
async fn error_box(message: String) -> Result {
    view! {
        <div class="error-box">
            <strong>"データ取得エラー"</strong>
            <span class="error-detail">(message)</span>
        </div>
    }
}

#[component]
async fn empty_box(message: &'static str) -> Result {
    view! { <div class="empty-box">(message)</div> }
}

// ---------------------------------------------------------------------------
// Screen 1: metrics overview (最新値)
// ---------------------------------------------------------------------------

#[component]
async fn overview_grid(cx: &Cx) -> Result {
    let db: &Db = app_context(cx);
    let rows = match db.overview().await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "overview query failed");
            return view! { error_box(message: error.to_string()) };
        }
    };
    if rows.is_empty() {
        return view! { empty_box(message: "直近1時間のデータがありません") };
    }

    // Rows arrive ordered by (metric, device_id); chunk them per metric.
    let mut sections: Vec<(String, Vec<OverviewRow>)> = Vec::new();
    for row in rows {
        match sections.last_mut() {
            Some((metric, group)) if *metric == row.metric => group.push(row),
            _ => sections.push((row.metric.clone(), vec![row])),
        }
    }
    let now = OffsetDateTime::now_utc();

    view! {
        for (metric, group) in sections {
            <section class="metric-section">
                <h2>(metric.as_str())</h2>
                <div class="card-grid">
                    for row in group {
                        <a class="card" href=(chart_link(&row.metric, &row.device_id))>
                            <div class="card-head">
                                <span class="device">(row.device_id.as_str())</span>
                                <span class="room">(row.room.as_str())</span>
                            </div>
                            <div class="value">(fmt_value(row.value))</div>
                            <div class="last-seen" title=(fmt_jst(row.last_ts))>
                                (ago(now, row.last_ts))
                            </div>
                        </a>
                    }
                </div>
            </section>
        }
    }
}

fn chart_link(metric: &str, device: &str) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("metric", metric)
        .append_pair("device", device)
        .append_pair("range", Range::Hour1.code());
    format!("/chart?{}", query.finish())
}

#[page("/")]
async fn overview_page(cx: &Cx) -> Result {
    let _ = cx;
    view! {
        <h1>"最新値"</h1>
        <p class="hint">"直近1時間に受信した metric × device の最新値 (10秒ごとに自動更新)"</p>
        <div id="overview" hx-get="/fragments/overview" hx-trigger="every 10s" hx-swap="innerHTML">
            overview_grid()
        </div>
    }
}

#[route(GET "/fragments/overview")]
async fn overview_fragment(cx: &Cx) -> Result<View> {
    let _ = cx;
    view! { overview_grid() }
}

// ---------------------------------------------------------------------------
// Screen 2: time-series chart (時系列)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, serde::Deserialize)]
struct ChartQuery {
    metric: Option<String>,
    device: Option<String>,
    range: Option<String>,
}

#[derive(Debug, Clone)]
struct ChartSelection {
    metric: Option<String>,
    device: String,
    range: Range,
}

impl ChartSelection {
    fn device_filter(&self) -> Option<&str> {
        (self.device != ALL_DEVICES).then_some(self.device.as_str())
    }
}

/// Resolves query params against the list of known metrics: an explicit
/// `metric` wins, otherwise the first known metric is used.
fn resolve_selection(query: &ChartQuery, known_metrics: &[String]) -> ChartSelection {
    let metric = query
        .metric
        .as_deref()
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .or_else(|| known_metrics.first().cloned());
    let device = query
        .device
        .as_deref()
        .filter(|d| !d.is_empty())
        .unwrap_or(ALL_DEVICES)
        .to_string();
    let range = query
        .range
        .as_deref()
        .and_then(Range::parse)
        .unwrap_or(Range::Hour1);
    ChartSelection {
        metric,
        device,
        range,
    }
}

#[component]
async fn chart_body(cx: &Cx, selection: ChartSelection) -> Result {
    let Some(metric) = selection.metric.clone() else {
        return view! { empty_box(message: "直近24時間にメトリクスがありません") };
    };
    let db: &Db = app_context(cx);
    let rows = match db
        .series(&metric, selection.device_filter(), selection.range)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, metric, "series query failed");
            return view! { error_box(message: error.to_string()) };
        }
    };
    if rows.is_empty() {
        return view! { empty_box(message: "選択範囲にデータがありません") };
    }

    let point_count = rows.len();
    let series = group_series(&rows);
    let device_count = series.len();
    let option = chart_option(&metric, &series);
    let script = format!(
        "(function(){{\
           var el=document.getElementById('chart-canvas');\
           if(!el||typeof echarts==='undefined')return;\
           var prev=echarts.getInstanceByDom(el);\
           if(prev)prev.dispose();\
           var chart=echarts.init(el);\
           chart.setOption({option});\
           if(!window.__dashResizeBound){{\
             window.__dashResizeBound=true;\
             window.addEventListener('resize',function(){{\
               var e=document.getElementById('chart-canvas');\
               if(!e)return;\
               var i=echarts.getInstanceByDom(e);\
               if(i)i.resize();\
             }});\
           }}\
         }})();",
        option = script_safe_json(&option),
    );

    view! {
        <div class="chart-box">
            <div id="chart-canvas"></div>
            (Unescaped::new_unchecked(format!("<script>{script}</script>")))
        </div>
        <p class="chart-meta">
            (device_count) " デバイス / " (point_count) " ポイント / 更新 "
            (fmt_jst(OffsetDateTime::now_utc())) " JST"
        </p>
    }
}

#[component]
async fn chart_controls(cx: &Cx, selection: ChartSelection, known_metrics: Vec<String>) -> Result {
    let db: &Db = app_context(cx);
    let mut devices = match &selection.metric {
        Some(metric) => db.devices_for_metric(metric).await.unwrap_or_default(),
        None => Vec::new(),
    };
    // Keep the current selection visible even if the device list is
    // unavailable or the device dropped out of the last 24 h.
    if selection.device != ALL_DEVICES && !devices.contains(&selection.device) {
        devices.push(selection.device.clone());
    }
    let selected_metric = selection.metric.clone().unwrap_or_default();
    let selected_device = selection.device.clone();
    let selected_range = selection.range;

    view! {
        <form class="controls" method="get" action="/chart">
            <label>
                "メトリクス"
                <select name="metric" onchange="this.form.submit()">
                    for metric in known_metrics {
                        <option
                            value=(metric.as_str())
                            if metric == selected_metric { selected="selected" }
                        >
                            (metric.as_str())
                        </option>
                    }
                </select>
            </label>
            <label>
                "デバイス"
                <select name="device" onchange="this.form.submit()">
                    <option
                        value=(ALL_DEVICES)
                        if selected_device == ALL_DEVICES { selected="selected" }
                    >
                        "全デバイス"
                    </option>
                    for device in devices {
                        <option
                            value=(device.as_str())
                            if device == selected_device { selected="selected" }
                        >
                            (device.as_str())
                        </option>
                    }
                </select>
            </label>
            <label>
                "期間"
                <select name="range" onchange="this.form.submit()">
                    for range in Range::ALL {
                        <option
                            value=(range.code())
                            if range == selected_range { selected="selected" }
                        >
                            (range.label())
                        </option>
                    }
                </select>
            </label>
            <button type="submit">"表示"</button>
        </form>
    }
}

#[page("/chart")]
async fn chart_page(cx: &Cx) -> Result {
    let query = parse_query_params::<ChartQuery>(cx).unwrap_or_default();
    let db: &Db = app_context(cx);
    let known_metrics = match db.metrics().await {
        Ok(metrics) => metrics,
        Err(error) => {
            tracing::warn!(%error, "metric list query failed");
            // Fall back to the requested metric so the screen stays usable.
            query.metric.iter().cloned().collect()
        }
    };
    let selection = resolve_selection(&query, &known_metrics);
    let fragment_url = chart_fragment_url(
        selection.metric.as_deref().unwrap_or(""),
        &selection.device,
        selection.range.code(),
    );
    view! {
        <h1>"時系列"</h1>
        chart_controls(selection: selection.clone(), known_metrics: known_metrics)
        <div id="chart-frag" hx-get=(fragment_url) hx-trigger="every 30s" hx-swap="innerHTML">
            chart_body(selection: selection)
        </div>
    }
}

#[route(GET "/fragments/chart")]
async fn chart_fragment(cx: &Cx) -> Result<View> {
    let query = parse_query_params::<ChartQuery>(cx).unwrap_or_default();
    // The fragment URL always carries an explicit metric; no DB round-trip
    // for the metric list is needed here.
    let selection = resolve_selection(&query, &[]);
    view! { chart_body(selection: selection) }
}

// ---------------------------------------------------------------------------
// Screen 3: device health (デバイス死活)
// ---------------------------------------------------------------------------

#[component]
async fn devices_table(cx: &Cx) -> Result {
    let db: &Db = app_context(cx);
    let rows = match db.device_statuses().await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "device status query failed");
            return view! { error_box(message: error.to_string()) };
        }
    };
    if rows.is_empty() {
        return view! { empty_box(message: "デバイスの status がまだ届いていません") };
    }
    let now = OffsetDateTime::now_utc();

    view! {
        <table class="status-table">
            <thead>
                <tr>
                    <th>"デバイス"</th>
                    <th>"状態"</th>
                    <th>"RSSI"</th>
                    <th>"稼働時間"</th>
                    <th>"最終受信"</th>
                </tr>
            </thead>
            <tbody>
                for row in rows {
                    <tr>
                        <td class="device">(row.device_id.as_str())</td>
                        <td>
                            if is_online(row.online, row.last_ts, now) {
                                <span class="badge online">"オンライン"</span>
                            } else {
                                <span class="badge offline">"オフライン"</span>
                            }
                        </td>
                        <td class="num">(row.rssi) " dBm"</td>
                        <td>(humanize_uptime(row.uptime_s))</td>
                        <td title=(fmt_jst(row.last_ts))>(ago(now, row.last_ts))</td>
                    </tr>
                }
            </tbody>
        </table>
    }
}

#[page("/devices")]
async fn devices_page(cx: &Cx) -> Result {
    let _ = cx;
    view! {
        <h1>"デバイス死活"</h1>
        <p class="hint">"online フラグが false、または 15 分以上受信がない場合はオフライン表示 (10秒ごとに自動更新)"</p>
        <div id="devices" hx-get="/fragments/devices" hx-trigger="every 10s" hx-swap="innerHTML">
            devices_table()
        </div>
    }
}

#[route(GET "/fragments/devices")]
async fn devices_fragment(cx: &Cx) -> Result<View> {
    let _ = cx;
    view! { devices_table() }
}

// ---------------------------------------------------------------------------
// Health check and static assets
// ---------------------------------------------------------------------------

#[route(GET "/healthz")]
async fn healthz() -> Result<&'static str> {
    Ok("ok")
}

type StaticAsset = ([(&'static str, &'static str); 2], &'static [u8]);

fn js_asset(body: &'static [u8]) -> StaticAsset {
    (
        [
            ("content-type", "application/javascript; charset=utf-8"),
            ("cache-control", "public, max-age=86400"),
        ],
        body,
    )
}

#[route(GET "/static/echarts.min.js")]
async fn echarts_js() -> Result<StaticAsset> {
    Ok(js_asset(ECHARTS_JS))
}

#[route(GET "/static/htmx.min.js")]
async fn htmx_js() -> Result<StaticAsset> {
    Ok(js_asset(HTMX_JS))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(db: Db) -> Router {
    Router::builder()
        .layout(root_layout)
        .page(overview_page)
        .page(chart_page)
        .page(devices_page)
        .route(overview_fragment)
        .route(chart_fragment)
        .route(devices_fragment)
        .route(healthz)
        .route(echarts_js)
        .route(htmx_js)
        .app_context(db)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_selection_defaults() {
        let selection = resolve_selection(&ChartQuery::default(), &[]);
        assert_eq!(selection.metric, None);
        assert_eq!(selection.device, "all");
        assert_eq!(selection.range, Range::Hour1);
        assert_eq!(selection.device_filter(), None);
    }

    #[test]
    fn resolve_selection_falls_back_to_first_known_metric() {
        let known = vec!["co2".to_string(), "temperature".to_string()];
        let selection = resolve_selection(&ChartQuery::default(), &known);
        assert_eq!(selection.metric.as_deref(), Some("co2"));
    }

    #[test]
    fn resolve_selection_explicit_params_win() {
        let query = ChartQuery {
            metric: Some("humidity".to_string()),
            device: Some("esp32-01".to_string()),
            range: Some("7d".to_string()),
        };
        let selection = resolve_selection(&query, &["co2".to_string()]);
        assert_eq!(selection.metric.as_deref(), Some("humidity"));
        assert_eq!(selection.device_filter(), Some("esp32-01"));
        assert_eq!(selection.range, Range::Day7);
    }

    #[test]
    fn resolve_selection_rejects_bad_range() {
        let query = ChartQuery {
            range: Some("99y".to_string()),
            ..ChartQuery::default()
        };
        assert_eq!(resolve_selection(&query, &[]).range, Range::Hour1);
    }

    #[test]
    fn chart_link_encodes() {
        assert_eq!(
            chart_link("temp", "dev/01"),
            "/chart?metric=temp&device=dev%2F01&range=1h"
        );
    }
}

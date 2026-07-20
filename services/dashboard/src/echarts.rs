//! Builds the full Apache ECharts option JSON on the server. The browser only
//! runs `echarts.init` + `setOption` on what we render here.

use serde_json::{Value, json};

/// One line on the chart: a device's points over time.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceSeries {
    pub device_id: String,
    /// `(epoch_ms, avg)` points.
    pub avg: Vec<(i64, f64)>,
    /// `(epoch_ms, min)` points, same timestamps as `avg`.
    pub min: Vec<(i64, f64)>,
    /// `(epoch_ms, max)` points, same timestamps as `avg`.
    pub max: Vec<(i64, f64)>,
}

const PALETTE: [&str; 8] = [
    "#4cc2ff", "#7ce38b", "#f0b429", "#f47067", "#b083f0", "#39d5c8", "#f78bd0", "#a8b3c5",
];

const TEXT: &str = "#9fb2c8";
const AXIS_LINE: &str = "#3b4a5e";
const SPLIT_LINE: &str = "#232d3b";

fn point_array(points: &[(i64, f64)]) -> Value {
    Value::Array(points.iter().map(|(t, v)| json!([t, v])).collect())
}

/// Builds the complete ECharts option. When exactly one device is plotted
/// (and min/max actually differ from avg), a shaded min–max band is added
/// behind the average line.
pub fn chart_option(metric: &str, series: &[DeviceSeries]) -> Value {
    let mut js_series = Vec::new();
    let legend: Vec<&str> = series.iter().map(|s| s.device_id.as_str()).collect();
    let with_band = series.len() == 1
        && series[0]
            .avg
            .iter()
            .zip(&series[0].min)
            .zip(&series[0].max)
            .any(|((a, mn), mx)| a.1 != mn.1 || a.1 != mx.1);

    if with_band {
        let s = &series[0];
        let band: Vec<(i64, f64)> = s
            .max
            .iter()
            .zip(&s.min)
            .map(|(mx, mn)| (mx.0, mx.1 - mn.1))
            .collect();
        js_series.push(json!({
            "name": "min",
            "type": "line",
            "stack": "band",
            "data": point_array(&s.min),
            "symbol": "none",
            "silent": true,
            "lineStyle": {"opacity": 0},
            "tooltip": {"show": false},
        }));
        js_series.push(json!({
            "name": "min-max",
            "type": "line",
            "stack": "band",
            "data": point_array(&band),
            "symbol": "none",
            "silent": true,
            "lineStyle": {"opacity": 0},
            "areaStyle": {"color": PALETTE[0], "opacity": 0.15},
            "tooltip": {"show": false},
        }));
    }

    for (i, s) in series.iter().enumerate() {
        js_series.push(json!({
            "name": s.device_id,
            "type": "line",
            "data": point_array(&s.avg),
            "showSymbol": false,
            "smooth": false,
            "lineStyle": {"width": 2},
            "itemStyle": {"color": PALETTE[i % PALETTE.len()]},
        }));
    }

    json!({
        "backgroundColor": "transparent",
        "animation": false,
        "textStyle": {"color": TEXT},
        "title": {
            "text": metric,
            "left": 8,
            "textStyle": {"color": "#dbe4f0", "fontSize": 14},
        },
        "tooltip": {"trigger": "axis"},
        "legend": {
            "data": legend,
            "top": 4,
            "right": 8,
            "textStyle": {"color": TEXT},
        },
        "grid": {"left": 56, "right": 24, "top": 40, "bottom": 44},
        "xAxis": {
            "type": "time",
            "axisLine": {"lineStyle": {"color": AXIS_LINE}},
            "axisLabel": {"color": TEXT},
        },
        "yAxis": {
            "type": "value",
            "scale": true,
            "axisLine": {"lineStyle": {"color": AXIS_LINE}},
            "axisLabel": {"color": TEXT},
            "splitLine": {"lineStyle": {"color": SPLIT_LINE}},
        },
        "series": js_series,
    })
}

/// Serializes JSON for embedding inside an inline `<script>` element.
///
/// `</` must not appear literally inside a script element (it could terminate
/// the script or open a comment-like context), so it is escaped to `<\/`,
/// which is an identical string in JavaScript. `<!--` is broken up the same
/// way.
pub fn script_safe_json(value: &Value) -> String {
    value.to_string().replace("</", "<\\/").replace("<!--", "<\\!--")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(device: &str, vals: &[(i64, f64, f64, f64)]) -> DeviceSeries {
        DeviceSeries {
            device_id: device.to_string(),
            avg: vals.iter().map(|(t, a, _, _)| (*t, *a)).collect(),
            min: vals.iter().map(|(t, _, m, _)| (*t, *m)).collect(),
            max: vals.iter().map(|(t, _, _, x)| (*t, *x)).collect(),
        }
    }

    #[test]
    fn multi_device_has_one_series_per_device_and_no_band() {
        let option = chart_option(
            "temperature",
            &[
                series("dev-a", &[(1000, 1.0, 0.5, 1.5)]),
                series("dev-b", &[(1000, 2.0, 1.5, 2.5)]),
            ],
        );
        let s = option["series"].as_array().unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0]["name"], "dev-a");
        assert_eq!(s[1]["name"], "dev-b");
    }

    #[test]
    fn single_device_gets_min_max_band() {
        let option = chart_option("temperature", &[series("dev-a", &[(1000, 1.0, 0.5, 1.5)])]);
        let s = option["series"].as_array().unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s[0]["stack"], "band");
        assert_eq!(s[1]["stack"], "band");
        // Band series is max - min.
        assert_eq!(s[1]["data"][0][1], 1.0);
        assert_eq!(s[2]["name"], "dev-a");
    }

    #[test]
    fn single_device_raw_points_skip_band() {
        // Raw points have min == avg == max: a band would be zero-height noise.
        let option = chart_option("temperature", &[series("dev-a", &[(1000, 1.0, 1.0, 1.0)])]);
        assert_eq!(option["series"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn data_points_are_epoch_ms_pairs() {
        let option = chart_option("co2", &[series("dev-a", &[(1_753_000_000_000, 640.0, 640.0, 640.0)])]);
        assert_eq!(option["series"][0]["data"][0][0], 1_753_000_000_000_i64);
        assert_eq!(option["series"][0]["data"][0][1], 640.0);
    }

    #[test]
    fn script_safe_json_escapes_script_terminators() {
        let value = json!({"a": "</script><!--"});
        let out = script_safe_json(&value);
        assert!(!out.contains("</script>"));
        assert!(!out.contains("<!--"));
        assert!(out.contains("<\\/script>"));
    }
}

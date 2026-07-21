//! Minimal Prometheus text-format exporter + health endpoint.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::{extract::State, routing::get, Router};

use crate::Metrics;

pub async fn serve(addr: String, metrics: Arc<Metrics>) {
    let app = Router::new()
        .route("/metrics", get(render))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(metrics);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("bind metrics addr");
    tracing::info!("metrics server listening on {addr}");
    axum::serve(listener, app).await.expect("metrics server");
}

async fn render(State(m): State<Arc<Metrics>>) -> String {
    let mut out = String::new();
    for (name, help, v) in [
        (
            "ingester_messages_received_total",
            "MQTT messages received",
            &m.messages_received,
        ),
        (
            "ingester_rows_inserted_total",
            "Rows inserted into ClickHouse",
            &m.rows_inserted,
        ),
        (
            "ingester_batch_flushes_total",
            "Batch flushes",
            &m.batch_flushes,
        ),
        (
            "ingester_parse_errors_total",
            "Messages skipped due to parse errors",
            &m.parse_errors,
        ),
        (
            "ingester_insert_errors_total",
            "Failed insert attempts",
            &m.insert_errors,
        ),
        (
            "ingester_rows_dropped_total",
            "Rows dropped after retry exhaustion",
            &m.rows_dropped,
        ),
        (
            "ingester_mqtt_reconnects_total",
            "MQTT reconnect events",
            &m.mqtt_reconnects,
        ),
    ] {
        out.push_str(&format!(
            "# HELP {name} {help}\n# TYPE {name} counter\n{name} {}\n",
            v.load(Ordering::Relaxed)
        ));
    }
    out
}

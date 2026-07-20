//! MQTT → ClickHouse ingester.
//!
//! Subscribes to `sensors/#`, parses JSON payloads, and bulk-inserts into
//! ClickHouse with a two-stage batch: flush every BATCH_MAX_SECS or when
//! sensor_raw buffer reaches BATCH_MAX_ROWS, whichever comes first.
//! Timestamps are assigned from the ingester's receive time (devices are
//! clockless by design).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clickhouse::Client;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

mod metrics_server;
mod parse;

pub use parse::{parse_message, Parsed};

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
pub struct SensorRow {
    #[serde(with = "clickhouse::serde::time::datetime")]
    pub ts: OffsetDateTime,
    pub room: String,
    pub device_id: String,
    pub metric: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
pub struct As7341Row {
    #[serde(with = "clickhouse::serde::time::datetime")]
    pub ts: OffsetDateTime,
    pub device_id: String,
    pub gain: u8,
    pub atime: u16,
    pub astep: u16,
    pub f1_415: u16,
    pub f2_445: u16,
    pub f3_480: u16,
    pub f4_515: u16,
    pub f5_555: u16,
    pub f6_590: u16,
    pub f7_630: u16,
    pub f8_680: u16,
    pub clear: u16,
    pub nir: u16,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
pub struct StatusRow {
    #[serde(with = "clickhouse::serde::time::datetime")]
    pub ts: OffsetDateTime,
    pub device_id: String,
    pub rssi: i16,
    pub uptime_s: u32,
    pub online: bool,
}

#[derive(Default)]
pub struct Metrics {
    pub messages_received: AtomicU64,
    pub rows_inserted: AtomicU64,
    pub batch_flushes: AtomicU64,
    pub parse_errors: AtomicU64,
    pub insert_errors: AtomicU64,
    pub rows_dropped: AtomicU64,
    pub mqtt_reconnects: AtomicU64,
}

struct Config {
    mqtt_host: String,
    mqtt_port: u16,
    mqtt_username: String,
    mqtt_password: String,
    mqtt_client_id: String,
    clickhouse_url: String,
    clickhouse_user: String,
    clickhouse_password: String,
    clickhouse_database: String,
    batch_max_rows: usize,
    batch_max_secs: u64,
    metrics_addr: String,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Config {
    fn from_env() -> Self {
        Self {
            mqtt_host: env_or("MQTT_HOST", "localhost"),
            mqtt_port: env_or("MQTT_PORT", "1883").parse().expect("MQTT_PORT"),
            mqtt_username: env_or("MQTT_USERNAME", "ingester"),
            mqtt_password: env_or("MQTT_PASSWORD", ""),
            mqtt_client_id: env_or("MQTT_CLIENT_ID", "ingester"),
            clickhouse_url: env_or("CLICKHOUSE_URL", "http://localhost:8123"),
            clickhouse_user: env_or("CLICKHOUSE_USER", "ingester"),
            clickhouse_password: env_or("CLICKHOUSE_PASSWORD", ""),
            clickhouse_database: env_or("CLICKHOUSE_DATABASE", "default"),
            batch_max_rows: env_or("BATCH_MAX_ROWS", "5000").parse().expect("BATCH_MAX_ROWS"),
            batch_max_secs: env_or("BATCH_MAX_SECS", "60").parse().expect("BATCH_MAX_SECS"),
            metrics_addr: env_or("METRICS_ADDR", "0.0.0.0:9090"),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = Config::from_env();
    let metrics = Arc::new(Metrics::default());

    let ch = Client::default()
        .with_url(&cfg.clickhouse_url)
        .with_user(&cfg.clickhouse_user)
        .with_password(&cfg.clickhouse_password)
        .with_database(&cfg.clickhouse_database);

    let (tx, rx) = mpsc::channel::<Parsed>(65536);

    tokio::spawn(metrics_server::serve(
        cfg.metrics_addr.clone(),
        metrics.clone(),
    ));

    let batcher = tokio::spawn(batch_loop(
        ch,
        rx,
        cfg.batch_max_rows,
        cfg.batch_max_secs,
        metrics.clone(),
    ));

    mqtt_loop(&cfg, tx, metrics.clone()).await;

    batcher.abort();
    Ok(())
}

/// MQTT receive loop. rumqttc's eventloop reconnects on next poll after an
/// error; we add exponential backoff and re-subscribe on every ConnAck.
async fn mqtt_loop(cfg: &Config, tx: mpsc::Sender<Parsed>, metrics: Arc<Metrics>) {
    let mut opts = MqttOptions::new(&cfg.mqtt_client_id, &cfg.mqtt_host, cfg.mqtt_port);
    opts.set_credentials(&cfg.mqtt_username, &cfg.mqtt_password);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(false);

    let (client, mut eventloop) = AsyncClient::new(opts, 100);
    let mut backoff_secs = 1u64;

    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                info!("connected to MQTT broker, subscribing sensors/#");
                backoff_secs = 1;
                if let Err(e) = client.subscribe("sensors/#", QoS::AtLeastOnce).await {
                    error!("subscribe failed: {e}");
                }
            }
            Ok(Event::Incoming(Packet::Publish(p))) => {
                metrics.messages_received.fetch_add(1, Ordering::Relaxed);
                let now = OffsetDateTime::now_utc();
                match parse_message(&p.topic, &p.payload, now) {
                    Ok(parsed) => {
                        if tx.send(parsed).await.is_err() {
                            error!("batcher channel closed, exiting");
                            return;
                        }
                    }
                    Err(e) => {
                        metrics.parse_errors.fetch_add(1, Ordering::Relaxed);
                        warn!("skipping message on {}: {e}", p.topic);
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                metrics.mqtt_reconnects.fetch_add(1, Ordering::Relaxed);
                warn!("MQTT connection error: {e}; reconnecting in {backoff_secs}s");
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(60);
            }
        }
    }
}

struct Buffers {
    sensor: Vec<SensorRow>,
    as7341: Vec<As7341Row>,
    status: Vec<StatusRow>,
}

impl Buffers {
    fn new() -> Self {
        Self {
            sensor: Vec::new(),
            as7341: Vec::new(),
            status: Vec::new(),
        }
    }
    fn is_empty(&self) -> bool {
        self.sensor.is_empty() && self.as7341.is_empty() && self.status.is_empty()
    }
}

async fn batch_loop(
    ch: Client,
    mut rx: mpsc::Receiver<Parsed>,
    max_rows: usize,
    max_secs: u64,
    metrics: Arc<Metrics>,
) {
    let mut buf = Buffers::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(max_secs));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.reset(); // don't fire immediately

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Some(Parsed::Sensor(rows)) => buf.sensor.extend(rows),
                    Some(Parsed::As7341(row)) => buf.as7341.push(row),
                    Some(Parsed::Status(row)) => buf.status.push(row),
                    None => {
                        flush(&ch, &mut buf, &metrics).await;
                        return;
                    }
                }
                if buf.sensor.len() >= max_rows {
                    flush(&ch, &mut buf, &metrics).await;
                    ticker.reset();
                }
            }
            _ = ticker.tick() => {
                if !buf.is_empty() {
                    flush(&ch, &mut buf, &metrics).await;
                }
            }
        }
    }
}

async fn flush(ch: &Client, buf: &mut Buffers, metrics: &Metrics) {
    // Insert order matches the table's ORDER BY to help ClickHouse merges.
    buf.sensor.sort_by(|a, b| {
        (&a.metric, &a.device_id, a.ts).cmp(&(&b.metric, &b.device_id, b.ts))
    });

    let sensor = std::mem::take(&mut buf.sensor);
    let as7341 = std::mem::take(&mut buf.as7341);
    let status = std::mem::take(&mut buf.status);

    insert_retry(ch, "sensor_raw", &sensor, metrics).await;
    insert_retry(ch, "as7341_raw", &as7341, metrics).await;
    insert_retry(ch, "device_status", &status, metrics).await;

    metrics.batch_flushes.fetch_add(1, Ordering::Relaxed);
}

const INSERT_RETRIES: u32 = 3;

async fn insert_retry<T>(ch: &Client, table: &str, rows: &[T], metrics: &Metrics)
where
    T: for<'a> clickhouse::Row<Value<'a> = T> + clickhouse::RowWrite,
{
    if rows.is_empty() {
        return;
    }
    for attempt in 1..=INSERT_RETRIES {
        match try_insert(ch, table, rows).await {
            Ok(()) => {
                metrics
                    .rows_inserted
                    .fetch_add(rows.len() as u64, Ordering::Relaxed);
                debug!("inserted {} rows into {table}", rows.len());
                return;
            }
            Err(e) => {
                metrics.insert_errors.fetch_add(1, Ordering::Relaxed);
                warn!("insert into {table} failed (attempt {attempt}/{INSERT_RETRIES}): {e}");
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
            }
        }
    }
    // Retries exhausted: drop the batch (accepted data-loss policy) and count it.
    metrics
        .rows_dropped
        .fetch_add(rows.len() as u64, Ordering::Relaxed);
    error!("dropping {} rows for {table} after {INSERT_RETRIES} failed inserts", rows.len());
}

async fn try_insert<T>(ch: &Client, table: &str, rows: &[T]) -> anyhow::Result<()>
where
    T: for<'a> clickhouse::Row<Value<'a> = T> + clickhouse::RowWrite,
{
    let mut insert = ch.insert::<T>(table).await?;
    for row in rows {
        insert.write(row).await?;
    }
    insert.end().await?;
    Ok(())
}

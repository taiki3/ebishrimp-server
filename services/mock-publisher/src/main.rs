//! Mock sensor fleet: publishes spec-compliant MQTT messages until replaced by
//! real ESP32-C6 devices in Phase 2.
//!
//! 3 rooms x 2 devices. One device is an AS7341 (measurement-event payload),
//! the rest publish plain metric JSON. Each device holds its own MQTT
//! connection with a retained Last Will of {"online": 0} on its status topic,
//! so killing this process (SIGKILL) flips devices offline on the dashboard.

use std::time::{Duration, Instant};

use rand::Rng;
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS};
use serde_json::json;
use tracing::{info, warn};

#[derive(Clone, Copy)]
enum Kind {
    Env { co2: bool },
    As7341,
}

struct Device {
    room: &'static str,
    id: &'static str,
    kind: Kind,
}

const DEVICES: &[Device] = &[
    Device { room: "living", id: "env-living-1", kind: Kind::Env { co2: true } },
    Device { room: "living", id: "as7341-living-1", kind: Kind::As7341 },
    Device { room: "bedroom", id: "env-bedroom-1", kind: Kind::Env { co2: true } },
    Device { room: "bedroom", id: "env-bedroom-2", kind: Kind::Env { co2: false } },
    Device { room: "kitchen", id: "env-kitchen-1", kind: Kind::Env { co2: true } },
    Device { room: "kitchen", id: "env-kitchen-2", kind: Kind::Env { co2: false } },
];

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let host = env_or("MQTT_HOST", "localhost");
    let port: u16 = env_or("MQTT_PORT", "1883").parse().expect("MQTT_PORT");
    let user = env_or("MQTT_USERNAME", "device");
    let pass = env_or("MQTT_PASSWORD", "");
    let interval: u64 = env_or("INTERVAL_SECS", "10").parse().expect("INTERVAL_SECS");
    let status_interval: u64 = env_or("STATUS_INTERVAL_SECS", "300")
        .parse()
        .expect("STATUS_INTERVAL_SECS");

    let mut tasks = Vec::new();
    for (i, dev) in DEVICES.iter().enumerate() {
        let (host, user, pass) = (host.clone(), user.clone(), pass.clone());
        tasks.push(tokio::spawn(async move {
            run_device(dev, host, port, user, pass, interval, status_interval, i).await;
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_device(
    dev: &Device,
    host: String,
    port: u16,
    user: String,
    pass: String,
    interval: u64,
    status_interval: u64,
    phase: usize,
) {
    let data_topic = format!("sensors/{}/{}", dev.room, dev.id);
    let status_topic = format!("{data_topic}/status");

    let mut opts = MqttOptions::new(dev.id, &host, port);
    opts.set_credentials(&user, &pass);
    opts.set_keep_alive(Duration::from_secs(15));
    opts.set_last_will(LastWill::new(
        &status_topic,
        r#"{"online": 0}"#,
        QoS::AtLeastOnce,
        true,
    ));

    let (client, mut eventloop) = AsyncClient::new(opts, 64);

    // Drain the event loop; rumqttc reconnects on the next poll after errors.
    let id = dev.id;
    tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => info!("{id}: connected"),
                Ok(_) => {}
                Err(e) => {
                    warn!("{id}: connection error: {e}");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    });

    let start = Instant::now();
    let mut data_tick = tokio::time::interval(Duration::from_secs(interval));
    let mut status_tick = tokio::time::interval(Duration::from_secs(status_interval));

    loop {
        tokio::select! {
            _ = data_tick.tick() => {
                let t = start.elapsed().as_secs_f64() + phase as f64 * 100.0;
                let payload = match dev.kind {
                    Kind::Env { co2 } => env_payload(t, co2),
                    Kind::As7341 => as7341_payload(t),
                };
                publish(&client, &data_topic, payload, false).await;
            }
            _ = status_tick.tick() => {
                let uptime = start.elapsed().as_secs();
                let rssi = -55 - rand::rng().random_range(0..15);
                let payload = json!({"rssi": rssi, "uptime_s": uptime, "online": 1}).to_string();
                publish(&client, &status_topic, payload, true).await;
            }
        }
    }
}

async fn publish(client: &AsyncClient, topic: &str, payload: String, retain: bool) {
    if let Err(e) = client
        .publish(topic, QoS::AtLeastOnce, retain, payload)
        .await
    {
        warn!("publish to {topic} failed: {e}");
    }
}

fn wave(t: f64, base: f64, amp: f64, period_s: f64, noise: f64) -> f64 {
    let mut rng = rand::rng();
    base + amp * (2.0 * std::f64::consts::PI * t / period_s).sin()
        + rng.random_range(-noise..=noise)
}

fn env_payload(t: f64, co2: bool) -> String {
    let temperature = (wave(t, 25.0, 3.0, 3600.0, 0.2) * 10.0).round() / 10.0;
    let humidity = (wave(t, 50.0, 10.0, 5400.0, 1.0) * 10.0).round() / 10.0;
    if co2 {
        let co2v = wave(t, 650.0, 150.0, 1800.0, 15.0).round();
        json!({"temperature": temperature, "humidity": humidity, "co2": co2v}).to_string()
    } else {
        json!({"temperature": temperature, "humidity": humidity}).to_string()
    }
}

fn as7341_payload(t: f64) -> String {
    let ch = |base: f64| wave(t, base, base * 0.3, 2700.0, base * 0.05).max(0.0) as u16;
    json!({
        "gain": 8, "atime": 29, "astep": 599,
        "f1_415": ch(1000.0), "f2_445": ch(1100.0), "f3_480": ch(1250.0),
        "f4_515": ch(1500.0), "f5_555": ch(1800.0), "f6_590": ch(1650.0),
        "f7_630": ch(1400.0), "f8_680": ch(1200.0),
        "clear": ch(4000.0), "nir": ch(800.0)
    })
    .to_string()
}

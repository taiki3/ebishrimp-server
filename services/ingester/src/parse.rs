//! Topic/payload parsing and routing.
//!
//! Routing contract (spec §6.3 / §7):
//! - topic `sensors/<room>/<device_id>/status` → device_status
//! - payload containing key `f1_415`           → as7341_raw
//! - otherwise                                 → sensor_raw, one row per numeric key

use serde_json::Value;
use time::OffsetDateTime;

use crate::{As7341Row, SensorRow, StatusRow};

#[derive(Debug)]
pub enum Parsed {
    Sensor(Vec<SensorRow>),
    As7341(As7341Row),
    Status(StatusRow),
}

pub fn parse_message(topic: &str, payload: &[u8], ts: OffsetDateTime) -> anyhow::Result<Parsed> {
    let parts: Vec<&str> = topic.split('/').collect();
    let (room, device_id, is_status) = match parts.as_slice() {
        ["sensors", room, device] => (*room, *device, false),
        ["sensors", _room, device, "status"] => (*_room, *device, true),
        _ => anyhow::bail!("unrecognized topic shape: {topic}"),
    };

    let json: Value = serde_json::from_slice(payload)
        .map_err(|e| anyhow::anyhow!("invalid JSON payload: {e}"))?;
    let obj = json
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("payload is not a JSON object"))?;

    if is_status {
        return Ok(Parsed::Status(StatusRow {
            ts,
            device_id: device_id.to_string(),
            rssi: get_i64(obj, "rssi").unwrap_or(0) as i16,
            uptime_s: get_i64(obj, "uptime_s").unwrap_or(0).max(0) as u32,
            online: get_i64(obj, "online").map(|v| v != 0).unwrap_or(false),
        }));
    }

    if obj.contains_key("f1_415") {
        let g = |k: &str| get_i64(obj, k).unwrap_or(0).clamp(0, u16::MAX as i64) as u16;
        return Ok(Parsed::As7341(As7341Row {
            ts,
            device_id: device_id.to_string(),
            gain: get_i64(obj, "gain").unwrap_or(0).clamp(0, 255) as u8,
            atime: g("atime"),
            astep: g("astep"),
            f1_415: g("f1_415"),
            f2_445: g("f2_445"),
            f3_480: g("f3_480"),
            f4_515: g("f4_515"),
            f5_555: g("f5_555"),
            f6_590: g("f6_590"),
            f7_630: g("f7_630"),
            f8_680: g("f8_680"),
            clear: g("clear"),
            nir: g("nir"),
        }));
    }

    // Vertical table: one row per numeric metric. Bools map to 0/1,
    // non-numeric values are skipped (logged by caller via row count 0).
    let mut rows = Vec::with_capacity(obj.len());
    for (key, value) in obj {
        let num = match value {
            Value::Number(n) => n.as_f64(),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        };
        if let Some(v) = num {
            rows.push(SensorRow {
                ts,
                room: room.to_string(),
                device_id: device_id.to_string(),
                metric: key.clone(),
                value: v,
            });
        }
    }
    if rows.is_empty() {
        anyhow::bail!("payload contained no numeric metrics");
    }
    Ok(Parsed::Sensor(rows))
}

fn get_i64(obj: &serde_json::Map<String, Value>, key: &str) -> Option<i64> {
    obj.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const TS: OffsetDateTime = datetime!(2026-07-20 12:00:00 UTC);

    #[test]
    fn routes_plain_metrics_to_sensor_rows() {
        let p = parse_message(
            "sensors/living/env-1",
            br#"{"temperature": 25.3, "humidity": 48.2, "co2": 640}"#,
            TS,
        )
        .unwrap();
        match p {
            Parsed::Sensor(rows) => {
                assert_eq!(rows.len(), 3);
                assert!(rows
                    .iter()
                    .all(|r| r.room == "living" && r.device_id == "env-1"));
                let co2 = rows.iter().find(|r| r.metric == "co2").unwrap();
                assert_eq!(co2.value, 640.0);
            }
            _ => panic!("expected Sensor"),
        }
    }

    #[test]
    fn routes_status_topic() {
        let p = parse_message(
            "sensors/living/env-1/status",
            br#"{"rssi": -61, "uptime_s": 86400, "online": 1}"#,
            TS,
        )
        .unwrap();
        match p {
            Parsed::Status(r) => {
                assert_eq!(r.rssi, -61);
                assert_eq!(r.uptime_s, 86400);
                assert!(r.online);
            }
            _ => panic!("expected Status"),
        }
    }

    #[test]
    fn last_will_offline() {
        let p = parse_message("sensors/living/env-1/status", br#"{"online": 0}"#, TS).unwrap();
        match p {
            Parsed::Status(r) => assert!(!r.online),
            _ => panic!("expected Status"),
        }
    }

    #[test]
    fn routes_as7341_payload() {
        let p = parse_message(
            "sensors/living/as7341-1",
            br#"{"gain": 8, "atime": 29, "astep": 599,
                "f1_415": 1023, "f2_445": 980, "f3_480": 1100, "f4_515": 1500,
                "f5_555": 1800, "f6_590": 1600, "f7_630": 1400, "f8_680": 1200,
                "clear": 4000, "nir": 800}"#,
            TS,
        )
        .unwrap();
        match p {
            Parsed::As7341(r) => {
                assert_eq!(r.gain, 8);
                assert_eq!(r.f1_415, 1023);
                assert_eq!(r.nir, 800);
            }
            _ => panic!("expected As7341"),
        }
    }

    #[test]
    fn bools_become_01_and_strings_skipped() {
        let p = parse_message(
            "sensors/living/env-1",
            br#"{"door_open": true, "note": "hello", "temp": 20}"#,
            TS,
        )
        .unwrap();
        match p {
            Parsed::Sensor(rows) => {
                assert_eq!(rows.len(), 2);
                assert_eq!(
                    rows.iter().find(|r| r.metric == "door_open").unwrap().value,
                    1.0
                );
            }
            _ => panic!("expected Sensor"),
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_message("sensors/living/env-1", b"not json", TS).is_err());
        assert!(parse_message("other/topic", b"{}", TS).is_err());
        assert!(parse_message("sensors/living/env-1", br#"{"a": "b"}"#, TS).is_err());
    }
}

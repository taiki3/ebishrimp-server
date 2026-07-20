#!/usr/bin/env bash
# Quick E2E verification queries against the in-cluster ClickHouse.
set -euo pipefail
export KUBECONFIG="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml}"
PASS=$(kubectl -n iot get secret clickhouse-credentials -o jsonpath='{.data.admin-password}' | base64 -d)
q() { kubectl -n iot exec chi-iot-main-0-0-0 -c clickhouse -- clickhouse-client --user admin --password "$PASS" --query "$1"; }
echo "sensor_raw rows:";      q "SELECT count() FROM sensor_raw"
echo "sensor_1m rows:";      q "SELECT count() FROM sensor_1m"
echo "as7341_raw rows:";     q "SELECT count() FROM as7341_raw"
echo "device_status rows:";  q "SELECT count() FROM device_status"
echo "latest per metric:";   q "SELECT metric, device_id, argMax(value, ts) v, max(ts) FROM sensor_raw GROUP BY metric, device_id ORDER BY metric, device_id FORMAT PrettyCompact"

#!/usr/bin/env bash
# Local (no-GitHub) deployment: builds images, imports them into k3s
# containerd, installs Flux controllers, and applies all manifests with
# SOPS secrets decrypted on the fly. Once the repo is on GitHub, switch to
# scripts/bootstrap-flux.sh for full GitOps.
set -euo pipefail
cd "$(dirname "$0")/.."
export KUBECONFIG="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml}"

echo "== build images =="
docker build -t ghcr.io/taiki3/ebishrimp-rumqttd:0.20.0 docker/rumqttd
docker build -t ghcr.io/taiki3/ebishrimp-ingester:dev -f services/ingester/Dockerfile .
docker build -t ghcr.io/taiki3/ebishrimp-dashboard:dev -f services/dashboard/Dockerfile .
docker build -t ghcr.io/taiki3/ebishrimp-mock-publisher:dev -f services/mock-publisher/Dockerfile .

echo "== import images into k3s containerd =="
for img in ghcr.io/taiki3/ebishrimp-rumqttd:0.20.0 \
           ghcr.io/taiki3/ebishrimp-ingester:dev \
           ghcr.io/taiki3/ebishrimp-dashboard:dev \
           ghcr.io/taiki3/ebishrimp-mock-publisher:dev; do
  docker save "$img" | sudo k3s ctr images import -
done

echo "== flux controllers =="
flux check --pre
flux install

echo "== infrastructure =="
kubectl apply -f infrastructure/namespaces.yaml
kubectl apply -k infrastructure

echo "== waiting for HelmReleases (operator CRDs, prometheus CRDs) =="
kubectl -n clickhouse-operator wait helmrelease/clickhouse-operator --for=condition=Ready --timeout=10m
kubectl -n monitoring wait helmrelease/kube-prometheus-stack --for=condition=Ready --timeout=20m

echo "== apps (secrets decrypted via sops) =="
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cp -r apps "$TMP/"
find "$TMP" -name '*.enc.yaml' -exec sops -d -i {} \;
kubectl apply -k "$TMP/apps"

echo "== done. watch: kubectl -n iot get pods -w =="

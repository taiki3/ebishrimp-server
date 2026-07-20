#!/usr/bin/env bash
# Full GitOps bootstrap once the repo exists on GitHub.
# Requires: GITHUB_TOKEN with repo scope, gh repo created (ebishrimp-server).
set -euo pipefail
export KUBECONFIG="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml}"

flux bootstrap github \
  --owner=taiki3 \
  --repository=ebishrimp-server \
  --branch=main \
  --path=clusters/n150 \
  --personal

# Give kustomize-controller the age key for SOPS decryption
kubectl -n flux-system create secret generic sops-age \
  --from-file=age.agekey="$HOME/.config/sops/age/keys.txt" \
  --dry-run=client -o yaml | kubectl apply -f -

#!/usr/bin/env bash
# k8s マニフェストの静的検証。CI (.github/workflows/manifests.yml) と
# `make lint-manifests` の両方から呼ばれる。
#
# 検証できないもの: clickhouse-operator が CHI から生成する ClickHouse 設定の
# 意味的な妥当性。これは実際に Pod を起動しないと分からない層で、過去に
# system ログの <ttl> と operator 生成の <engine> が衝突して ClickHouse が
# 起動不能になった事故はここでは防げない (docs/phase1-verification.md 参照)。
set -euo pipefail

cd "$(dirname "$0")/.."
fail=0
note() { printf '\n\033[1m== %s\033[0m\n' "$1"; }

# ---------------------------------------------------------------- kustomize
note "kustomize build"
for d in apps infrastructure clusters/n150/flux-system; do
  if out=$(kubectl kustomize "$d" 2>&1); then
    printf '  ok    %s (%s docs)\n' "$d" "$(grep -c '^---' <<<"$out" || true)"
  else
    printf '  FAIL  %s\n' "$d"; sed 's/^/        /' <<<"$out" | head -20; fail=1
  fi
done

# ---------------------------------------------------------------- kubeconform
note "kubeconform"
if command -v kubeconform >/dev/null; then
  # Secret は SOPS 暗号化で `sops:` キーが増えるため -strict から除外する。
  # CRDs-catalog に無い CRD (ClickHouseInstallation 等) は
  # -ignore-missing-schemas で素通りするので、-summary で件数を必ず表示する。
  CRD='https://raw.githubusercontent.com/datreeio/CRDs-catalog/main/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json'
  for d in apps infrastructure; do
    printf '  %s\n' "$d"
    kubectl kustomize "$d" | kubeconform \
      -strict -summary -skip Secret -ignore-missing-schemas \
      -schema-location default -schema-location "$CRD" \
      2>&1 | sed 's/^/        /' || fail=1
  done
  printf '  clusters/n150 (素の Flux CR)\n'
  kubeconform -strict -summary -ignore-missing-schemas \
    -schema-location default -schema-location "$CRD" \
    clusters/n150/*.yaml 2>&1 | sed 's/^/        /' || fail=1
else
  printf '  skip  kubeconform 未インストール (CI では必ず実行される)\n'
  printf '        go install github.com/yannh/kubeconform/cmd/kubeconform@latest\n'
fi

# ---------------------------------------------------------------- XML
# CHI の configuration.files に埋め込む ClickHouse 設定は YAML の中の文字列
# なので、YAML が通っても XML として壊れていることがある。整形式かどうかだけ
# は静的に確認できる (意味的な妥当性は上のコメントの通り確認できない)。
note "CHI 埋め込み XML の整形式チェック"
if ! python3 -c 'import yaml' 2>/dev/null; then
  printf '  skip  PyYAML 未インストール (CI では必ず実行される)\n'
  printf '        pip install --user pyyaml\n'
elif ! command -v xmllint >/dev/null; then
  printf '  skip  xmllint 未インストール (CI では必ず実行される)\n'
  printf '        sudo pacman -S libxml2   # Debian 系は libxml2-utils\n'
else
python3 - <<'PY' || fail=1
import glob, subprocess, sys, yaml

checked = failed = 0
for path in glob.glob("apps/**/*.yaml", recursive=True):
    with open(path) as fh:
        try:
            docs = list(yaml.safe_load_all(fh))
        except yaml.YAMLError as e:
            print(f"  FAIL  {path}: YAML パース不可: {e}"); failed += 1; continue
    for doc in docs:
        if not isinstance(doc, dict) or doc.get("kind") != "ClickHouseInstallation":
            continue
        files = (doc.get("spec", {}).get("configuration", {}) or {}).get("files") or {}
        for name, content in files.items():
            if not name.endswith(".xml"):
                continue
            checked += 1
            r = subprocess.run(["xmllint", "--noout", "-"],
                               input=content, text=True, capture_output=True)
            if r.returncode:
                print(f"  FAIL  {path} :: {name}")
                print("\n".join("        " + l for l in r.stderr.strip().splitlines()))
                failed += 1
            else:
                print(f"  ok    {path} :: {name}")
print(f"  {checked} 件中 {failed} 件が不正")
sys.exit(1 if failed else 0)
PY
fi

note "結果"
if [ "$fail" -eq 0 ]; then echo "  すべて通過"; else echo "  失敗あり"; fi
exit "$fail"

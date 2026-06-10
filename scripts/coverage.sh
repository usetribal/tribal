#!/usr/bin/env bash
set -euo pipefail

cargo llvm-cov --workspace --summary-only \
  --ignore-filename-regex 'src/main\.rs$' \
  --json --output-path /tmp/lineage-cov.json >/dev/null

line_pct="$(python3 - <<'PY'
import json
with open("/tmp/lineage-cov.json") as f:
    data = json.load(f)
print(f"{data['data'][0]['totals']['lines']['percent']:.2f}")
PY
)"

region_pct="$(python3 - <<'PY'
import json
with open("/tmp/lineage-cov.json") as f:
    data = json.load(f)
print(f"{data['data'][0]['totals']['regions']['percent']:.2f}")
PY
)"

echo "Line coverage: ${line_pct}%"
echo "Region coverage: ${region_pct}%"

cargo llvm-cov --workspace --summary-only \
  --ignore-filename-regex 'src/main\.rs$' 2>/dev/null | tail -3

python3 - <<PY
line = float("${line_pct}")
if line < 80.0:
    raise SystemExit(f"line coverage {line:.2f}% is below 80%")
print("coverage gate passed (>=80% lines)")
PY

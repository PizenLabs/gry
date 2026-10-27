#!/usr/bin/env bash
# Reproducible local benchmark. Usage: scripts/benchmark.sh <repo> [name]
set -euo pipefail
REPO="${1:?usage: scripts/benchmark.sh <repo> [name]}"
NAME="${2:-$(basename "$(cd "$REPO" && pwd)")}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GRY="${GRY_BIN:-$ROOT/target/release/gry}"
CACHE="${GRY_CACHE_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/gry-bench.XXXXXX")}"
export GRY_CACHE_DIR="$CACHE"
echo "cache=$CACHE project=$NAME repo=$REPO"
/usr/bin/time -l "$GRY" index "$REPO" --name "$NAME" >/tmp/gry-bench-index.json
python3 -c 'import json; d=json.load(open("/tmp/gry-bench-index.json")); print({k:d[k] for k in ["files","source_bytes","nodes","edges","duration_ms","parsed_files","reused_files"]})'
/usr/bin/time -l "$GRY" index "$REPO" --name "$NAME" >/tmp/gry-bench-unchanged.json
python3 -c 'import json; d=json.load(open("/tmp/gry-bench-unchanged.json")); print({k:d[k] for k in ["unchanged","duration_ms","parsed_files","reused_files"]})'
for args in \
  "search $NAME handler" \
  "query $NAME 'MATCH (f:Function) RETURN f.name LIMIT 20'" \
  "architecture $NAME --aspect overview" \
  "context $NAME --task handler --max-tokens 1600" \
  "hook before_plan --project $NAME --task handler --max-tokens 1600" \
  "schema $NAME"; do
  echo "--- gry $args"
  /usr/bin/time -p sh -c "$GRY $args >/dev/null" 2>&1 | tail -3
done

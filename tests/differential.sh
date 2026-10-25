#!/usr/bin/env bash
# Differential check against the CBM reference binary. Requires
# `cbm` on PATH (or CBM_BIN). This is a semantic comparison,
# not a byte-for-byte one: representation and version differences are reported.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GRY="${GRY_BIN:-$ROOT/target/release/gry}"
CBM="${CBM_BIN:-cbm}"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/gry-diff.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

if ! command -v "$CBM" >/dev/null 2>&1; then
  echo "differential: CBM binary not found ($CBM); skipping" >&2
  exit 0
fi
if [ ! -x "$GRY" ]; then
  echo "differential: gry binary not found ($GRY); build with: cargo build --release" >&2
  exit 1
fi

REPO="$TMP/repo"
mkdir -p "$REPO/src" "$REPO/tests" "$REPO/docs"
cat > "$REPO/src/lib.rs" <<'RUST'
use crate::parser::Parser;
pub struct Engine { count: usize }
impl Engine {
    /// Parse a request.
    pub fn parse(&mut self, input: &str) -> usize {
        let p = Parser::new();
        p.run(input)
    }
}
fn main() { let mut e = Engine { count: 0 }; e.parse("x"); }
RUST
cat > "$REPO/src/parser.rs" <<'RUST'
pub struct Parser;
impl Parser {
    pub fn new() -> Self { Parser }
    pub fn run(&self, input: &str) -> usize { input.len() }
}
RUST
cat > "$REPO/tests/test_engine.rs" <<'RUST'
#[test]
fn test_parse() { let mut e = crate::Engine { count: 0 }; assert_eq!(e.parse("abc"), 3); }
RUST
cat > "$REPO/docs/guide.md" <<'MD'
# Guide

The `Engine` parses requests. See [parser](src/parser.rs).
MD

export GRY_CACHE_DIR="$TMP/gry-cache"
CBM_NAME="gry-diff-cbm-$$"
GRY_NAME="gry-diff"

"$GRY" index "$REPO" --name "$GRY_NAME" >"$TMP/gry-index.json"
"$CBM" cli index_repository --repo-path "$REPO" --name "$CBM_NAME" >"$TMP/cbm-index.json"

"$GRY" query "$GRY_NAME" 'MATCH (n) RETURN n.qn, n.label' >"$TMP/gry-nodes.json"
"$CBM" cli query_graph --project "$CBM_NAME" --query 'MATCH (n) RETURN n.qn, n.label' --format json >"$TMP/cbm-nodes.json"
"$GRY" query "$GRY_NAME" 'MATCH (a)-[:CALLS]->(b) RETURN a.qn, b.qn' >"$TMP/gry-calls.json"
"$CBM" cli query_graph --project "$CBM_NAME" --query 'MATCH (a)-[:CALLS]->(b) RETURN a.qn, b.qn' --format json >"$TMP/cbm-calls.json"

python3 - "$TMP" "$GRY_NAME" "$CBM_NAME" <<'PY'
import json, sys
tmp, gry_name, cbm_name = sys.argv[1:4]

def load(path):
    with open(path) as f:
        return json.load(f)

g = load(f"{tmp}/gry-nodes.json")
c = load(f"{tmp}/cbm-nodes.json")
INFRA = {"Image","EnvVar","Process","Channel","Resource","Chart","ADR","Diagram","DiagramElement"}
def normalize(rows, project, skip_infra=True):
    out = set()
    for qn, label in rows:
        if not isinstance(qn, str):
            continue
        if label in INFRA and skip_infra:
            continue
        qn = qn.replace(project + ".", "", 1)
        if qn.startswith(project):
            qn = qn[len(project):].lstrip(".")
        out.add((qn, label))
    return out

gset = normalize(g["rows"], gry_name)
cset = normalize(c["rows"], cbm_name)
common = gset & cset
only_g = sorted(gset - cset)
only_c = sorted(cset - gset)

def calls(path, project):
    rows = load(path)["rows"]
    out = set()
    for a, b in rows:
        a = a.replace(project + ".", "", 1) if isinstance(a, str) else a
        b = b.replace(project + ".", "", 1) if isinstance(b, str) else b
        out.add((a, b))
    return out

gc = calls(f"{tmp}/gry-calls.json", gry_name)
cc = calls(f"{tmp}/cbm-calls.json", cbm_name)
print(f"differential: nodes common={len(common)} gry_only={len(only_g)} cbm_only={len(only_c)}")
print(f"differential: CALLS common={len(gc & cc)} gry_only={len(gc - cc)} cbm_only={len(cc - gc)}")
if only_g:
    print("  gry-only nodes (first 12):", only_g[:12])
if only_c:
    print("  CBM-only nodes (first 12):", only_c[:12])
if gc - cc:
    print("  gry-only CALLS (first 8):", sorted(gc - cc)[:8])
if cc - gc:
    print("  CBM-only CALLS (first 8):", sorted(cc - gc)[:8])
# Common code symbols are the migration-critical contract.
critical = {n for n in only_c if n[1] in {"Function","Method","Class","Struct","Interface","Trait","Enum","Type","Field","Variable","Constant","Macro"}}
if critical:
    print(f"differential: FAIL missing {len(critical)} CBM code symbols")
    sys.exit(1)
print("differential: PASS (code symbols preserved; representation differences above)")
PY

"$CBM" cli delete_project --project "$CBM_NAME" >/dev/null 2>&1 || true

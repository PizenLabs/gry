# Development and verification

This guide covers building, testing, benchmarking and releasing gry.

## Workspace layout

```
crates/
  gry-core/    domain model, QN, language table, config, MinHash, Nomic vectors
  gry-store/   SQLite schema, CRUD, FTS, traversal, coverage/traces/ADR, artifact
  gry-parse/   grammars, extraction, docs/config/infra analyzers
  gry-index/   discovery, pipeline, delta indexing, resolution, jobs, watcher
  gry-query/   search, snippets, trace, architecture, Cypher, impact, test impact
  gry-mcp/     JSON-RPC transport, tool schemas, handlers, prompts, rendering
  gry-cli/     the `gry` binary and its commands
docs/          architecture, capabilities, benchmarks, this guide
tests/         differential fixture and helper scripts
scripts/       benchmark helper
third_party/   Nomic vectors, license and attribution
```

## Requirements

- Rust 1.85+ (edition 2021)
- A C toolchain for tree-sitter grammars and bundled SQLite
- Git, for change-impact and history-coupling features
- Optional: `codebase-memory-mcp` (`cbm`) on `PATH` for the differential check

## Build

```bash
cargo build --release
./target/release/gry --version
```

Install from the workspace:

```bash
cargo install --path crates/gry-cli
```

Development builds use `opt-level = 1` for the workspace and `opt-level = 2` for dependencies.

## Test suite

```bash
cargo test --workspace
```

Current result: **118 tests pass across 19 suites** (`cargo test --workspace`, 2026-10-26).

| Area | Coverage |
|---|---|
| `gry-core` | QN, language detection, configuration, MinHash, Nomic vectors, settings |
| `gry-store` | schema/publication, search, traversal, ADR/traces, artifact |
| `gry-parse` | grammar registry, extraction (Rust/Python/JS/C), docs, config, infra, lexical fallback |
| `gry-index` | full/delta indexing, deletion, artifact round-trip, cross-repo linking, Git-history coupling, failed/cancelled generation preservation, incremental-vs-clean consistency (Rust and C, late imports/config/docs) |
| `gry-query` | search/snippets/outlines, Cypher (including snapshot cursors), trace, schema, coverage, compare, architecture, ADR, traces |
| `gry-mcp` | all 17 tools, prompts, profiles, errors |
| `gry-cli` | index/search/search-code/status/config/delete, doctor, compact `--json`, memory budget, generic tool, stdio server, install/uninstall, async, test-impact select/publish |

Test counts by crate: core 23, store 6, parse 31, index 8 unit + 11 integration, query 22 unit +
2 integration, mcp 6 unit + 2 contract, cli 7.

## Verification checklist

Run the complete local verification set before a release:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # when available
cargo test --workspace
cargo build --release --bin gry
./target/release/gry --version
./target/release/gry doctor --json
tests/differential.sh                                    # optional
```

The checklist covers:

1. **Unit tests** for QN, language detection, tokenization, MinHash, AST profile, Cypher
   expressions, policy rules, config edits and output budgeting.
2. **Integration tests** with real SQLite and temporary repositories: full/incremental index, graph
   semantics, search, snippets, traversal, coverage, traces, ADR, artifact round-trip.
3. **Regression tests** for every defect fixed and every accepted limit.
4. **Incremental consistency**: add/edit/delete/rename sequences compared with a clean full rebuild.
5. **MCP contract tests**: initialize, tools/list, each tool success and representative failures,
   prompts, JSON-RPC errors, pagination, cancellation.
6. **CLI tests**: first-class commands, generic tool invocation, install/uninstall, async jobs.
7. **Clean build/install**: release build from a clean checkout, `--version`, index a fixture, serve
   MCP, and run with no network access at runtime.
8. **Scope audit**: no viewer/browser UI dependency or asset in the binary.

## Manual end-to-end smoke

```bash
export GRY_CACHE_DIR=/tmp/gry-smoke
gry index /path/to/repo --name smoke
gry status smoke --diagnostics summary
gry doctor
gry search smoke handler --label Function
gry query smoke "MATCH (f:Function)-[:CALLS]->(g) RETURN f.name, g.name LIMIT 10"
gry cli get_architecture --project smoke --aspect overview
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | gry serve
# edit one file, then (unchanged files must not be parsed):
gry index /path/to/repo --name smoke
```

## Benchmarks

See [BENCHMARKS.md](BENCHMARKS.md) for the measured environment, workloads, commands and results.
Re-run with `/usr/bin/time -l` on macOS or `/usr/bin/time -v` on Linux and record the same fields.
`scripts/benchmark.sh` reproduces the full/unchanged and query portions.

## Differential check

`tests/differential.sh` indexes a Rust fixture with gry and with `codebase-memory-mcp` 0.11.0,
normalizes the project prefix, and compares code nodes and `CALLS` edges. It is an optional
compatibility check for the shared graph contract, not an acceptance requirement.

```bash
tests/differential.sh
# or:
CBM_BIN=/path/to/cbm tests/differential.sh
```

Expected output:

```
differential: nodes common=23 gry_only=0 cbm_only=0
differential: CALLS common=3 gry_only=1 cbm_only=0
differential: PASS (code symbols preserved)
```

The one extra `CALLS` edge is the test-body call (`test_parse → Engine.parse`), which the reference
records as `USAGE`. gry also emits `TESTS` from it; this is intentional for test impact.

The check is skipped with a clear message when the reference binary is not installed.

## Release process

1. Update `CHANGELOG.md` and the version in the workspace `Cargo.toml`.
2. Run the verification checklist on a clean checkout.
3. Confirm `docs/CAPABILITIES.md` still matches the implementation and that every known limit is
   listed.
4. Build with `cargo build --release --bin gry` and record the binary size.
5. Tag the release and attach the binary plus checksums.

`gry update` is intentionally not implemented: installs are managed by the package manager or by
rebuilding from source.

## Documentation conventions

- [README.md](../README.md) is the product front door: what gry is, install, quick start, MCP setup.
- [ARCHITECTURE.md](ARCHITECTURE.md) explains how the system works and its contracts.
- [CAPABILITIES.md](CAPABILITIES.md) is the coverage matrix and the single place for known limits.
- [BENCHMARKS.md](BENCHMARKS.md) is the single place for measured numbers.
- [CONTRIBUTING.md](../CONTRIBUTING.md) is the contributor workflow and pull-request checklist.
- [SECURITY.md](../SECURITY.md) is the vulnerability reporting policy and threat scope.
- [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) is the community behavior standard.
- This guide is the single place for build, test and release procedure.

When behavior changes, update the capability matrix and the changelog together. Keep measured claims
in `BENCHMARKS.md` and link to them rather than copying numbers into other documents.

# Benchmarks

All numbers are measured, not estimated. Environment, workload and commands are recorded so results
can be reproduced or challenged.

## Environment

- Host: macOS (Darwin) on Apple silicon (arm64)
- Toolchain: `rustc 1.99.1`, `cargo 1.99.1` (Homebrew)
- Build: `cargo build --release` (thin LTO, 4 codegen units, stripped debuginfo)
- Storage: bundled SQLite via `rusqlite`, WAL mode, `synchronous=NORMAL`
- No network access at runtime; no daemon
- Release binary: **151 MB** (68 statically linked grammars + 30 MB of Nomic vectors)

## Workload A — dense C repository

A real C codebase with a small number of very large files:

- **341 files**, **12,024,707 bytes** of C source
- **20,753 nodes**, **111,882 edges** (`full` mode)
- Includes a 12,618-line file; this is a worst case for single-file parse latency.

### Indexing

| Operation | Wall time (p50) | Peak RSS | Notes |
|---|---:|---:|---|
| Full index | **3.46 s** | **654 MB** | 341 files parsed; 20,753 nodes / 111,882 edges |
| Unchanged re-index | **12 ms** | **16 MB** | 0 files parsed; 341 cache hits |
| Comment-only edit | **188 ms** | — | one file parsed; nodes-only delta (no edge work) |
| Local structural edit | **363 ms** | — | one file parsed + 2 module siblings re-resolved |
| Public-symbol edit | ~1.5 s | — | 74 dependent files re-resolved (correct, wider invalidation) |

The public-symbol case is intentionally not a fixed-cost operation: changing a macro or function
used by 74 files re-resolves those files. A private body edit stays in the 180–370 ms range.

### Query latency

CLI process time on workload A, 5 runs each (includes ~9 ms process start):

| Operation | p50 | p95 |
|---|---:|---:|
| `gry --version` | 9.5 ms | 10.1 ms |
| Structural search (50 rows) | 14.1 ms | 14.5 ms |
| BM25 search (20 rows) | 15.4 ms | 15.6 ms |
| Semantic search (20 rows) | 97.8 ms | 98.9 ms |
| Trace, outbound | 12.5 ms | 13.4 ms |
| Cypher, simple MATCH + LIMIT 20 | 304.8 ms | 316.5 ms |
| Architecture overview | 320.7 ms | 330.5 ms |
| Schema (counts/patterns) | 124.3 ms | 124.9 ms |
| `search_code` (10 result files) | 81.6 ms | 87.0 ms |
| `doctor` (integrity check of 88 MB DB) | 505 ms | 511 ms |

Cypher/architecture/schema load project aggregates per process; an MCP server amortizes this across
calls. Trace fetches nodes on demand rather than loading the full graph.

## Workload B — 10k medium files

A synthetic C repository with 10,001 files (4 declarations per file, shared header), designed to
match the performance target's repository shape:

- **10,001 files**, **50,056 nodes**, **80,055 edges**
- Full index: **3.57 s** = ~2,800 files/s, peak RSS **749 MB**
- Comment-only one-file edit: **p50 233 ms** (6 runs, 222–231 ms reported index time)
- Unchanged: ~15 ms

This workload meets the two headline targets:

- incremental single-file update **< 300 ms p50** (233 ms)
- full index throughput **≥ 800 files/s** (2,800 files/s)

## Measurement commands

```bash
export GRY_CACHE_DIR=/tmp/gry-bench
/usr/bin/time -l ./target/release/gry index /tmp/repo --name bench
/usr/bin/time -l ./target/release/gry index /tmp/repo --name bench   # unchanged
# edit one file, then:
/usr/bin/time -l ./target/release/gry index /tmp/repo --name bench
```

`/usr/bin/time -l` supplied peak RSS on macOS. `scripts/benchmark.sh` reproduces the full/unchanged
and query portions.

## Resource and correctness notes

- Delta indexing reuses per-symbol bodies (`body_hash`) and skips all edge work when a body edit
  leaves calls/usages/imports and source ranges unchanged.
- When a symbol name changes, files that reference that name are re-resolved; this is the correct
  blast radius and can exceed the 300 ms target by design.
- Semantic/similarity edges are preserved across deltas and refreshed on a full index; structural
  edges are exact.
- Peak index memory is dominated by parallel extraction plus the in-memory graph. A soft budget can
  be configured with `GRY_MAX_INDEX_MEMORY_MB` / `index_max_memory_mb`; an estimate above the budget
  is rejected before allocation.
- The 100k-file / 500k-node peak-RSS target has **not** been measured; 10k files / 50k nodes
  measured 749 MB.

## Known measurement limits

- macOS `maximum resident set size` includes shared pages; cross-process comparisons are approximate.
- The dense C workload is one repository; Python/TypeScript/docs-heavy repositories have different
  parse/resolution profiles.
- Query timings include process startup; MCP handlers reuse the process.
- Energy was not measured directly; CPU time/throughput is reported instead.
- The machine was near-full on disk during some runs; commit latency varied by tens of milliseconds.

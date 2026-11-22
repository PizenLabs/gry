# GRY Performance Report — 2026-11-22

## Method

- All query measurements were taken through the live MCP server (`gry serve`) from OpenCode using
  wall-clock timestamps around each tool call. Values therefore include JSON-RPC transport, argument
  parsing and response rendering, not only the core query.
- Five iterations were used for most queries; three for architecture and Cypher. The table reports
  min / median / max. Medians are used for comparisons.
- Index measurements used `index_repository` over the real ecommerce repository. Cold measurements
  used a second project name over the same root so the full parser ran without discarding the main
  project. The temporary performance project was deleted after measurement.
- Incremental measurements used a disposable Go fixture outside ecommerce.
- No performance target was invented. Baseline values were recorded first; the same machine, server
  binary family and database were used for before/after.

Environment: macOS arm64, Rust 1.100.0, GRY 1.0.4, SQLite bundled, warm filesystem cache mostly,
OpenCode 2.3.2 MCP client.

## Indexing

| Measurement | Baseline | After fixes |
|---|---|---|
| Full cold index, ecommerce | 894 ms (422 files, 2.07 MB, 5,299 nodes, 24,835 edges) | 1,503 ms (495 files, 2.26 MB, 6,493 nodes, 28,715 edges) |
| Incremental ecommerce re-index (2 parsed, 420 reused) | 1,712 ms wall (1,712 ms reported) | not repeated after the final corpus; unchanged path measured instead |
| Unchanged re-index (full scan, 0 parsed) | 44 ms core / 48 ms wall | 35 ms core / 38 ms wall |
| Incremental one-file fixture | 33–57 ms | 39–53 ms |
| Async one-file fixture | not measured | 53 ms worker (39 ms reported) |
| Files per second, cold | 472 files/s | 329 files/s |
| Nodes per second, cold | 5,927 nodes/s | 4,320 nodes/s |

The cold index is slower in absolute terms because the corrected parser now covers 73 template
files (71 net new discovered files versus the stale baseline). The main project index has 1,191
more nodes and 3,874 more edges, including Go type hierarchies and interface methods; the cold
comparison shows 1,194 nodes / 3,880 edges because semantic derived edges differ slightly between a
delta index and a clean full rebuild. The repository is small enough that fixed passes (Git
history, semantic edges, coverage) dominate; the raw files-per-second numbers are not directly
comparable to the README benchmark workload.

## Query latency (warm cache, MCP round trip)

| Tool | Baseline median | After median | Notes |
|---|---|---|---|
| `search_graph` exact name | 27 ms | 30 ms | Same command and project |
| `search_code` literal, `mode:files` | 16 ms | 18 ms | 495 scanned files vs 424 |
| `trace_path` inbound depth 2 | 8 ms | 8 ms | ProductService.Add |
| `get_code_snippet` small function | 4 ms | 5 ms | Adds SHA-256 freshness check |
| `get_code_snippet` large `auto` | 4 ms (returned no source) | 4 ms (120 lines) | Better payload at same cost |
| `get_file_outline` product.go | 4 ms | 5 ms | Adds path/freshness validation |
| `get_task_context` depth 2 | 20 ms | 12 ms | Same target selection style |
| `get_architecture` small aspects | 49 ms | 59 ms | 22% more nodes and 21 KB output |
| `query_graph` count | 63 ms | 58 ms | Cypher parse identical |

No query regressed materially. Context latency improved; architecture latency increased in line
with the larger graph and the requested output.

## Response size / budgeting

- All tools enforce the deterministic 4-bytes/token ceiling.
- Large interfaces now return 41 of 45 outline members with `has_more` and `next_member_offset`
  instead of failing with “one coherent row”.
- A 634-line `main.main` returns a 120-line window with `next_start_line`.
- A 500-row `search_graph` request returns 49 whole rows with exact totals and continuation; a
  100,000-character search pattern returns the budget fallback with a retry hint.
- Task context now retains truncated node/relationship/file evidence rather than replacing it with
  counts.

## Memory and storage

| Measurement | Baseline | After |
|---|---|---|
| Shared SQLite database size | 254 MB (7 projects) | 274 MB (7 projects after re-index) |
| MCP server RSS after sustained load | not measured | 337 MB |
| Second idle MCP server RSS | — | 8 MB |
| Peak indexing RSS | not measured | not measured in this campaign |

The two MCP server processes belong to the OpenCode harness (one active query server and one idle
process). The 337 MB figure is after many large JSON responses and is not a steady-state index
measurement. A longitudinal memory/DB-growth test was not performed; this is recorded as a
limitation rather than extrapolated.

## Reproducibility

Commands used (all through the real MCP server unless noted):

```text
index_repository repo_path=/Users/anvndev/Documents/Project/Golang/ecommerce mode=full
index_repository repo_path=<fixture> name=gry-audit-fixture mode=full
search_graph project=ecommerce name_pattern=AddVariantToProduct
search_code project=ecommerce pattern=ErrProductNotFound mode=files
trace_path project=ecommerce function_name=ProductService.Add direction=inbound depth=2
get_code_snippet project=ecommerce qualified_name=...ProductService.Add
get_file_outline project=ecommerce file_path=backend/productcatalog/app/product.go
get_task_context project=ecommerce targets=["ProductStorage"] depth=2
get_architecture project=ecommerce aspects=["overview","languages"]
query_graph project=ecommerce query="MATCH (n:Function) RETURN count(n) AS n"
```

Cold-index timings are sensitive to disk cache and machine load; the reported comparison uses the
same machine and the same repository family. The audit does not claim benchmark-grade precision.

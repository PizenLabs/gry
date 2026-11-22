# GRY End-to-End Audit — Final Report

**Date:** 2026-11-22
**GRY version:** 1.0.4 (branch `feature/hooks`, commit `1bc028f`, working tree modified only by this audit)
**Implementation under test:** `/Users/anvndev/Documents/Project/OpenSource/PizenLabs/gry`
**External evaluation target:** `/Users/anvndev/Documents/Project/Golang/ecommerce` (read-only)
**MCP client / agent:** OpenCode 2.3.2, connected to `gry serve` over stdio MCP

## Conclusion

**PASS WITH LIMITATIONS**

Core project selection, indexing, search, navigation, source retrieval, graph traversal, freshness
reporting and agent-facing error behavior are reliable after the fixes. Every P0/P1 issue found in
the campaign was reproduced, corrected and re-tested. The remaining limitations are explicit,
documented, non-destructive, and mostly concern graph completeness for implicit Go interface
dispatch and untested language families. No critical or high-severity defect remains unresolved.

## Executive summary

The audit exercised GRY exclusively through its real MCP interface for the functional campaign,
supplemented by a second real `gry serve` process for restart/interruption experiments. Direct
source inspection was used only to verify claims and root causes; it did not replace MCP testing.

The largest correctness problems were:

1. **Go type declarations were not indexed.** `struct`, `interface` and named type declarations were
   absent, so `ProductService`, `ProductStorage` and hundreds of other central types could not be
   searched, traced or read. Go methods were also labeled `Function`, and parameter names were
   recorded as their types (`NewProductId(id string)` → `["string"]`).
2. **Source retrieval could return wrong source with success.** After a file changed without
   re-indexing, `get_code_snippet` returned the recorded line range from the new file, which could
   be unrelated code. `get_file_outline` returned an empty successful result for missing,
   unindexed and directory paths.
3. **The graph contained false relationships.** 103 routes were linked to the `logger` field instead
   of their handlers; interface/call resolution could bind calls such as `w.Header().Add` to
   unrelated same-module methods; `CALLS` edges could target fields and plain types.
4. **Async status could report a false success after a restart.** A queued/running job from a dead
   process was replaced by a synthetic `succeeded` status from the previous project generation.

These and ten further confirmed issues were fixed inside GRY. Regression tests were added for
every fix. The complete Rust workspace passes:

- baseline: `247 passed; 0 failed`
- after: `264 passed; 0 failed` (17 new regression tests)
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  and `cargo build --release` pass.

The ecommerce repository remains byte-for-byte untouched (`git status --porcelain` is empty and
no `.gry` artifact was created).

## Scope

In scope:

- Project discovery, registration, resolution, persistence and deletion.
- Indexing of the ecommerce Go repository, including nested package structure, migrations,
  documentation, configuration and Go templates.
- Structural/BM25/text search, symbol navigation, source snippets, outlines, graph traversal,
  Cypher, task context, freshness/coverage and change impact.
- MCP tool inventory, schemas, error behavior, output budgeting and agent usability.
- Incremental add/modify/rename/delete on a disposable external fixture.
- Performance of indexing and representative queries.

Out of scope:

- Non-Go language repositories (no second external target of comparable quality was available).
- Runtime behavior of the ecommerce application (no server was started).
- Unrelated pre-existing warnings (unused manifest dependencies) and pre-existing projects in the
  user's cache.

## Environment

| Item | Value |
|---|---|
| OS / arch | macOS, arm64 |
| Rust | 1.100.0 (Homebrew) |
| Cargo | 1.100.0 |
| GRY | 1.0.4, commit `1bc028f`, branch `feature/hooks` |
| GRY database | `~/.cache/gry/graph.db` (shared, 274 MB during the audit) |
| MCP launch | `/Users/anvndev/.local/bin/gry serve` (installed from the audited release build) |
| OpenCode | 2.3.2 (`opencode serve --service`) |
| Ecommerce | branch `master`, clean, 498 tracked files, 305 Go files, one Go module (`backend/go.mod`) |

The MCP server exposes 18 tools: `index_repository`, `search_graph`, `search_code`, `query_graph`,
`trace_path`, `get_code_snippet`, `get_file_outline`, `get_graph_schema`, `compare_graphs`,
`get_architecture`, `list_projects`, `delete_project`, `index_status`, `check_index_coverage`,
`get_task_context`, `detect_changes`, `manage_adr`, `ingest_traces`.

## Baseline

Before changes:

- Full Rust checks passed (`fmt`, `check`, `clippy`, `247` tests, release build).
- Ecommerce was last indexed on 2026-11-18 at 424 files / 5,302 nodes / 24,837 edges, with a stale
  `backend/productcatalog/domain/product.go` (two later commits existed).
- `check_index_coverage` correctly reported the stale file, but the read tools did not use that
  evidence and returned stale line ranges as successful source/outline data.
- `search_graph` could not find `ProductService` or `ProductStorage`; there were no `Struct`,
  `Interface`, `Type` or `Method` nodes.
- `get_file_outline` returned `{"total":0}` for a nonexistent file and for a directory.
- 103 `HANDLES` edges pointed at `layout.http.logger` for routes registered through
  `observability.HTTPWrap(handler, logger)`.
- The initial index covered 422 of 498 tracked files; all 73 `.gohtml`/`.tmpl` templates were
  invisible to discovery and `search_code`.

## Coverage assessment of the ecommerce index

After the fixes and a clean re-index:

| Item | Result |
|---|---|
| Discovered/indexed files | 495 |
| Tracked files | 498 |
| Missing tracked files | 3: `.github/CODEOWNERS`, `LICENSE`, `data/uploads/.gitkeep` (unsupported/blank markers) |
| Go files | 305 / 305 |
| SQL migrations | 77 / 77 |
| Go templates | 73 / 73 (file-only, searchable) |
| Markdown | 19 / 19 |
| YAML/JSON/Compose/Dockerfile/OpenAPI/CSS/Make/Gomod | 20 / 20 |
| File-label nodes | 497 (495 source files + 2 referenced-path placeholders: `.github/workflows/backend`, `data/uploads`) |
| Nodes | 6,493 |
| Edges | 28,711 |
| Parse gaps | 20 line-level gaps, all in SQL migrations; no dropped Go files |
| Languages | go 305, sql 77, gotemplate 73, markdown 19, yaml 10, css 2, gomod 2, json 2, unknown 2 (placeholders), compose 1, dockerfile 1, gitignore 1, make 1, openapi 1 |

The two placeholder `File` nodes are deliberate referenced paths (a CI working directory and a
Compose volume), not discovered files; this explains the difference between `files` (495) and the
`File` label count (497). The three unindexed tracked files are intentionally outside the language
table; no source file was silently dropped.

## Verification of fixed issues

All rows were re-executed through OpenCode MCP after installing the rebuilt server and reconnecting
OpenCode (configuration reload). Detailed test rows are in `TEST_RESULTS.md`.

| Issue | Before | After |
|---|---|---|
| Go `Struct`/`Interface`/`Type` nodes | 0 / 0 / 0 | 373 / 96 / 24 |
| Go `Method` nodes | 0 | 1,821 |
| `ProductStorage` exact lookup | 0 results | 3 results, interface first |
| `ProductService` symbol | not found | struct + exact snippet |
| Go `NewProductId` params | `["string"]` | `["id"]` |
| Stale file snippet | wrong lines returned as success | explicit “changed since indexed; re-index” error |
| Missing-file outline | empty success | explicit not-found / not-indexed error |
| Same-size + preserved-mtime edit | reported fresh (stat-only) | reported stale via SHA-256 |
| Route `HANDLES` → logger | 103 | 0 |
| `/auth/login` handlers | `logger` field | `httpHandler.Login` + `httpHandler.HandleLogin` |
| `catalogService.Add` inbound callers | 29, mostly false | 1: `AdminCreateProduct` (confidence 1.00) |
| `CALLS` targets that are not callables | 412 edges to Field/Type/Struct/Interface | 0; only Function/Method |
| Async status after restart while job was running | synthetic `succeeded` from old generation | persisted `running` + `stale_status` + re-run note |
| Explicit project root/descendant path | rejected | resolved deterministically |
| Explicit context target `NopeSymbolXyz` | resolved to `Money.Display` | `not_found` |
| Large function auto snippet | no source, no continuation | 120 lines + `next_start_line: 178` |
| Large interface outline (`format:"json"`) | budget error / no content | 41 of 45 members + continuation |
| Empty global Cypher aggregate | zero rows | one row with `0` |
| Template files | 0 indexed | 73 indexed and searchable |

## Performance summary

Measured on the same machine and index; full details in `PERFORMANCE.md`.

| Metric | Baseline / before | After |
|---|---|---|
| Full cold index, ecommerce-like corpus | 894 ms / 422 files / 5,299 nodes | 1,503 ms / 495 files / 6,493 nodes |
| Unchanged re-index | 44–48 ms | 35–38 ms |
| Incremental one-file fixture index | 33–57 ms | 39–53 ms (async worker 53 ms) |
| `search_graph` exact (warm) | 27 ms | 30 ms |
| `search_code` (warm) | 16 ms | 18 ms |
| `trace_path` depth 2 (warm) | 8 ms | 8 ms |
| `get_code_snippet` (warm) | 4 ms | 5 ms |
| `get_task_context` depth 2 (warm) | 20 ms | 12 ms |
| `get_architecture` small aspect set | 49 ms | 59 ms |
| `query_graph` count | 63 ms | 58 ms |
| DB size (shared, 7 projects) | 254 MB | 274 MB |
| MCP server RSS | not measured before | 337 MB after sustained query load; 8 MB idle process |

The full index is slower in absolute terms because it now covers 73 additional template files
(71 net new discovered files versus the stale baseline), 1,191 more nodes and 3,874 more edges
(including the type hierarchy and `DEFINES_METHOD`). Per-file throughput for this small corpus is
dominated by fixed passes, so it is not compared to the synthetic benchmark target; warm-query
latency did not regress materially.

## Remaining limitations

1. **Go implicit interface implementation is not synthesized.** Interface method sets and concrete
   method sets are indexed, and calls resolve to interface methods, but GRY does not emit
   `IMPLEMENTS` edges from structural matching. An agent can inspect consumers of the interface and
   the concrete type's methods, but cannot ask for “all implementations” directly. Documented in
   `CAPABILITIES.md` (LANG-07) and `ARCHITECTURE.md` §5.
2. **Conservative Go method dispatch.** Calls through locals whose type cannot be inferred, embedded
   interfaces, reflection and dynamic dispatch remain unresolved. This avoids false edges at the
   cost of missing some real ones.
3. **Async jobs are process-local.** The worker thread dies with the MCP process. The persisted
   status is now reported honestly as `running` with `stale_status`, but the user must re-run the
   index. There is no detached daemon by design.
4. **Template files are file-only.** `.gohtml`/`.tmpl` files are discovered, searchable and counted,
   but not parsed into symbols.
5. **Environment-variable extraction is partial.** Go struct tags used by `ardanlabs/conf`
   (`SESSION_SECRET`) are not emitted as `EnvVar` nodes; `search_code` finds them. The `EnvVar`
   label also contains CSS custom properties.
6. **Two referenced-path `File` placeholders** are included in the `File` label count for ecommerce.
   They are labelled as placeholders in node properties and are not discovered source files.
7. **`compare_graphs` identities are project-scoped.** Re-indexing the same repository under a
   different project name reports every node as added/removed. The tool now documents this.
8. **`search_code` schema rendering in OpenCode.** The valid `anyOf` schema renders as
   `search_code(unknown)` in OpenCode’s signature display; the tool itself accepts the documented
   `pattern`/`query` alias and returns correct results. This is a client rendering limitation.
9. **Broad task-only context is noisy.** `get_task_context` without explicit targets is suitable for
   focused multi-part tasks; broad architecture questions are better served by `get_architecture`.
   The scout profile was observed selecting weak BM25 targets for a generic architecture question.
10. **Single external repository.** Only the ecommerce Go repository was evaluated end-to-end;
    non-Go language families and very large monorepos were not independently validated in this run.
11. **`search_code` symbol attribution can be stale for a changed file.** Match text and line come
    from the current file, but the attached enclosing symbol comes from indexed line ranges. Exact
    source/outline reads reject stale files, so this does not affect source retrieval.

## Final quality assessment

GRY is usable by an external AI agent for real repository work. It now returns accurate declarations
and source for Go, distinguishes uncertainty and staleness instead of fabricating success, exposes
interface contracts and call chains, and gives actionable errors. The remaining gaps are
completeness limits in a conservative static graph, not silent correctness failures.

The audit therefore concludes **PASS WITH LIMITATIONS**, with the limitations above explicitly
documented and no unresolved P0/P1 defect.

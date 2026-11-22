# GRY Defect Register — 2026-11-22

Only issues reproduced through the live MCP server or a real `gry serve` process are listed as
confirmed. Severity follows the audit rubric: P0 critical, P1 high, P2 medium, P3 low.

Status values: **FIXED — VERIFIED** (original reproduction passes after the correction),
**OPEN — DOCUMENTED LIMITATION** (not corrected in this audit).

## Fixed defects

### D01 — Go struct, interface and named-type declarations were not indexed

- **Severity:** P1 (incorrect indexing; missing important symbols)
- **Component:** `gry-parse` extraction
- **Reproduction:** `search_graph project=ecommerce name_pattern="^ProductStorage$"` returned 0;
  `ProductService` returned 0; `get_code_snippet` for either returned “symbol not found”. The
  `index_status` label inventory contained no `Struct`, `Interface` or `Type` labels.
- **Root cause:** Go's tree-sitter grammar represents `type X struct{...}` as
  `type_declaration → type_spec → struct_type`. The classifier handled `type_declaration` (label
  `Type`) but the name lives on the child `type_spec`; `definition_name` found no name and the
  definition was dropped. `struct_type` and `interface_type` were not classified, so the body was
  never walked with a type container.
- **Fix:** classify Go `type_spec`/`type_alias`, choose `Struct`/`Interface`/`Type` from the child
  type node, and classify interface `method_elem` as `Method`. See `crates/gry-parse/src/extract.rs`.
- **Verification:** post-fix index contains 373 `Struct`, 96 `Interface`, 24 `Type`; exact
  `ProductStorage` lookup returns the interface first; `ProductService` source is retrievable.
- **Regression tests:** `go_type_declarations_keep_names_and_scopes`, fixture outline/snippet tests.

### D02 — Go methods were labeled `Function`

- **Severity:** P2 (data quality / filtering)
- **Component:** `gry-parse` extraction
- **Reproduction:** `search_graph label="Method"` returned 0 in a repository with hundreds of Go
  methods; `AddVariantToProduct` only appeared under `label="Function"`.
- **Root cause:** the generic classifier only labeled a function as `Method` when its container was
  a class/struct/interface/trait. Go methods are top-level `method_declaration` nodes.
- **Fix:** label Go `method_declaration` as `Method`.
- **Verification:** 1,821 `Method` nodes in ecommerce; route handlers resolve as methods.
- **Regression test:** covered by type-declaration and route tests.

### D03 — Go parameter names were recorded as their types

- **Severity:** P2 (data integrity; breaks parameter-scoped data flow)
- **Component:** `gry-parse` extraction
- **Reproduction:** `NewProductId(id string)` produced `params:["string"]`; a function ending in
  `currency string` produced `["…","string"]` instead of the parameter name.
- **Root cause:** the generic extractor takes the last whitespace-separated word as the name, which
  is correct for `Type name` syntax but wrong for Go's `name Type`.
- **Fix:** language-aware `go_param_names` for `parameter_declaration` and
  `variadic_parameter_declaration`; grouped names (`a, b int`) and unnamed parameters handled.
- **Verification:** `NewProductId` → `["id"]`; fixture `Sum` → `["a","b"]`.
- **Regression test:** `go_type_declarations_keep_names_and_scopes`.

### D04 — `get_code_snippet` returned wrong source with a success response after a file changed

- **Severity:** P0 (fundamentally incorrect source + silent false success)
- **Component:** `gry-query` snippet path
- **Reproduction:** on the fixture, prepend three lines to `util.go` without re-indexing. The recorded
  `Add` range was lines 3–5. `get_code_snippet` returned those lines from the new file:
  a blank line, `package main`, and a blank line — with `source_mode:"full"` and no error.
- **Root cause:** the query read the current file and sliced the line range stored in the index,
  without validating that the indexed content still matched the file.
- **Fix:** compute SHA-256 of the file bytes and compare against `file_hashes`; if the hash is
  absent or different, return an actionable error (“changed since it was indexed … re-index”).
- **Verification:** reproduction now returns the explicit error; after `index_repository` the
  snippet has the correct range (`Hello` 20–22, `Add` 6–9).
- **Regression tests:** `snippet_and_outline_reject_a_changed_file`.

### D05 — `get_file_outline` returned an empty success for missing/unindexed/directory paths

- **Severity:** P1 (silent false success)
- **Component:** `gry-query` outline path
- **Reproduction:** `get_file_outline file_path="backend/nope/file.go"`, `file_path="backend"` and an
  ignored file all returned `{"total":0,"rows":[]}` with no error.
- **Root cause:** the outline path only queried indexed nodes and never verified that the requested
  path was an indexed file on disk.
- **Fix:** validate absolute/traversal paths; require an indexed hash and an on-disk file; verify
  SHA-256 freshness; distinguish missing, unindexed and directory paths with specific errors.
- **Verification:** missing path → “file not found in the index or on disk”; directory → “file_path is
  a directory”; stale file → “changed since it was indexed”; template file → valid empty outline
  (indexed, no declarations).
- **Regression tests:** `outline_rejects_paths_that_are_not_indexed`,
  `snippet_and_outline_reject_a_changed_file`.

### D06 — Wrapped route handlers produced false `HANDLES` edges

- **Severity:** P1 (serious false graph relationships)
- **Component:** `gry-parse` route extraction, `gry-index` handler resolution
- **Reproduction:** `layout/http.go` registers
  `r.HandleFunc("/auth/login", observability.HTTPWrap(m.handler.Login, m.logger))`. The graph
  attributed `/auth/login` to `layout.http.logger`, and 103 routes had a
  `logger -[:HANDLES]-> Route` edge.
- **Root cause:** the extractor passed the whole wrapper expression to `extract_callable_name`,
  which returns the last identifier (`m.logger`). The indexer then resolved any same-file symbol
  with that name, including fields.
- **Fix:** parse the handler expression recursively, prefer the first exported method reference
  inside a wrapper, fall back to the wrapper/factory callable, infer the handler receiver type, and
  restrict route targets to `Function`/`Method`.
- **Verification:** `logger` HANDLES count is 0; `/auth/login` maps to
  `httpHandler.Login` and `httpHandler.HandleLogin`; `AdminCreateProduct` maps to
  `catalogService.Add`/`SetProductAttributeSet`.
- **Regression tests:** `go_wrapped_route_handler_uses_the_handler_not_the_middleware_argument`.

### D07 — Calls bound to unrelated same-module methods and non-callable nodes

- **Severity:** P1 (false relationships; graph trust)
- **Component:** `gry-index` resolver
- **Reproduction:** after interface methods were indexed,
  `catalogService.Add` had 29 inbound `CALLS` edges, including `AddToCart` via
  `w.Header().Add` and `AccountAddAddress` via a shipping-service call. `buffer.Header` absorbed
  `w.Header()`. A snapshot showed 412 `CALLS` edges targeting `Field`/`Type`/`Struct`/`Interface`.
- **Root cause:** (a) same-module + name/suffix/unique-name scoring could exceed the threshold
  without receiver evidence; (b) CALLS candidates were not restricted to callables; (c) field types
  declared in a different file were unknown to the per-file type map.
- **Fix:** skip non-callable CALLS targets in Go (except explicit constructors); skip method
  candidates whose known receiver type does not match; heavily penalize untyped-receiver method
  candidates; build a project-wide `(type, field) → declared type` map and enrich call receiver
  types before resolution. A dotted receiver such as `e.store.Add` also carries the base
  variable's declared type (`English`), so the indexer can walk `(English, store) → Storage`
  even when the field is declared in another file. `EXTRACTOR_VERSION` was bumped so existing
  graphs are rebuilt with these semantics.
- **Verification:** `catalogService.Add` inbound is exactly `AdminCreateProduct` (confidence 1.00);
  `AccountAddAddress` resolves to `shippingService.Add`; `AddToCart` no longer has `Add`/`Header`
  edges; CALLS targets are only `Function`/`Method`; a call through a parameter's cross-file field
  (`Use(e English) → e.store.Add`) resolves to the interface method.
- **Regression tests:** `go_calls_do_not_resolve_to_fields_or_plain_types`,
  `go_untyped_receiver_does_not_bind_same_module_methods`,
  `go_response_header_chain_is_not_a_local_method_call`,
  `go_receiver_base_type_outlives_a_cross_file_field`,
  `go_call_through_a_parameter_field_resolves_across_files`.

### D08 — Async status could report `succeeded` for a job that was still running or lost

- **Severity:** P1 (stale information presented as completed)
- **Component:** `gry-mcp` `index_repository status:true`
- **Reproduction:** start an async index, terminate the MCP process (worker is process-local), start a
  new server and call `status:true`. The tool reported `succeeded` with the generation and counts
  of the *previous* index, while the new job’s persisted status was `running`.
- **Root cause:** the status handler only consulted the in-process `JobManager`; when absent it
  synthesized a success from the project record, ignoring the persisted job file.
- **Fix:** read the persisted job status first; if it is `queued`/`running`/`cancelling`, return it
  with `stale_status:true` and an explicit note to re-run if the owning process is gone. Only fall
  back to the project record when no job status exists.
- **Verification:** contract test `async_status_after_restart_is_not_reported_as_succeeded`;
  live async job succeeded in 53 ms when the MCP server stayed alive.
- **Regression test:** included in `crates/gry-mcp/tests/contract.rs`.

### D09 — Explicit project root/descendant paths were rejected

- **Severity:** P2 (usability; needless round trips)
- **Component:** `gry-mcp` project resolution
- **Reproduction:** `search_graph project="/Users/.../ecommerce"` returned “project not found or not
  indexed” even though the project existed at that exact root.
- **Root cause:** an explicit `project` value was only looked up as a project name.
- **Fix:** try name, exact root, canonical root, then unique deepest containing project; reject
  ambiguous path matches with the candidate names.
- **Verification:** root and `.../backend` both resolve to `ecommerce`; typo/path errors remain
  actionable.
- **Regression test:** `all_eighteen_tools_succeed` now checks root and descendant values.

### D10 — Routes outranked declarations in literal name search

- **Severity:** P3 (search relevance)
- **Component:** `gry-store` ordering
- **Reproduction:** `search_graph name_pattern="Product"` returned 24 route nodes before the
  `Product` struct/function declarations.
- **Root cause:** `Route` had the same label rank as callables, and ties were sorted by name, where
  `/...` sorts before uppercase identifiers.
- **Fix:** rank routes below declarations in structural and BM25 ordering.
- **Verification:** exact `Product` search returns declarations (structs/methods/fields); routes are
  still returned for path-like queries.
- **Regression tests:** `route_nodes_rank_below_declarations_in_name_search`; manual MCP re-check in
  `TEST_RESULTS.md`.

### D11 — Explicit context targets silently resolved to unrelated symbols

- **Severity:** P1 (fabricated evidence / uncertainty handling)
- **Component:** `gry-query` target resolution
- **Reproduction:** `get_task_context targets=["NopeSymbolXyz"]` resolved to
  `sharedkernel.Money.Display` with `status:"resolved"` and no warning. The single shared token
  “symbol” came from that node’s docstring.
- **Root cause:** the BM25 fallback accepted any top hit, even for an explicit symbol target.
- **Fix:** for explicit targets (not task-derived), require every identifier token of the input to
  appear in the node’s name/QN/file/docstring; otherwise return `unresolved`/`not_found`. Task-only
  search retains broad relevance behavior.
- **Verification:** `NopeSymbolXyz` → `status:"not_found"`, target `unresolved`,
  unknown `unresolved_target`; valid targets such as `DependencyManager` still resolve.
- **Regression test:** `explicit_target_rejects_a_partial_token_match`.

### D12 — Bounded task context dropped all evidence nodes at the default output budget

- **Severity:** P2 (context usefulness)
- **Component:** `gry-mcp` renderer
- **Reproduction:** `get_task_context targets=["DependencyManager"] depth=2` returned target metadata
  but `nodes:{"count":12,"omitted":true}` and no files.
- **Root cause:** the shrink path did not include `nodes`/`relationships`/`files`, so the generic
  fallback collapsed the largest arrays to counts.
- **Fix:** shrink those arrays (and `targets`) before collapsing; keep page metadata.
- **Verification:** the same call returns a truncated array of caller/handled-route/callee nodes and
  files.
- **Regression test:** `context_nodes_shrink_instead_of_collapsing_to_a_count`.

### D13 — Large `auto` snippets returned no source and no continuation

- **Severity:** P2 (inspection usability)
- **Component:** `gry-query` snippet path
- **Reproduction:** `get_code_snippet main.main` (634 lines) returned `source_omitted:true`, empty
  `source`, and no guidance at the default budget.
- **Root cause:** `auto` bounded only large containers; a large function produced a single oversized
  row that the MCP budget dropped.
- **Fix:** cap `auto` source at 120 lines and add `returned_lines`, `truncated` and
  `next_start_line`; explicit `full`/`max_lines` still control the window.
- **Verification:** `main.main` returns 120 lines with `next_start_line:178`.
- **Regression test:** `snippet_auto_bounds_a_large_function_with_continuation`; MCP re-check in
  `TEST_RESULTS.md` E10.

### D14 — Large interface outlines failed at the default JSON budget

- **Severity:** P2 (inspection usability)
- **Component:** `gry-mcp` renderer
- **Reproduction:** `get_code_snippet ProductStorage source_mode=outline format=json` returned
  “max_output_tokens too small for one coherent row”.
- **Root cause:** nested `outline.members` was not part of the shrink paths.
- **Fix:** shrink `outline.members`, add `next_member_offset`, and verify nested paging.
- **Verification:** 41 of 45 members returned with `has_more:true` and `next_member_offset:41`.
- **Regression test:** `nested_outline_members_are_truncated_instead_of_dropped`.

### D15 — Global Cypher aggregate over an empty match returned zero rows

- **Severity:** P3 (query semantics)
- **Component:** `gry-query` Cypher
- **Reproduction:** `MATCH (n:Zzz) RETURN count(n) AS n` returned `rows:[]` instead of `[[0]]`.
- **Root cause:** grouping produced no groups when the input row set was empty, even with no
  grouping key.
- **Fix:** emit one aggregate row when every projection is an aggregate and the input is empty;
  grouped aggregates still return no rows.
- **Verification:** unknown-label count returns `[[0]]` with `total:1`; known-label count unchanged.
- **Regression test:** extended `aggregates_and_exists`.

### D16 — Coverage freshness used size/mtime only

- **Severity:** P2 (stale information presented as fresh)
- **Component:** `gry-query` coverage
- **Reproduction:** same-size edit with the original mtime restored; `check_index_coverage` reported
  `indexed/fresh` although the content changed.
- **Root cause:** only stat comparison was performed for requested paths.
- **Fix:** hash requested paths (bounded by the caller’s path list) and mark mismatches stale.
- **Verification:** the reproduction now reports `stale` / `file_changed_since_index`.
- **Regression test:** `coverage_hashes_content_even_when_stat_matches`.

### D17 — Go templates were invisible to discovery and text search

- **Severity:** P2 (coverage)
- **Component:** `gry-core` language table
- **Reproduction:** 73 `.gohtml`/`.tmpl` files were tracked but absent from the index and
  `search_code`; only 422 of 498 tracked files were discovered.
- **Root cause:** neither extension was recognized by the language table, so discovery skipped them.
- **Fix:** add a `gotemplate` file-only language (`.gohtml`, `.gotmpl`, `.tmpl`); no symbol parser is
  claimed.
- **Verification:** 73 template files indexed, 495/498 tracked files covered, `search_code "{{define"`
  returns 55 files / 98 matches.
- **Regression test:** language detection assertions.

### D18 — Full graph-schema properties were unreachable at the default budget

- **Severity:** P3 (tool authoring usability)
- **Component:** `gry-mcp` schema tool
- **Reproduction:** `get_graph_schema diagnostics=full format=json` omitted `properties` and had no
  way to request a larger budget.
- **Root cause:** the tool did not accept `max_output_tokens` and returned a fixed 3,200-token budget.
- **Fix:** add `max_output_tokens` to the schema and pass it to the renderer.
- **Verification:** with 50,000 tokens, `properties` is present, including `edge:CALLS` and
  `Function` property types.
- **Regression tests:** tools schema consistency suite; `all_eighteen_tools_succeed` now asserts that
  `properties` is present when a larger budget is requested.

## Open limitations (documented, not fixed)

| ID | Severity | Limitation | Evidence / impact |
|---|---|---|---|
| L01 | P2 | Go implicit interface implementation is not synthesized into `IMPLEMENTS` edges | Interface/concrete method sets are indexed; “which types implement X” needs method-set inspection. Documented in CAPABILITIES (LANG-07) and ARCHITECTURE §5 |
| L02 | P2 | Conservative Go dispatch through untyped locals, embedding and reflection | Calls can remain unresolved; no false edge is fabricated. Seen in `marshalEvent` lineDTO field accesses |
| L03 | P2 | Async jobs are process-local | Worker thread dies with the server; persisted status is now honest but the job must be re-run |
| L04 | P3 | Go templates are file-only | No symbol extraction from `.gohtml`/`.tmpl` |
| L05 | P3 | `EnvVar` extraction is partial | Go `conf` struct tags (`SESSION_SECRET`) absent from graph; `search_code` finds them; CSS custom properties share the label |
| L06 | P3 | Referenced-path `File` placeholders are counted in the `File` label | Two ecommerce placeholders explain files=495 vs File label=497; properties mark `placeholder:true` |
| L07 | P3 | `compare_graphs` identities are project-scoped | Re-indexing the same repo under a different name appears as all-added/all-removed; tool description now states this |
| L08 | P3 | `search_code` schema renders as `unknown` in OpenCode | Valid `anyOf` schema; runtime accepts `pattern`/`query`; client display limitation |
| L09 | P3 | Broad task-only context is noisy | Use `get_architecture` for system shape; `get_task_context` is target-centric |
| L10 | P3 | Single external repository; non-Go languages not independently validated | Scope limitation of this audit |
| L11 | P3 | `search_code` symbol attribution can be stale for a changed file | Match text and line come from the current file, but the attached enclosing symbol comes from indexed line ranges; re-index after edits. Source/outline reads reject stale files, so this does not affect exact source retrieval |

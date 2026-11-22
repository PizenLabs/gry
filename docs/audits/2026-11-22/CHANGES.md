# GRY Changes — 2026-11-22 Audit

No commit, push, tag, publish or release was performed. Changes are limited to the GRY repository.
The ecommerce repository was not modified.

## Implementation changes

| File | Change | Why |
|---|---|---|
| `crates/gry-parse/src/model.rs` | Added `Def.type_name` and `CallSite.receiver_base_type` (serde default) | Preserve a field's declared type and the base variable's type for cross-file receiver inference |
| `crates/gry-parse/src/extract.rs` | Classify Go `type_spec`/`type_alias` as Struct/Interface/Type; classify interface `method_elem` as Method; label Go `method_declaration` as Method; add language-aware Go parameter extraction; capture declared field types (data declarations only); record a dotted receiver's base type; parse wrapped route handler expressions; skip generic base extraction on Go type specs; regression tests | Fix missing Go type/method symbols, wrong parameter names, polluted container properties, and false route-handler edges |
| `crates/gry-index/src/build.rs` | Store `declared_type` node property; build project-wide `(type, field) → type` map; walk dotted receivers from a known base type; restrict route handler resolution to callables | Resolve cross-file Go interface calls correctly, including calls through a parameter's field in another file; prevent `HANDLES` edges to logger/other fields |
| `crates/gry-index/src/pipeline.rs` | Bump `EXTRACTOR_VERSION` 36 → 37 | Existing indexes rebuild once so the new extraction and resolution semantics actually apply |
| `crates/gry-index/src/resolve.rs` | Reject non-callable CALLS targets in Go; skip methods with a known conflicting receiver type; penalize untyped-receiver methods; regression tests | Remove false `CALLS` edges to fields/types and unrelated same-module methods |
| `crates/gry-query/src/search.rs` | Validate snippet/outline source against the indexed SHA-256; reject missing/unindexed/directory/traversal paths; bound `auto` snippets at 120 lines with continuation metadata; add regression tests | Stop silent wrong-source success and unusable empty outlines |
| `crates/gry-query/src/coverage.rs` | Hash explicitly requested paths even when size/mtime match; test | Detect same-size/preserved-mtime edits as stale |
| `crates/gry-query/src/context.rs` | Require all identifier tokens to match for explicit BM25 target fallback; test | Stop silently resolving a nonexistent target to an unrelated symbol |
| `crates/gry-query/src/cypher.rs` | Emit one aggregate row for a global aggregate over empty input; test | Correct `count()` semantics |
| `crates/gry-mcp/src/handlers.rs` | Accept indexed root/descendant paths as explicit project; read persisted async job status and flag queued/running as stale; pass `max_output_tokens` for schema/compare tools | Usability and stale-status correctness |
| `crates/gry-mcp/src/render.rs` | Truncate nested outline/compare/context arrays instead of dropping them; add `next_member_offset`; tests | Keep usable evidence under the output budget |
| `crates/gry-mcp/src/tools.rs` | Added `max_output_tokens` to graph-schema/compare schemas; clarified source/outline/compare descriptions | Make budgets and failure behavior explicit to agents |
| `crates/gry-mcp/tests/contract.rs` | Tests for explicit root/descendant project resolution and persisted async status after restart | Regression coverage at the MCP boundary |
| `crates/gry-store/src/lib.rs` | Rank routes below declarations in filter-only search | Prevent route noise from outranking symbols |
| `crates/gry-store/src/search.rs` | Same route-rank adjustment in BM25 ordering | Consistency |
| `crates/gry-core/src/language.rs` | Add file-only `gotemplate` language for `.gohtml`, `.gotmpl`, `.tmpl`; detection tests | Cover 73 ecommerce template files without claiming symbol extraction |

## Documentation changes

| File | Change |
|---|---|
| `docs/CAPABILITIES.md` | Record Go templates as file-only inputs; document that Go implicit interface satisfaction is not synthesized into `IMPLEMENTS` |
| `docs/ARCHITECTURE.md` | Document conservative Go interface resolution and the missing implicit-implementation edges |
| `docs/audits/2026-11-22/FINAL_REPORT.md` | Authoritative audit conclusion |
| `docs/audits/2026-11-22/TEST_RESULTS.md` | Test matrix |
| `docs/audits/2026-11-22/DEFECTS.md` | Defect register and open limitations |
| `docs/audits/2026-11-22/PERFORMANCE.md` | Baseline/post-fix measurements |
| `docs/audits/2026-11-22/AGENT_EVALUATION.md` | OpenCode/MCP agent-usability evaluation |
| `docs/audits/2026-11-22/CHANGES.md` | This file |

## Tests added

- `go_type_declarations_keep_names_and_scopes`
- `go_receiver_base_type_outlives_a_cross_file_field`
- `go_call_through_a_parameter_field_resolves_across_files`
- `go_wrapped_route_handler_uses_the_handler_not_the_middleware_argument`
- `go_response_header_chain_is_not_a_local_method_call`
- `go_calls_do_not_resolve_to_fields_or_plain_types`
- `go_untyped_receiver_does_not_bind_same_module_methods`
- `coverage_hashes_content_even_when_stat_matches`
- `explicit_target_rejects_a_partial_token_match`
- `outline_rejects_paths_that_are_not_indexed`
- `snippet_and_outline_reject_a_changed_file`
- `snippet_auto_bounds_a_large_function_with_continuation`
- `nested_outline_members_are_truncated_instead_of_dropped`
- `nested_compare_rows_are_truncated_instead_of_dropped`
- `context_nodes_shrink_instead_of_collapsing_to_a_count`
- `route_nodes_rank_below_declarations_in_name_search`
- `async_status_after_restart_is_not_reported_as_succeeded`
- plus assertions added to `outlines_filter_and_page`, `aggregates_and_exists`, language detection,
  and `all_eighteen_tools_succeed`.

## Validation

- `cargo fmt --all -- --check`: pass
- `cargo check --workspace`: pass (pre-existing unused manifest-dependency warnings only)
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: pass
- `cargo test --workspace`: 264 passed, 0 failed (baseline 247)
- `cargo build --release`: pass
- Audited binary installed at `/Users/anvndev/.local/bin/gry`; OpenCode MCP reloaded and verified
- Ecommerce: `git status --porcelain` empty; no `.gry` artifact

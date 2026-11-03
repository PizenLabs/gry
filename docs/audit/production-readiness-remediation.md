# GRY production-readiness remediation — 2026-11-03

This report records every implemented correction, its root cause, changed components, regression
tests and verification evidence. It complements `production-readiness-audit.md`; finding IDs match.

## 1. Summary

| Finding | Change | Regression test |
|---|---|---|
| F1 | Coverage distinguishes `unavailable`, `missing_on_disk`, `stale`, `indexed`, `missing`; `fresh` is only true when the file exists and matches the index; scopes become `unavailable` when the root is gone. | `coverage::tests::coverage_reports_missing_and_indexed` (extended) |
| F2 | `search_code` errors when the root is unavailable, counts unreadable files, exposes `complete`; `get_code_snippet` errors on unavailable source/out-of-range and keeps outline mode graph-only. | `source_search_reports_an_unavailable_root_instead_of_empty_results`, `snippet_fails_when_source_is_unavailable` |
| F3 | Discovery always hashes the bytes it already reads; no mtime/size SHA reuse. | `same_size_edit_with_preserved_mtime_still_changes_the_hash`, `same_size_edit_with_preserved_mtime_is_not_treated_as_unchanged` |
| F4 | Extractor marker is per project (`extractor_version:<project>`); a missing marker forces a rebuild; the global marker is kept for diagnostics. | `a_stale_per_project_extractor_forces_a_rebuild` |
| F5 | Project resolution validates explicit names, ignores deleted remembered projects, resolves descendant directories deterministically, and names indexed projects in the error. CLI and MCP share `Store::projects_containing_path`. | `projects_containing_path_returns_deepest_first`, `unknown_project_errors_name_the_indexed_projects`, live MCP run |
| F6 | `search_code` accepts `query` as a documented alias for `pattern`; schema uses `anyOf`; handler gives an actionable error if neither is present. | `search_code_accepts_the_documented_query_alias`, `search_code_documents_the_query_alias` |
| F7 | `search_graph` rejects `query` combined with structural-only filters instead of ignoring them. | `query_rejects_structural_filters_instead_of_ignoring_them` |
| F8 | `match_count` is the true per-file count; `returned_matches` is the page size. | `source_search_reports_the_true_match_count_per_file` |
| F9 | Lock file is never unlinked; only the advisory lock is released. | Reasoning recorded; no deterministic test |
| F10 | Removed the redundant pre-publication orphan scan; delta publication already reconciles deletes atomically. | `newly_ignored_files_are_removed_by_delta_publication`, `graph_invariants_hold_after_incremental_updates`; benchmark |
| F11/F12 | Destructive/write tools require an explicit project in schema and handler. | Existing contract suite still passes |
| F13 | Ambiguous snippet names return candidates. | `snippet_reports_ambiguity_instead_of_not_found` |
| F14 | Removed dead computations and unused parameters. | Compiler/clippy |

## 2. Detailed changes

### 2.1 Source availability is evidence, not emptiness

**Problem.** The index stores paths and hashes, so graph queries remain valid after a repository
moves. Source-reading operations did not distinguish “the file contains no matches” from “the file
could not be read.” `search_code` swallowed every `read_to_string` error with `continue` and returned
`total_files: 0`; `get_code_snippet` used `unwrap_or_default()` and returned `source: ""`.

**Correction.**

- `gry-query/src/search.rs`: `search_code` checks `project.root_path.is_dir()` and errors with the
  project name, root, and recovery instruction. It tracks `scanned_files`, `unreadable_files`,
  `unreadable_samples` and `complete`. If every scanned file is unreadable it errors rather than
  claiming an empty result.
- `get_code_snippet`: determines `auto`/`outline` mode before touching the filesystem. Outline mode
  is served from the graph. Full mode reads the file with an explicit error, rejects an empty source
  location for global nodes, and rejects a recorded range outside the current file (stale index)
  rather than returning a short/empty snippet.
- `gry-mcp/src/handlers.rs`: `search_code` accepts `pattern` or `query`.

**Evidence.** Live before/after against the stale `gry` project:

```text
before: search_code -> {"total_files":0, ... "scope":"indexed_files"}
before: get_code_snippet -> {"source":"", ...}
after:  search_code -> invalid argument: project root is unavailable: ... (project gry)
after:  get_code_snippet -> invalid argument: source unavailable for ...: No such file or directory
```

### 2.2 Coverage can no longer call a missing file fresh

`gry-query/src/coverage.rs` now computes freshness only when both a hash and filesystem metadata
exist. Status precedence:

```text
parse_partial / parse_unusable
unavailable         (root missing)
missing_on_disk     (recorded hash, no filesystem entry)
stale               (mtime/size differ)
indexed
skipped
missing
```

`index_status` exposes `root_available`, `source_available`, `availability`, and a warning when the
root is missing. `check_index_coverage` exposes `root_available` and marks scopes `unavailable`
instead of `clean`.

**Test.** The regression test creates a real temporary root, indexes it, deletes the file, then
deletes the root, asserting `indexed/fresh` → `missing_on_disk/not fresh` → `unavailable/not fresh`.
The old test used a fabricated `/tmp/p` root and accidentally encoded the defect.

### 2.3 Incremental correctness: content hashes are exact

Discovery already read every candidate file for language disambiguation. It then reused the prior
SHA when `mtime_ns` and size matched. The shortcut could miss a same-size edit within the filesystem
timestamp resolution (or an edit that preserved mtime). The shortcut is removed: the hash is always
computed from the bytes in memory.

The end-to-end regression test edits a file to the same byte length, restores the original mtime,
and asserts `unchanged=false`, `parsed_files=1`, and a new generation.

### 2.4 Per-project extractor semantics

`store_meta.extractor_version` was global. A mixed-version database could take a delta for project B
after project A was rebuilt. The marker is now `extractor_version:<project>`; the global key remains
as a diagnostic mirror. A missing per-project marker forces a full rebuild, which is conservative
and requires no schema migration.

**Compatibility.** Existing databases have no per-project marker, so the first index after upgrade
performs one clean rebuild per project. No data is deleted before a successful publication.

### 2.5 Project resolution and recovery

`Ctx::project` now:

1. validates an explicit project name and reports available projects if absent;
2. reuses `last_project` only while the project still exists;
3. checks MCP roots (exact and canonical);
4. checks the exact workspace root and any indexed ancestor, deepest first;
5. refuses an ambiguous tie instead of choosing arbitrarily;
6. falls back to the single-project case;
7. otherwise reports the workspace, up to 10 indexed projects with root availability, and both
   recovery actions (`project=<name>` or `index_repository`).

The CLI hook/status path uses the same `Store::projects_containing_path` implementation, so CLI and
MCP no longer have divergent ancestor-resolution rules.

**Live evidence.** With the five-project local cache and a moved `gry` index:

```text
before: invalid argument: project is required; call list_projects to see indexed projects
after:  invalid argument: project is required and this workspace does not match an indexed project.
        Current workspace: /Users/.../PizenLabs/gry. Indexed projects: bench (..., root missing);
        ... gry (.../RnD/gry, root missing); ... Pass project=<name>, or index this workspace ...
```

This converts a failed task into a one-call recovery.

### 2.6 Search contract correctness

- `search_graph` + `query` + `name_pattern`/`qn_pattern`/`relationship`/degree/entry-point/
  `include_connected` now returns an explicit error naming the incompatible fields. `label`,
  `file_pattern`, `kind` and `namespace` remain supported with BM25.
- `search_code` schema: `pattern` and `query` are both declared, neither is individually required,
  and `anyOf` requires one. The handler also rejects the neither-present case with a descriptive
  message.
- `search_code` `match_count` is the true count; `returned_matches` is the page size.
- CLI `gry search` now accepts a structural-only query (`query` is optional when `--name-pattern`
  or `--qn-pattern` is present), matching the MCP contract.

### 2.7 Concurrency and publication

`ProjectLock::drop` no longer unlinks the lock file. The old unlink created a classic advisory-lock
inode race. The lock file is small and remains in the cache directory.

The pre-publication `prune_orphan_nodes` call was removed from the index pipeline. It ran before the
file/source limit check, so a rejected run could mutate the last valid generation. Delta publication
already deletes rows for deleted/ignored files and calls `delete_orphan_nodes` inside its
transaction. A regression test confirms a newly ignored file loses its nodes.

### 2.8 Contract and code hygiene

- `delete_project` requires `project` in schema and handler; deletion never defaults.
- `manage_adr` and `ingest_traces` require an explicit project in schema and handler.
- `get_code_snippet` returns candidate QNs for ambiguous names.
- Removed unused cache-hash computation in `adr_put`, unused `source` parameter in `slice_tokens`,
  unused `project` parameter in `recompute_importance`, and an unused `stdin.is_terminal()` call.
- Added a tool-schema consistency test: unique names, non-empty bounded descriptions, object
  schemas, required keys declared, `anyOf` keys declared.

## 3. Verification evidence

All commands were run from the repository root on 2026-11-03.

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo check --workspace` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass |
| `cargo test --workspace` | 236 tests pass, 0 failed |
| `cargo build --release` | pass |
| `GRY_OPENCODE_LIVE=1 cargo test -p gry --test opencode_runtime` | 3 tests pass |
| `bash scripts/install_test.sh` | 9 passed, 0 failed (no network) |
| `gry index` on the 10k synthetic Go fixture | pass |
| `gry index` unchanged / one-file edit | pass; performance in `performance-report.md` |
| `eval/run_context_eval.py` (baseline and current) | identical metrics on the same fixture |
| Graph invariant test after full + incremental + delete | pass |

The test count grew from 222 to 236. No existing test was weakened or removed; one test
(`coverage_reports_missing_and_indexed`) was strengthened to use a real filesystem and assert the
new failure states, and the old `hashes_are_reused_when_mtime_and_size_match` test was replaced
because it asserted the defect itself.

## 4. Compatibility and rollback

- No SQLite schema version change. `SCHEMA_VERSION` and `FORMAT_VERSION` remain 3.
- Existing databases open unchanged.
- The first index after upgrade rebuilds each project once because the per-project extractor marker
  is absent. This is intentional and safe.
- Existing project rows, file hashes, nodes, edges, coverage, parse cache and ADRs are preserved
  until a successful replacement generation commits.
- Rollback is code-only: run the previous binary. It will read the per-project marker table as
  ordinary `store_meta` rows; the global marker remains present.

## 5. Not changed

- The static token embedding model and semantic ranking weights.
- The SQLite schema and query planner.
- The context ranking algorithm and per-language extraction rules.
- Tool pagination size and profile membership.
- Installer adapters, other than the shared project-resolution behavior exercised by hooks.
- The context freshness stat heuristic (documented budget limitation).

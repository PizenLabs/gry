# gry × ohgo Integration Report — 2026-10-26

> **Status:** draft for fix planning
> **Owner:** gry index / resolve / query / MCP output
> **Counterpart:** `ohgo/docs/report/GRY_MCP_INTEGRATION_REPORT_2026-10-26.md`
> **Run:** `gry 1.0.2`, `ohgo v0.0.1-15-g97b06af-dirty`
> **Target:** `go-template` (5 Go files, 278 lines), generation
> `1793012093799-29fad916207a4e648a3fe6ef8a4e4668`
> **Verdict:** gry was **not** the primary cause of the failed run. The target
> project was not indexed when ohgo started. gry nevertheless has seven
> correctness and relevance defects that make graph-assisted exploration
> unreliable once indexing is present.

---

## 1. Scope and method

The live ohgo run happened at 09:03–09:06 UTC on 2026-10-26. The `go-template`
index was created at 10:54:53 UTC, after the run. I replayed the same
exploration against the indexed project through gry MCP and exercised:

`list_projects`, `index_status`, `get_architecture`, `check_index_coverage`,
`search_graph`, `search_code`, `get_file_outline`, `get_code_snippet`,
`trace_path`, `query_graph`, `get_graph_schema`, `detect_changes`.

All replay calls were read-only. The repository was not modified by gry.

**Baseline facts about the target:**

| Fact | Value |
|---|---|
| Indexed files | 55 |
| Files under `.izen/` + `.ohgo/` | 39 (71%) |
| Real project files | 16 |
| `Variable` nodes | 567 |
| `EnvVar` nodes | 4 (in `go-template`) |
| Go functions | 16 |
| Routes | `GET /`, `GET /health` |

The `gry` project itself, used as a second corpus, has **131 `EnvVar` nodes**;
115 contain lowercase letters and 9 are purely numeric (see G5). That is the
clearest evidence that the env-var extractor is over-broad.

---

## 2. Why the live run failed (context, not a gry defect)

At 09:03:48, `gry.list_projects` returned only `bench` and `gry`. The target
`go-template` was absent. Four of five graph tools correctly returned:

```text
project not found or not indexed: go-template
```

`search_graph` returned `{"total":0,"results":[]}` instead (G1). ohgo then
classified the run as `informational`, withheld structural tools, and never
indexed the target (correctly: indexing is a mutation). The harness side is
specified in the ohgo report.

After indexing, the same objective is satisfiable: architecture, entry points,
routes, symbols, outlines, snippets, traces, and coverage all return useful
results. The remainder of this report is about the defects that remain.

---

## 3. Findings

Severity: **P1** corrupts or silently empties an answer, **P2** degrades
relevance or recovery, **P3** polish.

### G1 — P1: `search_graph` silently returns zero for an unindexed project

**Evidence.** During the run:

```json
{"project":"go-template","total":0,"returned":0,"results":[]}
```

while `get_architecture`, `check_index_coverage`, `index_status`, and
`query_graph` all failed with `project not found or not indexed`.

Reproduced on 2026-10-26 with a fake project:

```text
search_graph(project="zzz-not-indexed") -> total: 0
get_architecture(...)                  -> Error: project not found or not indexed
check_index_coverage(...)              -> Error: project not found or not indexed
index_status(...)                      -> Error: project not found or not indexed
query_graph(...)                       -> Error: project not found or not indexed
```

**Root cause.** `search.rs:37` never calls `store.require_project`. Other read
paths do (`search.rs:318`, `search.rs:550`, `architecture.rs:16`,
`impact.rs:71`). `store.bm25` / `store.search` return an empty page for an
unknown project. The handler (`handlers.rs:301-347`) passes the name through
`ctx.project()`, which only validates that a name was supplied
(`handlers.rs:37-69`).

**Fix.** Call `store.require_project(&req.project)?` at the top of
`search_graph`, before BM25 or structural search. This is a one-line change plus
a test.

**Test.** `search_graph` against an unknown project returns the same typed error
as `get_architecture`. A project that exists but has zero matching symbols still
returns `total: 0`.

---

### G2 — P1: agent-state directories dominate the index

**Evidence.**

- 39 of 55 indexed files are `.izen/` or `.ohgo/` artifacts.
- Top `Variable` producers: `.izen/runtime/snapshot.json` (162),
  `.ohgo/progress.json` (143), `.izen/sessions/A/session.json` (23),
  `.izen/sessions/A/context.json` (20), plus more state JSON. The visible
  state files account for roughly 450 of 567 `Variable` nodes (~80%).
- `search_code "gin.New"` returned `.ohgo/progress.json` **first**, ahead of
  `internal/server/routes_test.go`, because the agent transcript contains a copy
  of the source.
- `.izen` is ignored by `~/.gitignore_global` (`core.excludesFile`) but still
  indexed. `.ohgo` is not ignored at all in this repository.

**Root cause.**

1. `global_gitignore()` reads only `$XDG_CONFIG_HOME/git/ignore`
   (`discover.rs:521-535`). It does not read Git's configured
   `core.excludesFile` (`~/.gitignore_global` here), nor the default
   `~/.config/git/ignore` when Git is configured differently.
2. `BUILTIN_DIRS` (`discover.rs:117-168`) has build/cache directories but no
   agent-state directories (`.ohgo`, `.izen`, `.crush`, `.aider*`,
   `.claude`, `.codex`, etc.).
3. Any `.json` / `.yaml` file is parsed into `Variable` nodes, including
   transcripts and checkpoints.

**Fix.**

1. Resolve the global exclude in this order: `core.excludesFile` from
   `git config --get core.excludesFile`, then `$XDG_CONFIG_HOME/git/ignore`,
   then `~/.config/git/ignore`.
2. Add a small default agent-state list to `BUILTIN_DIRS`, or document a
   project-level `.gryignore` and make `gry init` write one. Prefer a default
   list; requiring every project to opt out is unsafe.
3. Do not extract `Variable`/`Module` symbols from arbitrary JSON/YAML. Parse
   only recognized config files (for example `.air.toml`, `package.json`,
   `docker-compose.yml`). Transcripts and checkpoints are not configuration.
4. Keep coverage reporting able to show ignored state directories, so the
   omission is visible rather than silent.

**Test.** Index the `go-template` corpus. Assert:

- `File` count excludes `.izen/` and `.ohgo/`.
- `Variable` count is representative of source/config, not transcript keys.
- `search_code("gin.New")` returns `internal/server/routes_test.go` first.
- A global `core.excludesFile` entry is honored.

---

### G3 — P1: false `CALLS` edge from a short-name collision

**Evidence.** `trace_path("RegisterRoutes")` reports:

```text
RegisterRoutes -CALLS-> database.New
```

`internal/server/routes.go` does not call `database.New`. It calls
`cors.New(...)` at line 13. `database.New` is called from `cmd/api/main.go`
(indirectly, via `server.NewServer`).

**Root cause.** `Registry::resolve_call` (`resolve.rs:332-476`):

- `cors.New` has receiver `cors`, but `cors` is an external import with
  `target_module = None`; no candidate matches, so the import bonus is skipped.
- `New` has only one internal candidate, so the `indices.len() == 1` block
  (`resolve.rs:428-433`) adds `+0.25`, and the suffix block adds `+0.10`.
  `0.25 + 0.25 + 0.10 = 0.60`, which clears the `0.45` threshold.
- The resolver does not reject a receiver that is a known external import.

**Fix.**

1. When `call.receiver` is present and binds to an import with
   `external == true` (or `target_module == None`), return `None` unless a
   candidate actually belongs to that import. Never fall back to unique-name
   matching for an external receiver.
2. Do not apply the `indices.len() == 1` bonus when a receiver is present
   unless the candidate is same-class/self or same-file.
3. Record `confidence` and `resolved_by` (already present) and surface
   `ambiguous` in trace output when the margin is small.
4. Add a Go fixture: `cors.New` must not resolve to an internal `New`; a real
   internal `database.New` call must still resolve.

**Test.** `trace_path("RegisterRoutes", direction="outbound")` has no
`database.New` edge. `trace_path("main")` still reaches `database.New`.

---

### G4 — P2: grouped Go imports produce pseudo-packages and mark internal packages external

**Evidence.** `get_architecture` returned packages whose names are raw import
blocks:

```text
__package__go/(\n\t"context"\n\t"database/sql"\n\t"fmt"\n...
```

and internal packages:

```text
go-template/internal/database   external = "true"
go-template/internal/server     external = "true"
```

The graph stores `Package.external` as a **string** (`get_graph_schema` reports
type `string`), while `get_architecture` renders it as `true`. It should be a
boolean.

**Root cause.**

1. `is_import_node` accepts any node whose kind contains `import`
   (`extract.rs:1542-1559`). In Go that includes `import_declaration`,
   `import_spec_list`, and `import_spec`. `collect_node_signals` visits every
   node (`extract.rs:599-605`), so the whole block is parsed by the generic
   fallback at `extract.rs:1758`, and each `import_spec` is parsed again at
   `extract.rs:1650-1672`.
2. Go module resolution registers the source `package` name (`server`, `main`)
   as a package root (`resolve.rs:116-136`), not the module path from `go.mod`.
   `go-template/internal/database` therefore does not match a root and falls
   through to the external binding (`resolve.rs:309-313`).

**Fix.**

1. For Go, only parse `import_spec`. Return `None` for
   `import_declaration` / `import_spec_list` in `parse_import`, or narrow
   `is_import_node` by language.
2. Read the `module` directive from the root `go.mod` and register it as a
   project-level package root. Resolve `module_path/sub/pkg` to the directory
   `sub/pkg`, and then to the module QN of the files in that directory.
3. Store `external` as a boolean and default it to `false` for module-internal
   imports.
4. Add a fixture with a grouped import block and one module-internal import.

**Test.** No `Package` name contains `\n` or `"`. No `__package__` name starts
with `__package__go/(`. `go-template/internal/database` and
`go-template/internal/server` have `external == false`.

---

### G5 — P1: any call named `get` is treated as an environment read

**Evidence.**

- `go-template` has `CONFIGURES` edges from `RegisterRoutes` to
  `EnvVar "/"` and `EnvVar "/health"`. Those are Gin route paths, not env vars.
- `gry` itself has **131 `EnvVar` nodes**. 115 contain lowercase letters and 9
  are numeric (`0`–`8`). Examples: `path`, `file_path`, `base_branch`,
  `body_hash`, `apiVersion`, `arguments`, and `__node_id`.
- `CONFIGURES` edges in `gry` include `get_graph_schema -> "0"`,
  `clear_fts_project -> "0"`, and `parse_flat_config -> "1"`.

**Root cause.** `record_call_signals` treats **any** call whose lowercased name
is `get` as an env read (`extract.rs:665-680`). In Go, `r.GET("/")` matches
`get`; in Rust, `map.get("key")` matches; in JavaScript, generic `.get` calls do
the same. The receiver is not checked.

**Fix.**

1. Remove bare `get` from the env-access set. Keep explicit APIs:
   `getenv`, `getenvs`, `getenvironmentvariable`, `environ_get`,
   `process.env`, and language-specific accessors.
2. If `get` must be supported, require a receiver that identifies a config/env
   object (`os`, `env`, `config`, `viper`, `process.env`) and a language-aware
   allowlist.
3. Do not emit `CONFIGURES` for HTTP verbs. Route extraction already handles
   `r.GET`; the two paths must be mutually exclusive.
4. Add per-language tests: Go `r.GET("/")` -> `Route`, not `EnvVar`; Go
   `os.Getenv("PORT")` -> `EnvVar PORT`; Rust `map.get("x")` -> no `EnvVar`;
   JS `process.env.X` -> `EnvVar X`.

**Test.** Re-index `gry`: `EnvVar` count drops from 131 to the real set
(`GRY_MAX_INDEX_*`, `GRY_WORKERS`, `HOME`, `PATH`, `XDG_CONFIG_HOME`, plus
genuine config accessors). `go-template` has no `EnvVar "/"` or `"/health"`.

---

### G6 — P2: phantom `File "..."` node from a glob token

**Evidence.** The graph contains:

```text
label: File
name: "..."
qn: go-template.....__file__
file_path: "..."
placeholder: "true"
```

It appears in `file_tree` and in the `File: 56` count, but there is no
`CONTAINS_FILE` edge (55) and no file named `...` on disk.

**Root cause.** The Makefile contains `go test ./...` (line 19). The Makefile
infra parser treats any token starting with `./` as a program/path
(`infra.rs:1092-1113`) and emits `@file:...`. `resolve_infra_qn` then creates a
placeholder `File` node for the unresolved path (`build.rs:1214-1226`).

**Fix.**

1. Reject glob tokens (`...`, `*`, `?`, `[`) in `parse_makefile` before creating
   a file reference.
2. Do not create placeholder `File` nodes for globs. If a placeholder is
   necessary for an unresolved infrastructure reference, label it as synthetic
   and exclude it from `file_tree`, `File` counts, and `check_index_coverage`.
3. Add a Makefile fixture with `go test ./...` and `go build ./...`.

**Test.** No `File` node with path `...`. `File` count equals
`CONTAINS_FILE` count. `get_architecture(file_tree)` does not list `...`.

---

### G7 — P2: `detect_changes` loses changed rows under the default output budget

**Evidence.** On the `go-template` working tree (no commits, 22 untracked
files), with defaults:

```json
"changed_files": {"total":22,"returned":0,"offset":0,"has_more":true,"rows":[]}
"impact":        {"total":4,"returned":0,"offset":0,"has_more":true,"next_offset":null,"rows":[]}
"seeds":         {"total":247,"returned":42,...}
```

With `max_output_tokens: 20000`, `changed_files.returned` becomes 22 and rows
appear. With `seeds: "none"`, the changed rows still disappear at 3200.

**Root cause.** `render::shrink` trims arrays in this order
(`render.rs:121-131`):

```text
semantic.rows, coverage_rows, impact.rows, changed_files.rows,
seeds.rows, modules.rows, results, rows
```

`impact` and `changed_files` are trimmed **before** the much larger `seeds`
array. After trimming, it unconditionally sets `has_more: true` and never sets
`next_offset` for `changed_files` (`render.rs:145-153`). `changed_files` has no
`next_offset` in the query output either (`impact.rs:309-315`).

**Fix.**

1. Trim `seeds.rows` before `changed_files.rows` and `impact.rows`; seeds are
   supporting detail, changed files are the answer.
2. Always preserve at least one changed row and one impact row when the totals
   are non-zero and the budget allows one coherent row.
3. Add `next_offset` to `changed_files`, `seeds`, and `modules`, and make
   `has_more` reflect the actual page. Do not set `has_more: true` without a
   `next_offset`.
4. On a repository with no commits, return `git_available: true` plus a warning
   such as `no_commits: true; all files are reported as added`. Do not present
   a no-commit worktree as a normal diff.

**Test.** A fixture with 25 untracked files and default
`max_output_tokens: 3200` returns at least one changed row and a valid
`next_offset`; `seeds` are truncated first.

---

### G8 — note: in-progress output-budget work

The working tree already contains a useful change: `render.rs` now collapses
oversized top-level arrays to `{count, omitted}` and `architecture_tool` defaults
to 8000 tokens (`handlers.rs:483-489`). This fixes the "architecture degrades to
an empty error" class.

It does **not** fix G7: the new pass runs only after the old `shrink`, and it
only handles top-level arrays. `changed_files.rows` and `impact.rows` are
nested objects handled by the old pass. When completing that work:

- keep scalar metadata (`project`, `generation`, `total`, `has_more`);
- update `next_offset` whenever rows are removed;
- add a regression test for `detect_changes` at 3200 tokens.

---

### G9 — P1: delta re-index left nodes for removed or newly ignored files

**Evidence.** After fixing discovery to ignore `.izen/` and `.ohgo/`, re-indexing
the live project reported `files: 16` but `nodes: 693` and `Variable: 567`.
Direct inspection showed 524 node rows under `.izen/` and `.ohgo/`, while
`file_hashes` contained only 16 rows. A second re-index took the "unchanged"
fast path and never reconciled them. The live graph still contained the old
`EnvVar "/"` and `"/health"` nodes and the `RegisterRoutes -> New` edge.

**Root cause.**

1. `delete_file_rows` deletes hashes, cache, file metadata, symbols and refs,
   but not `nodes` (`gry-store/src/lib.rs:1555`). Deleted files therefore keep
   their nodes and edges.
2. The unchanged fast path (`pipeline.rs:206-267`) returns before any
   reconciliation when hashes match, so orphan nodes survive indefinitely.
3. Global nodes (`EnvVar`, `Package`, `Process`, ...) have an empty
   `file_path`, so file-level deletion never sees them.
4. Extractor semantics changed without an extractor-version bump, so the next
   index legitimately reused the old parse for unchanged files.

**Fix.**

- Add `delete_orphan_nodes` and call it during delta publication and before the
  unchanged fast path (`Store::prune_orphan_nodes`).
- Delete unattached global nodes as part of the same reconcile, including FTS
  rows.
- Bump `EXTRACTOR_VERSION` from `25` to `26` so an upgrade forces one clean
  rebuild instead of preserving stale edges.

**Test.** `delta_publish_removes_nodes_for_files_no_longer_present`; live
re-index now reports 16 files, 159 nodes, 43 variables, 2 env vars, and no
`RegisterRoutes -> New` edge.

---

## 4. Non-findings (do not investigate)

These looked like defects but are correct or external:

1. **`project not found or not indexed`** for four graph tools is the correct
   behavior when the project is absent. Only `search_graph` is wrong (G1).
2. **`index.html` lines 50–53 `parse_partial`** is real: the file contains
   leftover `=======` merge markers. gry reported the gap honestly.
3. **16 KiB discovery payload** is large but not wrong. The harness truncation
   is the ohgo-side defect; gry can still add a compact discovery mode.
4. **`detect_changes` reporting 22 added files on a no-commit worktree** is
   factually correct; the missing part is the warning and paging, not the count.
5. **No graph mutation occurred.** Every failure was a read or a rejected
   capability.

---

## 5. Fix plan

### Phase 0 — fixtures and invariants (S)

Add `tests/fixtures/go-gin-mini/`:

```text
go.mod                 module example.test/mini; go 1.22
cmd/api/main.go        calls mini/internal/server.NewServer
internal/server/routes.go  RegisterRoutes with r.GET("/") and cors.New
internal/server/server.go  NewServer calls database.New
internal/database/database.go  New/Health/Close
internal/server/routes_test.go  gin.New
.air.toml
Makefile               go test ./...
.ohgo/progress.json    a small transcript copy
.izen/runtime/snapshot.json
.gryignore             optional negative test
```

Add integration assertions for G2–G6 and a 25-file untracked fixture for G7.

### Phase 1 — correctness that corrupts answers (M)

- G3: external-receiver guard in `resolve_call`.
- G4: Go import parsing and module-path resolution.
- G5: remove bare `get` from env extraction.
- G1: `require_project` in `search_graph`.

These four change graph meaning; land them with fixture tests before touching
output budgeting.

### Phase 2 — index relevance (M)

- G2: `core.excludesFile`, agent-state defaults, JSON config allowlist.
- G6: glob-aware Makefile parsing and synthetic-file handling.
- Add `gry init` / docs for `.gryignore`.

### Phase 3 — MCP output and impact paging (M)

- G7/G8: trim order, `next_offset`, no-commit warning, 3200-token regression.
- Keep the architecture 8000 default; document it in `CAPABILITIES.md`.

### Phase 4 — cross-repo

- ohgo O9 adds `.ohgo/` to `.gitignore`; gry G2 ignores it regardless.
- ohgo O4 preserves gry's `project not found or not indexed` text.
- ohgo O3 uses gry's `kind: prompt` vs `kind: tool` distinction.

---

## 6. Acceptance criteria

1. `search_graph` on an unknown project returns
   `project not found or not indexed: <name>`, matching the other readers.
2. Indexing `go-template` excludes `.izen/` and `.ohgo/`; `search_code
   "gin.New"` returns the Go test first.
3. `trace_path("RegisterRoutes")` has no `database.New` edge; `main` still
   reaches `database.New`.
4. No `Package` name contains newlines or raw import-block text; module-internal
   packages are `external == false` and the property is a boolean.
5. `EnvVar` nodes contain only real environment keys; no `"/"`, `"/health"`,
   `"path"`, `"0"`, or `"apiVersion"`.
6. No `File "..."` node; `File` count equals `CONTAINS_FILE` count.
7. `detect_changes` at 3200 tokens returns changed rows plus `next_offset`, and
   trims seeds first.
8. All existing Rust unit/integration tests pass, plus the new Go fixture tests.

---

## 7. Appendix — reproduction queries

Run against a disposable gry data directory, not the live one.

```cypher
// G2: state files and variables
MATCH (f:File) WHERE f.file_path STARTS WITH '.izen' OR f.file_path STARTS WITH '.ohgo'
RETURN f.file_path, f.language ORDER BY f.file_path;

MATCH (f:File)-[:DEFINES]->(v:Variable)
RETURN f.file_path, count(v) AS vars ORDER BY vars DESC LIMIT 15;

// G4: pseudo-packages and external flags
MATCH (p:Package) WHERE p.qn CONTAINS 'go-template/internal'
RETURN p.name, p.external;

// G5: false env vars
MATCH (v:EnvVar) RETURN v.name, v.qn ORDER BY v.name;
MATCH (v:EnvVar) WHERE v.name =~ '.*[a-z].*' RETURN count(v);

// G6: phantom file
MATCH (f:File {file_path:'...'}) RETURN f;
```

MCP checks:

```text
search_graph(project="zzz-not-indexed")  -> must error
get_architecture(project="zzz-not-indexed") -> error (already correct)
detect_changes(project="go-template")    -> changed_files.returned > 0
```

### Reference run

| Item | Value |
|---|---|
| gry version | 1.0.2 |
| ohgo version | v0.0.1-15-g97b06af-dirty |
| Target generation | `1793012093799-29fad916207a4e648a3fe6ef8a4e4668` |
| Target indexed at | 2026-10-26T10:54:53.803Z |
| Live run | 2026-10-26T09:03–09:06Z |
| gry project generation | `1793002014001-b1906b0cda7d45e2adecb705a3f336c4` |

---

## 8. Fix status (2026-10-26)

| ID | Status | Change | Verification |
|---|---|---|---|
| G1 | Fixed | `search_graph` now calls `store.require_project`. | `unknown_project_is_an_error`; unknown project returns the typed error. |
| G2 | Fixed (core) | `core.excludesFile` + `~/.gitignore_global` fallback; `.ohgo`, `.izen`, `.crush`, `.aider`, `.codex`, `.claude`, `.cursor` added to built-in ignores. | Re-index: 16 files (was 55), 43 variables (was 567), no state JSON. Arbitrary JSON config parsing was left unchanged. |
| G3 | Fixed | External import receivers block unique-name resolution. | `external_receiver_does_not_absorb_unique_local_name`; `RegisterRoutes` no longer links to `database.New`. |
| G4 | Fixed | Go imports only parse `import_spec`; module-path package directories resolve locally. | `go_grouped_imports_are_not_one_import_block`, `go_module_internal_import_is_not_external`; no pseudo-packages; `main → NewServer → New` still resolves. |
| G5 | Fixed | Bare `get` no longer means env access; env-like receivers only. | Go/Rust extractor tests; re-index has only `PORT` and `BLUEPRINT_DB_URL`, no route paths. |
| G6 | Fixed | Makefile globs skip file references; placeholders reject glob paths. | `make_globs_are_not_file_references`; re-index has zero `File "..."` nodes. |
| G7 | Fixed (core) | Seeds trim before changed/impact; `next_offset` maintained; runtime-state paths filtered from changes. | `truncation_prefers_changed_files_over_seeds`, `truncation_sets_a_usable_next_offset`; `changes` returns 18 real files with valid paging. `modules.next_offset` remains for a follow-up. |
| G8 | Unchanged | Pre-existing array-collapsing render work retained. | `cargo fmt --check` and `cargo test --workspace` pass. |
| G9 | Fixed | Delta publication reconciles orphan file nodes and unattached globals; `EXTRACTOR_VERSION` bumped to 26 to force one clean rebuild. | `delta_publish_removes_nodes_for_files_no_longer_present`; live nodes drop 693 → 159, env vars 4 → 2. |

End-to-end verification used a disposable copy and a separate database:

```text
Files: 16 (was 55)
Variables: 43 (was 567)
EnvVars: PORT, BLUEPRINT_DB_URL (was 4, including "/" and "/health")
Phantom File "...": 0
Pseudo-packages: 0
RegisterRoutes -> database.New: absent
main -> NewServer -> New: present
search-code "gin.New": internal/server/routes_test.go first
changes: 18 rows, has_more=false, no .ohgo/.izen
```

`cargo test --workspace` passes. The installed `~/.local/bin/gry` was not
replaced; rebuild/install to deploy the fixed binary.

# GRY production-readiness audit — 2026-11-03

**Scope:** working tree at `01e422f` (`perf/maturity`, version 1.0.3) plus the remediation
recorded in `production-readiness-remediation.md`. The repository was clean at the start of the
audit; no user changes were discarded.

**Method:** read the implementation, compare it with the documented contract, reproduce failures
through the public MCP and CLI surfaces, add regression tests, then re-run the full verification
matrix. Measurements were taken on the machine described in `performance-report.md`. No result in
this document is inferred from documentation alone.

## 1. Repository inventory

- Rust workspace with seven crates: `gry-core`, `gry-store`, `gry-parse`, `gry-index`, `gry-query`,
  `gry-mcp`, `gry-cli`.
- 61 Rust source files, 40,237 lines after remediation.
- SQLite (bundled) persistence, FTS5 BM25, static Nomic vectors, tree-sitter grammars, MCP over
  stdio, one CLI binary.
- 18 MCP tools, 2 prompts, 3 tool profiles.
- Existing quality gates: `cargo fmt --check`, `cargo check`, `cargo clippy -D warnings`,
  `cargo test --workspace`, release build, differential/incremental tests, an OpenCode plugin and
  live MCP test, and a reviewed `goca-v1` context/hook evaluation fixture.

## 2. Architecture assessment

The crate split is coherent and dependency direction is sensible: `gry-core` owns shared types,
`gry-store` owns SQLite and query primitives, `gry-parse` owns extraction, `gry-index` owns
discovery/resolution/publication, `gry-query` composes graph services, and `gry-mcp`/`gry-cli` are
transport surfaces over the same dispatch path. CLI `gry cli <tool>` calls the same handlers as MCP,
which keeps argument semantics aligned.

The publication model is the strongest part of the design: one SQLite transaction replaces a
project generation, so a failed or cancelled run does not expose a partial graph. The delta path
compares incremental output with clean rebuilds in tests.

The most important weakness found was not architectural: it was inconsistent handling of an
**unavailable source root**. Graph-only reads correctly continued to work, but source-reading paths
turned failure into an apparently verified empty answer. That is a trust defect, not a missing
feature, and it is the main reason this audit did not treat the previous “ready” claim as sufficient.

## 3. Confirmed defects

Severity: **P1** corrupts or silently empties an answer; **P2** reliability or interoperability
risk; **P3** contract/polish.

| ID | Severity | Subsystem | Finding | Evidence |
|---|---|---|---|---|
| F1 | P1 | coverage | A file recorded in `file_hashes` but absent from disk was reported `status=indexed`, `fresh=true`. A missing project root produced the same result. | Live `check_index_coverage` on the stale `gry` project returned `indexed/fresh` while `/Users/.../RnD/gry` did not exist. Root cause: `stale` was only evaluated when metadata existed and the `Some(hash)` match arm defaulted to indexed. |
| F2 | P1 | source search / snippets | `search_code` returned `total_files:0` with success when the project root was missing. `get_code_snippet` returned `source:""` with success. | Live MCP calls against the stale `gry` project. The repository root was absent; both tools reported success instead of unavailable evidence. |
| F3 | P1 | incremental indexing | Discovery reused the stored SHA-256 whenever `mtime_ns` and size matched, even though it had just read the bytes. A same-size edit with a preserved timestamp could be missed. | Code path in `discover`; regression test writes a same-size body, restores the original mtime, and now asserts the hash and index generation change. |
| F4 | P1 | extractor cache | The extractor version marker was global (`store_meta.extractor_version`). After project A was rebuilt with a newer extractor, project B could still take the delta path with old extraction semantics. | Code path in `pipeline`/`delta`; new test sets a stale per-project marker and asserts a full rebuild. |
| F5 | P2 | MCP project resolution | Default resolution only matched an exact/canonical root. A session started in a subdirectory or after a project move failed with the opaque `project is required; call list_projects...`; a deleted remembered project stayed selected. | Reproduced live with the release binary and the five-project local cache; the same failure shape was reported in the OpenCode session. |
| F6 | P2 | MCP contract | `search_code` required `pattern` while `search_graph` uses `query`. An agent that carried the `query` habit received `missing required argument: pattern`. | Live tool call and the reported OpenCode failure. Server-side handler rejection, not a schema-metadata illusion. |
| F7 | P2 | search correctness | When `query` was supplied, `search_graph` silently ignored `name_pattern`, `qn_pattern`, `relationship`, degree and entry-point filters. | Live call with `query` + `qn_pattern` returned 191 unfiltered BM25 rows. |
| F8 | P2 | source search | `match_count` was capped at `match_limit`, under-reporting matches in a file. | Code inspection; regression test with 3 matches and `match_limit=1` now asserts `match_count=3`, `returned_matches=1`. |
| F9 | P2 | indexing concurrency | `ProjectLock::drop` unlinked the lock file. A second process could lock the old inode while a third created a new inode at the same path, allowing concurrent publication. | Code inspection; fixed by keeping the lock file. No deterministic multi-process regression test exists. |
| F10 | P2 | incremental performance / publication | A whole-graph `prune_orphan_nodes` scan ran before every index, before the limit check, and outside the publication transaction. It was redundant with delta reconciliation and could mutate the last valid generation when the run was later rejected. | Timing showed the scan dominated the incremental path; removal reduced p50 edit latency from 409 ms to 270 ms and unchanged latency from 349 ms to 229 ms on the 10k fixture. |
| F11 | P3 | MCP contract | `delete_project` declared `project` optional in its property description but required it in the schema; the handler defaulted to the workspace project for a destructive operation. | Schema/handler comparison; now required in both. |
| F12 | P3 | MCP contract | `manage_adr` and `ingest_traces` had the same “optional” description with a required field and a defaulting handler. | Schema/handler comparison; writes now require an explicit project. |
| F13 | P3 | snippets | An ambiguous short name was reported as “symbol not found” instead of listing candidates. | Code path; new test with two equally authoritative `Run` methods asserts `ambiguous symbol` plus both QNs. |
| F14 | P3 | dead code | `adr_put` computed and discarded a cache path, `slice_tokens` accepted an unused source, and `recompute_importance` accepted an unused project. | Code inspection; removed without behavior change. |

## 4. Reliability findings

Verified behavior (tests or direct runs):

- Publication remains a single SQLite transaction. Existing `failed_or_cancelled_run_preserves_previous_generation`
  still passes.
- Deleted or newly ignored files are removed by delta publication; new regression test
  `newly_ignored_files_are_removed_by_delta_publication`.
- Graph invariants now have an explicit integration assertion: no dangling edges, no cross-project
  edges, no invalid source ranges, no empty identities, no duplicate QNs, no file-owned node without
  a file record. It runs after full and incremental updates (including a delete).
- A stale per-project extractor marker forces a clean rebuild rather than preserving old edges.
- Cached extraction decode failure falls back to parsing; corrupt cache is not a hard failure.
- `search_code` now reports `complete=false` and samples of unreadable files when only part of the
  indexed set could be read; it errors if no indexed source file is readable.
- Source snippets fail explicitly when the root/file is unavailable or the recorded range is outside
  the current file. Outline mode remains available from the graph without source access.

Risks that remain (not silently fixed):

- Context freshness (`verify_freshness`) still trusts stat equality without rehashing. Rehashing up
  to 64 files of up to 8 MiB each would exceed the context latency budget. The limitation is
  documented in the payload.
- The per-project extractor marker is written after the publication commit. A crash between the two
  leaves a published graph that forces a full rebuild next time; it does not preserve stale edges.
- MCP `initialize` roots are not the standard `roots/list` exchange. Workspace and descendant
  resolution cover the normal client path; clients that only advertise roots in `initialize` are
  handled, clients that expect a server-initiated `roots/list` request are not.

## 5. Security findings

No SQL injection path was found in user-supplied Cypher: the Cypher parser builds an in-memory
graph and does not concatenate SQL. SQLite values are parameterized.

Path handling was reviewed:

- `resolve_index_root` canonicalizes and enforces `GRY_ALLOWED_ROOT`.
- Discovery does not follow symlinks and skips the safety core (`.git`, `node_modules`, worktrees)
  even when `.gryignore` whitelists a directory.
- Source snippets/search use paths from the index, not raw caller paths.
- Secrets: GRY does not execute repository-provided commands during indexing. Git is invoked only for
  branch/status/history queries with fixed arguments.
- ADR and trace data are stored in SQLite, not written to arbitrary repository paths.

Residual risk: the database file is created with the process umask; `ensure_dirs` sets `0700` on the
cache directory on Unix but the database file mode is not explicitly tightened. This is a local
single-user tool; no regression was introduced. It is recorded as a hardening opportunity, not a
confirmed exploit.

## 6. Performance findings

Detailed measurements and environment are in `performance-report.md`. The confirmed bottleneck was
the redundant pre-publication orphan scan (F10). Removing it improved incremental p50 by 34% on the
10k synthetic Go fixture with no retrieval or correctness regression. Full-index time and peak RSS
were statistically unchanged.

## 7. Retrieval-quality findings

The reviewed `goca-v1` evaluation was run with the baseline and current binaries against the same
fixture. Results are identical; the difference from the 2026-10-27 published baseline is fixture
drift (one untracked file), not a code regression. Details and per-language/task limits are in
`retrieval-quality-report.md`.

The evaluation confirms the previous conclusion: recall is strong, precision is structurally lower
because the context intentionally returns supporting files, and Go type-only impact remains a known
limitation.

## 8. MCP and agent interoperability findings

The two reported OpenCode failures were reproduced and separated by cause:

1. **Missing project:** the default root did not match any indexed project and the error did not
   name alternatives. This was a GRY resolution/recovery defect. Fixed with descendant-root
   resolution, stale remembered-project validation, and an error that lists indexed projects.
2. **`search_code` `query` rejection:** this was a GRY parameter-name mismatch, not a client failure.
   Fixed with a documented `query` alias and an `anyOf` schema.

`tools/list` pagination (6 tools per page, 3 pages) is spec-compliant. The live OpenCode runtime
loads all 18 tools, so it was not the cause of the observed failure. Clients that do not follow
`nextCursor` would still see only the first page; this remains an interoperability risk.

The live OpenCode plugin and MCP connection tests pass with the current binary
(`GRY_OPENCODE_LIVE=1 cargo test -p gry --test opencode_runtime`). Claude Code and Codex were not
executed in this environment; no live result is claimed for them.

## 9. Baseline verification (before remediation)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo check --workspace` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass |
| `cargo test --workspace` | 222 tests pass |
| `cargo build --release` | pass |
| OpenCode plugin contract + live connection | pass (baseline) |
| `goca-v1` evaluation | completed; numbers recorded |

The baseline was healthy at the compiler/test level, which is exactly why the trust defects above
mattered: the suite did not exercise a moved root or a same-timestamp edit.

## 10. Remaining limitations

- Go struct/interface type declarations are not graph nodes; type-annotation-only impact is
  under-detected.
- Context freshness is bounded and stat-based when metadata is unchanged.
- Tool discovery is paginated; untested clients may not follow cursors.
- 100k-file scale and Windows behavior are unmeasured.
- No live Claude Code/Codex run was performed.
- Multi-process lock behavior is reasoned and fixed, but not fuzz-tested under process crashes.
- The per-project extractor marker is not inside the publication transaction (safe rebuild, not
  atomic marker update).

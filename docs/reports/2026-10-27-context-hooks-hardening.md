# GRY Context & Hooks Hardening — 2026-10-27

> **Scope:** GRY `feature/hooks`, OHGO `fix/mcp`, and the read-only
> `go-backend-clean-architecture` evaluation fixture.
> **Status:** P0 contract closed; semantic and performance baselines established;
> OHGO headless (non-TUI) integration completed.
> **No commits, tags, releases, or pushes were made.**

---

## A. Executive summary

Completed:

- A query/evidence split in `gry context` and `gry hook`: `status` is the query
  outcome; `evidence` states whether the returned evidence is fresh, bounded,
  complete for the referenced scope, or repository-wide complete (never claimed).
- Bounded freshness verification for referenced files and their direct
  directories: new, renamed, deleted and modified files are reported; generation
  changes and file metadata changes during assembly are reported.
- New-file, rename, delete, concurrent-edit, generation-change, cache and reindex
  recovery regressions.
- Correct Go route registration (`group.POST("/task", tc.Create)` is a route, not
  an outbound HTTP call), route → handler `HANDLES` edges, type-qualified Go
  method identity, and interface-field dispatch for the evaluation fixture.
- A reviewed 9-task semantic fixture (7 development, 2 holdout) and a runner
  that reports precision, recall, MRR, relation completeness, impact quality and
  freshness separately.
- Synthetic benchmarks from 2 files to 20,002 files, including hub, deep, dense
  and disconnected graphs, plus incremental reindex and hook latency.
- An opt-in OHGO adapter (`internal/gryhook`) that invokes the documented GRY CLI
  at `before_plan`, `before_edit`, `after_edit`, `before_finish`, with bounded
  evidence, dedupe by lifecycle identity, timeout/malformed-output handling,
  environment filtering and secret redaction. OHGO retains all authority.
- A controlled end-to-end OHGO run against a temporary fixture copy.

Ready for the next integration stage: **yes for the documented headless path**;
the TUI path is not wired, and the measured ceiling is 20k files.

Most important remaining risk: GRY still cannot prove repository-wide freshness
in one query, and Go struct/interface type nodes are absent, so change impact
that depends only on a type annotation can be missed. Both are documented and
visible through `evidence`, not hidden.

---

## B. Changes by repository

### GRY — source

| File | Behaviour | Reason |
|---|---|---|
| `crates/gry-query/src/context.rs` | Adds `evidence` and extended `freshness`; SHA-256/stat verification of ≤64 referenced paths; scans ≤16 direct directories/2,000 entries for added files; post-assembly generation and metadata re-check; explicit `complete_for_scope`, `repository_scan=false`, `repository_complete=false`; task HTTP method+path route targeting; `HANDLES` traversal; target label priority; minimum two nodes under budget; tests | P0 freshness/partial contract and evidence quality |
| `crates/gry-query/src/hooks.rs` | Cache contract version 2; cache reuse only when referenced files and adjacent directories verify; no cache write for stale payloads; `before_plan` depth 3; `before_finish` is a bounded auditor (depth 2, callers/callees/imports/services/tests/changes/coverage); tests | Cache correctness and lifecycle evidence |
| `crates/gry-parse/src/extract.rs` | Route registration accepts non-routeish receivers when a callable handler argument exists; stores handler receiver type; Go method receiver scope is captured; `tc.Field.Call` receiver types; type map keeps same-named fields; tests | Routes were misclassified as outbound HTTP calls; interface dispatch and handler links were missing |
| `crates/gry-parse/src/model.rs` | `RouteDef.handler_type` | Route-to-handler resolution across files |
| `crates/gry-index/src/build.rs` | `HANDLES` target resolved through handler receiver type; `RouteHandlerRef` alias | Route → controller handler link |
| `crates/gry-index/src/resolve.rs` | Case-insensitive receiver type + suffix match; no same-file/self bonus when the receiver type differs; generated-mock penalty | Interface calls (`TaskUsecase`, `Database`) resolve to production implementations |
| `crates/gry-index/src/pipeline.rs` | `EXTRACTOR_VERSION=36` | Forces one clean rebuild after graph identity/semantics changes |

### GRY — tests, evaluation and docs

- `crates/gry-cli/tests/cli.rs`: CLI/MCP parity of `evidence`; new-file stale and
  reindex recovery; budget and hook tests.
- `crates/gry-mcp/tests/contract.rs`: MCP `get_task_context` exposes the evidence
  contract.
- `eval/tasks/goca-v1.json`, `eval/run_context_eval.py`, `eval/gen_synthetic.py`,
  `eval/benchmark.py`, `eval/README.md`, `eval/results/*.json`: reviewed semantic
  fixture, runner, synthetic generator, benchmark results.
- `docs/CAPABILITIES.md`: CTX-05/06/07, measured limits, compatibility notes.
- `docs/reports/2026-10-27-context-hooks-hardening.md`: this report.

### OHGO — source

| File | Behaviour | Reason |
|---|---|---|
| `internal/gryhook/gryhook.go` | Opt-in `gry hook` adapter; validated project/binary; filtered child env; 128 KiB stdout cap; 4 KiB stderr cap; secret-value redaction; explicit timeout/cancel/invalid-output/not-indexed failures; per-session/per-stage/per-file memo; bounded `EvidenceText`; lifecycle session and `Install` callbacks | Narrow integration without duplicating GRY graph intelligence |
| `agent/loop.go` | `SetBeforeToolCall` / `SetAfterToolCall` | Wire the existing lifecycle seams without rebuilding the loop |
| `cmd/ohgo/run.go` | Headless run attaches the adapter, injects `before_plan` evidence, runs `before_finish` | Real lifecycle integration |
| `cmd/ohgo/gryhooks.go` | Env config, plan injection, final status line | Keep wiring small and observable |
| `docs/gry-hooks.md` | Contract, enablement, lifecycle identity, failure table, security, verification, e2e result | Documented interface |
| `internal/gryhook/live_test.go` | Opt-in real-index hook check (`GRY_BIN`, `GRY_CACHE`, `GRY_PROJECT`) | Compatibility evidence without a live model |

### OHGO — tests

- `internal/gryhook/gryhook_test.go`: invocation, stage identity, dedupe of
  identical events, different-file/different-session non-suppression, timeout
  retry, malformed output, not-indexed failure, redaction, invalid project,
  oversized output, evidence bounds, disabled runner.
- `internal/gryhook/install_test.go`: real agent loop + real write tool + fake
  GRY; asserts stage order `before_plan → before_edit → after_edit →
  before_finish` and that edit results carry evidence with the authority
  boundary.

### Go fixture

No source changes. The fixture was used read-only. During the work an unintended
`git init`/initial commit appeared in its `.git` directory (source tree was
unmodified). It was detected, compared byte-for-byte with the public upstream
HEAD `8a8bf00` (the baseline HEAD), and the original `.git` was restored from
that exact commit. Final state: `main...origin/main`, clean, `go test ./...`
passes. No fixture commit remains.

---

## C. Freshness contract

### Two independent axes

- `status` — query execution: `ok`, `partial`, `stale`, `timeout`,
  `not_found`, `empty`.
- `evidence` — what the evidence supports:

```json
{
  "query_ok": true,
  "scope": "referenced_paths",
  "complete_for_scope": true,
  "safe_to_treat_as_complete": true,
  "repository_scan": false,
  "repository_complete": false,
  "freshness": "fresh",
  "reasons": [],
  "limitation": "Repository-wide completeness is not claimed. Use an explicit re-index for a global guarantee."
}
```

`freshness` carries `generation`, `generation_changed`, `stale`,
`stale_paths`, `modified_paths`, `deleted_paths`, `added_paths`,
`unindexed_paths`, `changed_during_query`, `checked_paths`,
`unverified_paths`, `scan.directories`, `scan.entries`, `scan.limited`,
`check_method`, and the explicit limitation strings.

### What is actually guaranteed

- **Generation-consistent read attempt:** the generation is captured before the
  query and re-read after assembly. A change sets `generation_changed=true` and
  `stale=true`; the payload is not presented as one coherent snapshot.
- **Referenced-file verification:** up to 64 referenced paths are checked
  against their stored hash. Metadata mismatch triggers a SHA-256 comparison, so
  a touch/checkout with identical bytes is not stale; a same-size/same-mtime
  edit is still detected when content differs.
- **Adjacent added/renamed files:** the direct parent directories of referenced
  paths are scanned (≤16 directories, ≤2,000 entries, non-recursive). New files
  with a supported language are reported as `added_paths`.
- **Deletion/rename:** a missing referenced file is `deleted_paths`; the new
  name is `added_paths` when it is in a scanned directory.
- **Concurrent edit:** metadata captured during verification is re-read after
  assembly; a change is `changed_during_query`.
- **Bounded claim:** paths beyond 64 or directories beyond 16 set
  `unverified_paths`/`scan.limited` and `complete_for_scope=false`.
  `repository_scan` and `repository_complete` are always false. A bounded scan
  never becomes a repository-wide guarantee.

### Representative outcomes

| Scenario | `status` | `evidence` | Freshness |
|---|---|---|---|
| Clean index, all referenced paths verified | `ok` | `complete_for_scope=true`, `repository_complete=false` | `fresh` |
| New file `repository/added.go` after indexing | `stale` | complete false, reason `added_files_not_indexed` | `stale`, `added_paths` |
| Rename `src/api.rs` → `src/api_renamed.rs` | `stale` | complete false | `deleted_paths` + `added_paths` |
| Referenced file beyond the 64-path bound | `ok` | complete false, reason `freshness_scope_bounded` | `unverified` |
| Index republished during assembly | `stale` | complete false, reason `generation_changed_during_query` | `generation_changed` |
| Context error/unindexed project | `not_found` (hook `not_indexed`) | no context; `query_ok=false` | not checked |
| Budget removed evidence | `ok`/`partial` | complete false, reason `output_truncated` | unchanged |

### Remaining blind spots

- No snapshot isolation across the whole query; the generation and metadata
  re-checks detect the common cases and report uncertainty, but a change after
  the final post-check is outside this result.
- No recursive repository scan. A new/renamed file outside referenced
  directories is not detected by that query; it remains outside the claimed
  scope.
- Concurrent-index stress was exercised through deterministic generation-change
  and metadata checks, not by a high-frequency multi-process fuzz run.
- `freshness.check_method` is `sha256_on_stat_mismatch`: unchanged metadata
  trusts mtime+size without re-hashing.

### Cache

- Hook cache key includes contract version 2, project, generation, stage, task,
  targets, changed paths, task id, max tokens and timeout.
- Cached entries are rejected on contract/generation/TTL mismatch.
- Reuse re-verifies referenced files and adjacent directories; a new file
  invalidates.
- `stale` payloads are not written.
- A malformed cache file is ignored and recomputed, not served.
- Tests: `cache_is_keyed_by_generation_and_inputs`,
  `cache_is_invalidated_by_an_unindexed_edit`,
  `cache_is_invalidated_by_a_new_file_beside_the_evidence`,
  `duplicate_dispatch_is_suppressed_and_marked_cached`,
  `cache_can_be_disabled`.

CLI and MCP share the same payload; `crates/gry-cli/tests/cli.rs` and
`crates/gry-mcp/tests/contract.rs` both assert `evidence`.

---

## D. Semantic evaluation results

Dataset: `goca-eval-v1`, 9 tasks (7 development, 2 holdout), K=15,
`max_output_tokens=6000`, generation
`1793045738856-7d30e82df0cf463a8152e0ed1e113d93`, GRY revision: working tree on
`feature/hooks`.

| Split | Tasks | File P@K | File R@K | File MRR | Symbol P@K | Symbol R@K | Symbol MRR | Relation recall | Impact recall | Impact FP rate | Freshness |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Development | 7 | 0.494 | 1.000 | 0.929 | 0.333 | 0.952 | 0.640 | 1.000 | 0.800 | 0.268 | 7/7 |
| Holdout | 2 | 0.236 | 1.000 | 1.000 | 0.225 | 1.000 | 1.000 | 1.000 | — | — | 2/2 |

Baseline before fixes (same generation discipline, K=10/3200 in the first run;
not directly comparable to the final K=15/6000 configuration): relation recall
0.71 development, symbol recall 0.62, impact recall 0.0. The final run fixed the
route/handler/interface chain and test traversal.

Confirmed failures / limits:

- `T06` (`domain/task.go` impact) misses `repository/task_repository.go` because
  Go structs/interfaces are not graph nodes; type-annotation-only impact is
  under-detected. One unsupported caller (`api/route/route.go`) is reported.
- At the CLI default 3200 tokens, the three-hop `POST /task` chain is truncated;
  the target and one neighbour survive. At 6000 the full chain fits.
- Precision is intentionally strict: supporting imports/usages/tests/mocks that
  are relevant but not in the minimal expected set count as non-expected.
- The dataset covers one Go repository; it is a baseline, not a universal
  threshold.

What the numbers do **not** prove: that GRY is semantically correct on arbitrary
repositories, that precision is high in an absolute sense, or that holdout
performance generalizes beyond this fixture.

Evidence quality: 100% of returned relationships had a known provenance label;
the verified share was 0.866 (development) and 0.923 (holdout). The remainder
were explicitly inferred/declared/observed.

Artifacts: `eval/results/2026-10-27-semantic.json`, `eval/tasks/goca-v1.json`.

---

## E. Performance results

Machine: macOS (`darwin`), Apple Silicon, 16 GiB, Rust 1.99.1, Go 1.27.2.
All latency is external CLI wall time (process startup included). Synthetic and
real repositories are separate.

### Index

| Dataset | Kind | Files | Nodes | Edges | Index ms | Peak RSS |
|---|---|---:|---:|---:|---:|---:|
| goca | real | 57 | 422 | 958 | 707 | 105.7 MB |
| gry | real | 95 | 3,817 | 17,898 | 1,177 | 192.4 MB |
| medium | synthetic | 2,002 | 10,047 | 48,393 | 1,514 | 327.9 MB |
| large | synthetic | 20,002 | 80,207 | 160,207 | 8,703 | 1,327.9 MB |
| hub | synthetic | 1,502 | 3,023 | 6,023 | 965 | 136.4 MB |
| deep | synthetic | 1,501 | 3,006 | 6,004 | 995 | 145.8 MB |
| dense | synthetic | 2 | 157 | 3,503 | 705 | 107.2 MB |
| disconnected | synthetic | 801 | 1,706 | 3,205 | 866 | 122.4 MB |

### Context (`--depth 2 --max-tokens 3200`)

| Dataset | Samples | p50 | p95 | Peak RSS | Tokens | Correctness |
|---|---:|---:|---:|---:|---:|---|
| goca | 20 | 10.2 ms | 11.2 ms | 12.1 MB | 2,858 | valid JSON, budget respected |
| gry | 20 | 13.3 ms | 20.6 ms | 14.7 MB | 2,854 | valid JSON, fresh/stale reported |
| medium 2k | 20 | 23.1 ms | 25.9 ms | 29.8 MB | 2,822 | valid JSON, budget respected |
| large 20k | 10 | 80.6 ms | 84.9 ms | 107.8 MB | 2,883 | valid JSON, budget respected |
| hub | 20 | 15.2 ms | 16.4 ms | 16.7 MB | 2,932 | bounded traversal |
| deep | 20 | 12.3 ms | 13.7 ms | 12.7 MB | 917 | bounded traversal |
| dense | 20 | 13.7 ms | 14.3 ms | 13.6 MB | 2,919 | bounded expansion |
| disconnected | 20 | 9.6 ms | 10.5 ms | 11.8 MB | 974 | bounded traversal |

P99: **not claimed**. The largest context sample set is 20; a p99 from 10–20
samples is not a reliable tail guarantee.

### Hooks on goca (10 samples)

| Stage | No-cache p50/p95 | Cached p50/p95 |
|---|---:|---:|
| before_plan | 15.3 / 16.6 ms | 9.6 / 16.1 ms |
| before_edit | 10.2 / 11.2 ms | 9.6 / 11.0 ms |
| after_edit | 14.3 / 14.6 ms | 9.8 / 14.7 ms |
| before_finish | 14.3 / 16.2 ms | 10.1 / 14.5 ms |

`gry --version` p50 6.6 ms / p95 7.0 ms (30 samples). A 120-call hook session
had first-20 9.7 ms, last-20 9.6 ms (−1.0% drift) and 11.5 MB peak RSS. No
persistent daemon is justified by these measurements.

### Incremental reindex

| Dataset | Operation | Parsed | Deleted | ms | Peak RSS |
|---|---|---:|---:|---:|---:|
| medium | unchanged | 0 | 0 | 206 | 21.5 MB |
| medium | edit one | 1 | 0 | 102 | 25.1 MB |
| medium | add one | 1 | 0 | 749 | 125.4 MB |
| medium | delete one | 0 | 1 | 141 | 44.1 MB |
| large | unchanged | 0 | 0 | 789 | 58.8 MB |
| large | edit one | 1 | 0 | 712 | 64.0 MB |
| large | add one | 1 | 0 | 1,481 | 289.0 MB |
| large | delete one | 0 | 1 | 941 | 219.6 MB |

Correctness under load: output budgets held; dense/hub traversal stayed bounded;
deep/disconnected graphs terminated; incremental add/delete reconciled orphan
nodes (existing `delta_publish_removes_nodes_for_files_no_longer_present` plus
the new CLI recovery test).

Artifact: `eval/results/2026-10-27-context-hooks.json`.

---

## F. Hook and OHGO integration

### Lifecycle exercised

A temporary copy of the fixture was indexed as `goca-e2e4` and OHGO was asked to
add one comment above `taskRepository.Create` under `--workspace build -y`.

Observed real order:

```text
gry before_plan:   status=ok     freshness=unverified complete_for_scope=false
gry before_edit:   status=ok     freshness=unverified complete_for_scope=false
gry after_edit:    status=stale  freshness=stale      complete_for_scope=false
gry before_finish: status=stale  freshness=stale      complete_for_scope=false
```

- `before_plan` ran once before the first model request and its evidence was
  injected as a system block.
- `before_edit` ran once for the edit target.
- `after_edit` ran once after the successful mutation and appended bounded
  evidence to the edit result.
- `before_finish` ran after the run and correctly refused to claim completeness
  because the edit was not reindexed.
- A repeated `before_edit` for the same identity was served from the session memo
  (`cached=true`) without a second process. A different file or session is not
  suppressed.
- Diff: exactly one added comment.
- `go test ./...` in the fixture copy: pass.
- OHGO outcome: `NEEDS_REVIEW` because the model did not run verification inside
  the session. That is OHGO's completion policy, not a hook failure. The test run
  was performed separately and passed.

### Failure contract

`disabled` (no process), `unavailable`, `timeout` (retryable), `cancelled`,
`invalid_output`, `not_indexed`, `output_too_large`, and stale/partial evidence
are all distinct. A failed hook never becomes an empty successful context. No
hook path can block, approve, grant or execute a mutation; OHGO's `Authorize`,
approval, grounding and budget remain the only authority path.

### Security

- No shell; `exec.CommandContext` with an argument vector.
- Child env limited to `PATH`, `HOME`, `TMPDIR`, locale, `GRY_DB`,
  `GRY_CACHE_DIR`.
- Project name validated.
- 128 KiB stdout / 4 KiB stderr cap.
- Secret-bearing environment values redacted in output.
- Evidence text contains names/paths/statuses only; no source bodies or prompts.
- Unit tests cover redaction and malformed/oversized output. The fixture context
  did not expose `DB_PASS`, `ACCESS_TOKEN_SECRET` or values.

### Scope

- Wired for headless `ohgo run` only. The TUI/REPL path is not wired; this is an
  explicit limitation.
- The GRY project must be indexed and named via `OHGO_GRY_PROJECT`.
- The installed `~/.local/bin/gry` / `ohgo` were not replaced; the integration
  was tested with built binaries.

---

## G. Verification evidence

### Baseline (before changes)

| Repository | Command | Result |
|---|---|---|
| GRY | `cargo test --workspace --locked` | 186 passed, 0 failed |
| OHGO | `go build ./...`; `go test ./...` | build pass; all packages pass |
| Fixture | `go test ./...` | pass |

### Final

| Repository | Command | Result |
|---|---|---|
| GRY | `cargo fmt --all -- --check` | pass |
| GRY | `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| GRY | `cargo test --workspace --locked` | **200 passed, 0 failed** |
| GRY | `cargo build --release --locked --bin gry` | pass |
| GRY | `make smoke` | `smoke: PASS` |
| GRY | `python3 eval/run_context_eval.py ...` | 9 tasks, 0 errors; metrics above |
| GRY | `python3 eval/benchmark.py ...` | saved `eval/results/2026-10-27-context-hooks.json` |
| OHGO | `gofmt -l .` | empty |
| OHGO | `go vet ./...` | pass |
| OHGO | `go build ./...` | pass |
| OHGO | `go test ./...` | 38 packages ok, 0 failed |
| OHGO | `GRY_BIN=... GRY_CACHE=... GRY_PROJECT=goca go test ./internal/gryhook -run TestGryHookLive` | pass |
| Fixture | `go vet ./...`; `go test ./...` | pass |
| Fixture | `git status --short --branch` | `## main...origin/main`, clean |

Pre-existing failures: none observed. The only interruption was the accidental
fixture `.git` reinitialization described in section B; the source tree was
unchanged and the original git state was restored.

---

## H. Remaining limitations

### P0 — correctness/security blockers

- None known in the documented headless path. The repository-wide freshness
  guarantee is deliberately **not** made; `repository_complete=false` is the
  contract, not a bug.

### P1 — major quality/performance risks

1. **Go type declarations are not graph nodes.** `domain.Task`,
   `domain.TaskRepository` and `domain.TaskUsecase` exist only as fields/uses, so
   type-annotation-only impact is under-detected (`T06` repository miss).
   Next action: add Go struct/interface type nodes and type-reference edges, then
   add an impact regression for a domain type change.
2. **Semantic precision is modest** (file 0.49, symbol 0.33 on development).
   Supporting evidence is returned intentionally; review whether callers want a
   stricter mode. Next action: label expected-vs-supporting evidence and add a
   precision-oriented profile, without changing default recall.
3. **20k-file ceiling.** 100k files are untested; index peak at 20k is 1.33 GB.
   Next action: measure 100k in a controlled run and set a hard operational
   limit if RSS is unacceptable.
4. **3200-token default truncates a three-hop Go chain.** The full chain needs
   ~6000 tokens. Next action: document/raise the hook/Scout default or add a
   compact route-chain summary.
5. **P99 not measured.** Next action: 200+ samples per dataset before claiming
   tail latency.

### P2 — non-blocking improvements

1. OHGO TUI/REPL lifecycle is not wired; only `ohgo run` is integrated.
2. Freshness scanning is non-recursive and bounded; a new file elsewhere in the
   repository is outside the claim.
3. Concurrent-index stress is deterministic unit-level, not a multi-process
   fuzz run.
4. `check_method` trusts unchanged mtime+size without re-hashing.
5. The adjacent-directory scan does not re-apply `.gitignore`/`.gryignore`; a
   newly added ignored source file can be reported as `added_paths` (conservative
   stale, not complete, but noisy).
5. The fixture's git history was restored; no source changes remain, but the
   original `.git` should be re-verified before any future evaluation run.

---

## I. Git status

### GRY

- Branch: `feature/hooks`
- Pre-existing changes: none (clean at baseline).
- Files changed by this task: 10 tracked files; 1 new `eval/` tree.
- Untracked: `eval/` (task artifact, intentional).
- Commits/tags/pushes: **none**.

### OHGO

- Branch: `fix/mcp`
- Pre-existing changes: none (clean at baseline).
- Files changed: `agent/loop.go`, `cmd/ohgo/run.go`.
- Untracked: `cmd/ohgo/gryhooks.go`, `docs/gry-hooks.md`,
  `internal/gryhook/` (intentional).
- Commits/tags/pushes: **none**.

### Go fixture

- Branch: `main`, tracking `origin/main`
- Pre-existing changes: none.
- Files changed by this task: **none**.
- Untracked: none.
- The accidental initial commit was removed by restoring the original `.git`
  from upstream `8a8bf00`; the working tree was verified byte-identical.
- Commits/tags/pushes: **none remaining**.

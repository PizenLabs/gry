# Multi-agent semantic hooks and installer hardening — 2026-10-27

> **Status:** vertical slice implemented; no commits, tags, pushes, or releases
> were created.
> **Repositories:** GRY `feature/reconnaissance` @ `070e0a6`, OHGO
> `feature/agent` @ `7cf493f`.
> **Starting state:** both working trees were clean before this task.

---

## 1. Executive summary

Implemented and verified:

- GRY installer/CLI now emits the **OpenCode V2** MCP shape
  (`mcp.servers.gry`) instead of the deprecated V1 `mcp.gry`, migrates a
  gry-shaped V1 entry, and refuses to overwrite or delete foreign entries.
- An opt-in OpenCode V2 native lifecycle plugin covers `before_plan`,
  `before_edit`, and `after_edit`. OpenCode V2 exposes **no finish hook**, so
  `before_finish` is explicitly unavailable rather than mapped to an unrelated
  event.
- `gry install --client opencode --hooks` installs the plugin with the absolute
  gry binary path; `gry integrations --verify` reports ownership and local
  modification; `gry uninstall` removes only files/entries that still match the
  gry-owned manifest and shapes.
- `gry hook --project auto` resolves the indexed project whose root contains the
  working directory and fails closed when none does.
- The GRY installer gained `--hooks` and `--status`, archive path-traversal
  rejection, symlink rejection, and deterministic fixture tests.
- OHGO's existing opt-in headless GRY adapter is preserved and now also reads a
  persistent `gry` section from `~/.ohgo/settings.json`.
- OHGO gained `ohgo gry enable|status|disable`, which preserves unrelated and
  unknown settings keys with an atomic raw-JSON edit.
- The OHGO installer now verifies `sha256sums.txt` from the same release, rejects
  unsafe archive paths, installs atomically, supports `--dry-run`,
  `--uninstall`, `--gry-project`, and `--version dev`, and is covered by
  deterministic fixture tests.

Verification level, stated precisely:

| Area | Status |
|---|---|
| GRY MCP + V2 config | Real OpenCode 2.1.4 connected to `gry serve` and reported **18 tools** |
| OpenCode plugin loading | Real OpenCode 2.1.4 loaded `plugins/gry.js` with no load failure |
| OpenCode hook callbacks | Verified with realistic events and a fake gry binary (subprocess and stage order), **not** with a live model session |
| OHGO adapter | Existing headless integration and tests preserved; new settings path unit-tested, not re-run against a live model in this task |
| OHGO TUI | Still not wired; `ohgo gry status` reports `tui_wired: false` |
| Other runtimes | MCP only; no native adapter claimed |

Not implemented / deferred:

- `before_finish` for OpenCode (no such hook exists).
- Native hooks for any runtime other than OpenCode and OHGO.
- Detecting edits performed through OpenCode's `shell` tool as `before_edit`/
  `after_edit` events (no reliable target path).
- TUI/REPL GRY lifecycle wiring in OHGO.
- Signature verification (the release pipeline publishes SHA-256 checksums, not
  signatures).

---

## 2. Reconnaissance findings

### 2.1 GRY distribution

- Canonical installer:
  `https://github.com/PizenLabs/gry/releases/latest/download/install.sh`.
- Release assets are `gry-<target>.tar.gz` plus `SHA256SUMS`; the installer
  already verified SHA-256 but did not validate archive member paths.
- Targets in the installer: `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `x86_64-unknown-linux-musl`/`gnu`, `aarch64-unknown-linux-musl`/`gnu`, with a
  source-build fallback. Windows is explicitly unsupported.
- Existing `gry install` supported 18 MCP clients with per-client JSON/TOML
  writers. OpenCode was written as V1 `mcp.gry` with `enabled: true`.
- Existing tests covered install/uninstall of several clients but not malformed
  structural values, foreign-entry conflicts, OpenCode V2, or hook assets.

### 2.2 OpenCode (verified against 2.1.4 and the V2 docs)

- MCP: `mcp.servers.<name>`; local servers use
  `{"type":"local","command":[...]}`. `disabled`, not `enabled`, disables a
  server. V1 `mcp.<name>` is deprecated.
- Native lifecycle: V2 plugins. Relevant hooks:
  - `session.prompt` (prompt admission),
  - `session.context` (before model requests, mutable `system` array),
  - `tool.execute.before` / `tool.execute.after` (completed/failed).
- The loaded tool IDs in 2.1.4 include `edit`, `write`, and `patch`; there is
  no finish hook.
- Local plugins are discovered from `~/.config/opencode/plugins/` and
  `.opencode/plugins/`. A plugin that exports `{id, setup}` loads without an npm
  dependency, which keeps the adapter dependency-free.
- `opencode mcp list` printed “No MCP servers configured” even while the server
  log showed `mcp connected server=gry tools=18`; the log is the reliable
  evidence and is what the test asserts.

### 2.3 OHGO distribution and integration

- `install.sh` selected `ohgo-<tag>-<os>-<arch>.tar.gz` but downloaded no
  checksum, although `make release` publishes `sha256sums.txt`.
- The existing `internal/gryhook` adapter already invoked
  `gry hook <stage> --project ... --json --no-cache` with argv-only execution,
  a filtered environment, bounded output, timeouts, memoization, and redaction.
- Only headless `ohgo run` was wired; the TUI was not. That remains true.
- `.ohgo/settings.json` is the machine-scoped settings file. `config.Load`
  ignores unknown keys and `config.Save` reserializes the typed struct, so a
  naive `gry enable` would have dropped unknown keys; the new command edits raw
  JSON instead.

### 2.4 Other runtimes

No other runtime's official API was verified in this task. They remain MCP-only
entries, and the documentation says so. No placeholder hook adapters were added.

---

## 3. Architecture

### 3.1 Responsibilities

- **GRY** remains a read-only evidence provider: indexing, graph queries, hook
  envelopes, freshness/provenance, MCP tools.
- **OHGO** retains lifecycle, planning, tool flow, authorization, approvals,
  mutation policy, budgets, cancellation, and final status.
- **Installers** configure only the surfaces they own and record what they
  wrote. There is no shared universal IR or shared config writer.

### 3.2 Data flow

**OpenCode native path**

```text
session.prompt ── task text ──> plugin state
session.context ── first model request ──> gry hook before_plan
                                    └─> <gry_evidence> system block
tool.execute.before (edit/write/patch) ──> gry hook before_edit
tool.execute.after (completed)         ──> gry hook after_edit
                                    └─> queued for the next session.context
before_finish                          ──> not available
```

**OpenCode MCP path**

```text
mcp.servers.gry -> gry serve (stdio) -> 18 existing read/mutate-classified tools
```

**OHGO path (unchanged contract)**

```text
before_plan -> one system block
before_edit -> observation only, never a permission gate
after_edit  -> bounded evidence appended to the next model result
before_finish -> bounded status line; OHGO's completion policy still decides
```

### 3.3 Failure behavior

- Hook process failure, timeout, invalid JSON, oversized output, and unknown
  status are explicit failures. They are never converted into empty success.
- OpenCode has no finish event, so the adapter cannot convert a successful
  process exit into a completion claim.
- The OpenCode plugin caches a failed event for 30 seconds to avoid a retry
  storm; the failure is still rendered.
- OHGO's existing failure semantics are unchanged.

### 3.4 Adapter boundaries

| File | Boundary |
|---|---|
| `crates/gry-cli/assets/opencode-gry.js` | OpenCode events → GRY hook CLI; no OpenCode permissions logic |
| `crates/gry-cli/src/main.rs` (`install_hooks`, manifest) | OpenCode plugin/config ownership |
| `internal/gryhook` (OHGO) | GRY CLI → OHGO evidence; no authority logic |
| `cmd/ohgo/gry.go` | OHGO settings selection only |

---

## 4. Runtime matrix

| Runtime / version | MCP | Native events | MCP verification | Native-hook verification | Status |
|---|---|---|---|---|---|
| OpenCode 2.1.4 | `mcp.servers.gry`, local stdio | `before_plan` (`session.context`), `before_edit` (`tool.execute.before`), `after_edit` (`tool.execute.after`) | Real runtime; log `mcp connected server=gry tools=18` | Real plugin load; deterministic event suite; **no live model loop** | Implemented, partially verified |
| OHGO (headless) | `mcp.servers[]` | `before_plan`, `before_edit`, `after_edit`, `before_finish` | Existing OHGO MCP tests + prior live test | OHGO loop test with a real write tool and fake GRY; prior controlled E2E | Preserved |
| OHGO TUI | MCP only | none claimed | — | — | Deferred |
| All other MCP clients | Per-client config | none | Existing tests | — | MCP only |

Minimum version note: the OpenCode adapter targets the V2 plugin API and
`mcp.servers`; it was verified on 2.1.4. An older V1-only OpenCode is not
claimed.

---

## 5. Installer changes

### 5.1 GRY

New/changed CLI surface:

```text
gry install --client opencode --hooks [--force] [--dry-run]
gry integrations [--verify]
gry uninstall --client opencode
gry hook <stage> --project auto
```

- OpenCode config path: `$XDG_CONFIG_HOME/opencode/opencode.json` (default
  `~/.config/opencode/opencode.json`).
- Plugin path: `$XDG_CONFIG_HOME/opencode/plugins/gry.js`.
- Ownership manifest: `$XDG_CONFIG_HOME/gry/integrations.json`.
- The plugin has the absolute gry path substituted at install time.
- Reinstall is convergent: identical plugin content is not rewritten; MCP
  entries are replaced in place; unrelated keys and servers are preserved.
- A V1 `mcp.gry` entry is migrated only when its shape matches gry's writer.
- A malformed `mcp`, non-object `mcp.servers`, non-array OHGO `mcp.servers`, or
  invalid Codex TOML is refused without writing.
- Uninstall refuses a `gry` entry that no longer has the gry shape (foreign or
  user-edited) and reports it instead of deleting it.
- The shell installer gained `--hooks`, `--status`, archive traversal rejection,
  symlink rejection for the extracted binary, and the same checksum behavior.
- Shell installer tests use a local fake release (`file://`) with a fake gry
  binary; no network or real user config.

Backups: JSON/TOML files modified by `gry install` are copied to
`<name>.gry-bak` before the atomic write. Managed plugin files are hashed in the
manifest; temporary files are mode `0600` and renamed atomically.

### 5.2 OHGO

New CLI surface:

```text
ohgo gry enable --project NAME [--binary PATH] [--cache-dir DIR]
                 [--timeout-ms N] [--max-tokens N] [--debug]
ohgo gry status
ohgo gry disable
```

Installer:

```text
sh install.sh --version TAG [--prefix DIR] [--dry-run]
              [--gry-project NAME] [--gry-binary PATH]
sh install.sh --uninstall
OHGO_VERSION=dev sh install.sh
```

- Release installs fetch `sha256sums.txt` from the same tag and fail before
  extraction on a mismatch or missing checksum entry.
- `--insecure`/skip-checksum is deliberately not provided.
- Archive paths are checked for absolute/`..` members before extraction.
- The binary is copied to a same-directory temp file, chmod 0755, then renamed.
- A matching installed version is reused instead of downloading again.
- `--uninstall` removes only `$PREFIX/bin/ohgo`; `~/.ohgo` is untouched.
- `--gry-project` is opt-in and calls `ohgo gry enable`; failure is reported as
  a partial outcome after the binary is installed.
- `gry enable/disable` edit the settings document as raw JSON, preserving
  unknown keys, and write atomically.

Runtime configuration precedence: file < `OHGO_GRY_*` environment.
`OHGO_GRY_HOOK=0` disables an enabled file entry for one process.

---

## 6. Security analysis

### Threat surface and mitigations

| Surface | Mitigation |
|---|---|
| Release archive | SHA-256 from the same release; traversal/symlink rejection; no TLS bypass; no invented hashes |
| Installer paths | Quoted variables; `mktemp -d`; atomic temp files; spaces/Unicode tested |
| Hook argv | `spawn`/`exec.CommandContext` argv arrays; no shell; task text and paths are arguments |
| Child environment | OpenCode plugin keeps `PATH`, `HOME`, `TMPDIR`, locale, `GRY_DB`, `GRY_CACHE_DIR`; OHGO keeps the existing set |
| Output size | 128 KiB stdout / 4 KiB stderr; plugin timeout and OHGO timeout |
| Secrets | OHGO redaction retained; plugin never logs body/prompts/env values |
| Paths | Project-relative conversion; absolute escapes, `..`, NUL, and oversized paths rejected |
| Evidence text | Repository content is untrusted; `<` is escaped so it cannot close the evidence tag; truncation keeps whole JSON envelopes and marks omission |
| Config writes | Atomic, mode 0600 temp, symlink target refused, manifest hash for hook ownership |
| Authority | GRY output cannot grant permission; no approval/authorization/budget writes in either adapter |

### Residual risks

- The release system provides checksums but **not signatures**. Verification is
  only as strong as TLS plus the release page.
- The ownership manifest is local metadata, not a security boundary; a local
  user can edit both the manifest and files.
- `--force` intentionally overwrites a managed plugin that the user edited.
- The OpenCode plugin runs with the OpenCode server's privileges. It is trusted
  local code; its safety relies on the fixed argv and bounded output.
- OpenCode's `shell` tool can mutate files without an edit event; evidence may go
  stale until the next indexed edit or reindex.

---

## 7. Compatibility analysis

Preserved:

- All existing GRY CLI commands and flags; `gry install` defaults unchanged.
- All MCP payload fields and freshness/completeness semantics.
- Existing OHGO env-based adapter behavior and failure table.
- OHGO approval, grounding, budget, cancellation, and completion policies.
- Existing installer commands/env vars; OHGO `OHGO_VERSION` still works.
- Unrelated and unknown JSON/TOML settings.
- No semantic engine source was changed.

Intentional behavior changes:

- OpenCode MCP is written in V2 form. Existing gry-shaped V1 entries are
  migrated on install.
- GRY uninstall now refuses to delete a `gry` entry that does not match the gry
  shape. Previously it deleted by key. This is safer but can require manual
  removal of a user-owned entry.
- OHGO's settings command writes the raw JSON document (preserving unknown keys)
  rather than resaving the typed struct.

No migration is required for users who never opted into OpenCode hooks. There is
no automatic downgrade or reset.

---

## 8. Tests and quality gates

All commands below were executed; exit status was checked.

### GRY

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | **210 passed, 0 failed** across 11 test binaries |
| `cargo build --release --locked --bin gry` | pass |
| `make smoke` | `smoke: PASS` |
| `cargo package --workspace --locked --no-verify --allow-dirty` | pass (plain `--no-verify` refuses a dirty working tree; `--allow-dirty` is required because no commit was allowed) |
| `bash scripts/install_test.sh` | 9 passed, 0 failed |
| `node --test crates/gry-cli/tests/opencode_plugin_test.mjs` | 10 passed, 0 failed |
| `GRY_OPENCODE_LIVE=1 cargo test -p gry --test opencode_runtime` | 3 passed, 0 failed |
| Semantic evaluation | development file P/R/MRR `0.494 / 1.000 / 0.929`, holdout `0.236 / 1.000 / 1.000`; relation recall 1.0; freshness 7/7 and 2/2 — identical to the documented baseline |

New GRY coverage includes: V1→V2 migration, idempotent reinstall, unrelated MCP
server preservation, foreign-entry uninstall refusal, plugin conflict and
`--force`, manifest/status hash verification, malformed JSON/TOML refusal,
`--project auto` inside and outside an indexed root, hook path translation,
dedupe, timeout, malformed output, oversized evidence parseability, tag
injection, and real runtime plugin/MCP loading.

### OHGO

| Command | Result |
|---|---|
| `gofmt -l .` | empty |
| `go vet ./...` | pass |
| `go build ./...` | pass |
| `go test ./...` | all packages pass (including `cmd/ohgo`, `internal/gryhook`, `config`) |
| `golangci-lint run ./...` | 0 issues |
| `bash scripts/install_test.sh` | 10 passed, 0 failed |
| `bash -n install.sh` | pass |

New OHGO coverage includes: file/environment precedence, `OHGO_GRY_HOOK=0`,
invalid budgets, project validation, `gry enable/status/disable`, preservation
of unrelated and unknown settings keys, refusal without writing, installer
fresh/repeat/dry-run/checksum/traversal/Unicode/dev/uninstall/unsupported-arch
paths.

No baseline failures were observed. `cargo package` is the only command that
required an extra flag because the task forbade commits.

---

## 9. Performance

Measured with the release binary on 2026-10-27:

- Fixture: `/Users/anvndev/Documents/Project/Benchmarks/go-backend-clean-architecture`
  (57 files, 422 nodes, 954 edges).
- Command: `gry hook before_plan --project goca --json --no-cache --max-tokens 6000 --task "..."`
- Samples: 5 after one warm-up; wall-clock measured around the process.
- Mean **15.86 ms**, min 15.62 ms, max 16.17 ms.
- Result: `status=ok`, `complete_for_scope=false` (bounded, as expected).

Overhead controls:

- OpenCode `before_plan` runs once per prompt/session, not on every model turn.
- Identical edit events are memoized per session/stage/task/paths.
- Failed events are suppressed for 30 seconds per session.
- Injection is bounded by `--max-tokens` and a 12,000-character presentation cap
  that keeps whole JSON envelopes.
- OHGO retains its existing per-session memo, timeout, and 128 KiB output bound.

Installer fixture suites take ~1.4 s (GRY, 9 cases) and ~1.6 s (OHGO, 10 cases)
on this machine; they use no network.

No 100k-file performance claim is made. The previously documented measured
ceiling remains 20k files.

---

## 10. Changed files

### GRY

Modified:

- `.github/workflows/ci.yml` — runs the installer fixture suite.
- `Cargo.lock` — gry-cli now uses the already-present workspace `sha2`, `hex`, `toml` crates.
- `Makefile` — `test-install`, `test-opencode`.
- `README.md` — opt-in hooks/status examples, link to the integration guide.
- `crates/gry-cli/Cargo.toml` — `sha2`, `hex`, `toml`.
- `crates/gry-cli/src/main.rs` — OpenCode V2 config/migration, hook install/manifest,
  `gry integrations`, safe uninstall shapes, `--project auto`, atomic writes,
  malformed-config guards, TOML validation.
- `crates/gry-cli/tests/cli.rs` — 6 new/updated integration tests.
- `scripts/install.sh` — `--hooks`, `--status`, traversal/symlink checks.

Added:

- `crates/gry-cli/assets/opencode-gry.js`
- `crates/gry-cli/tests/opencode_plugin_test.mjs`
- `crates/gry-cli/tests/opencode_runtime.rs`
- `docs/INTEGRATIONS.md`
- `scripts/install_test.sh`
- `docs/reports/2026-10-27-multi-agent-integration.md` (this report)

### OHGO

Modified:

- `.github/workflows/ci.yml` — runs the installer fixture suite.
- `Makefile` — `test-install`.
- `README.md` — checksum/flags/opt-in documentation.
- `cmd/ohgo/gryhooks.go` — settings-aware adapter resolution.
- `cmd/ohgo/main.go` — registers `ohgo gry`.
- `cmd/ohgo/run.go` — passes resolved settings to the adapter.
- `config/config.go` — `Gry *GrySettings`.
- `docs/gry-hooks.md` — persistent configuration, commands, tests.
- `install.sh` — checksum verification, flags, atomic install, traversal guard,
  dry-run/uninstall/dev/GRY opt-in.
- `internal/gryhook/gryhook.go` — `FileConfig`, `FromEnvAndSettings`,
  `ValidProject`; env precedence.

Added:

- `cmd/ohgo/gry.go`
- `cmd/ohgo/gry_test.go`
- `internal/gryhook/settings_test.go`
- `scripts/install_test.sh`

---

## 11. Git state

- GRY: branch `feature/reconnaissance`, upstream unchanged, working tree contains
  only the files listed above. No commit, tag, push, or release.
- OHGO: branch `feature/agent`, upstream unchanged, working tree contains only
  the files listed above. No commit, tag, push, or release.
- No unrelated user work was present at start, and none was discarded.
- The independent `go-backend-clean-architecture` fixture was used read-only for
  the semantic evaluation. It was indexed into a temporary cache; no source
  change was made.

---

## 12. Remaining risks and next steps

### P0

None identified in the tested paths.

### P1

1. **OpenCode hooks have not been exercised by a live model session.** The
   plugin loads in the real runtime and its callbacks are covered with realistic
   events, but a run with an actual model, `edit` tool call, and subsequent
   `session.context` would prove end-to-end ordering. Add an opt-in E2E with a
   local fake OpenAI-compatible endpoint or a recorded session.
2. **No `before_finish` in OpenCode.** Completion-time evidence remains an OHGO
   capability. Documented; do not add a fake mapping.
3. **Shell-based edits are invisible to the OpenCode adapter.** `edit`, `write`,
   and `patch` are covered; `shell` mutations cannot be attributed to a path.
4. **OHGO TUI remains unwired**, so the four-stage lifecycle is headless-only.

### P2

5. The ownership manifest and plugin hash are local integrity metadata, not
   signatures. Release checksums are also unsigned.
6. Live-release installer behavior is covered by fixtures only; a release smoke
   test against the actual GitHub release would catch asset-layout drift.
7. Other runtimes are MCP-only. Add adapters only when their official hook APIs
   are verified and the adapter can be tested without faking a runtime.
8. Windows remains unsupported by both installers.

### Recommended next slice

Add a documented fake-model OpenCode E2E that drives one `edit` tool call and
asserts `before_plan → before_edit → after_edit` delivery in `session.context`,
then decide whether a lightweight `session.idle` advisory is semantically
defensible as a *non-blocking* finish diagnostic. Do not present it as
`before_finish` unless OpenCode documents a finish hook.

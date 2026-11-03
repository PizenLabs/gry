# Changelog

## Unreleased

### Added

- `gry update` now installs the latest stable GitHub release instead of printing
  manual instructions. It resolves the latest tag from the public release
  redirect (with the GitHub API as a fallback), verifies the release archive
  against `SHA256SUMS`, extracts only the `gry` binary, and replaces the running
  executable with an atomic rename on Unix (rename-aside on Windows). `--check`,
  `--version`, `--dry-run`, `--force` and `--insecure` are supported.
  Homebrew, Nix, Scoop and Chocolatey installs are refused by default so the
  package manager stays in control. No update service, account, or telemetry is
  involved; downloads use the system `curl`/`wget`/PowerShell.

## 1.0.4 — 2026-11-03

### Fixed

- Source-reading tools no longer report a missing, moved or unreadable project root as a verified
  empty result. `search_code` errors when the root is unavailable and reports unreadable files plus
  a `complete` flag; full `get_code_snippet` errors when the source or recorded range is
  unavailable. Outline mode remains graph-only.
- `check_index_coverage` no longer reports a recorded-but-deleted file as `indexed/fresh`. It
  distinguishes `unavailable` (missing root), `missing_on_disk`, `stale`, `indexed`, `skipped` and
  `missing`; `index_status` exposes `root_available`, `source_available` and a warning.
- Incremental discovery always recomputes SHA-256 from the bytes it already reads. A same-size edit
  with a preserved timestamp can no longer be hidden by mtime/size reuse.
- Extractor-version gating is per project (`extractor_version:<project>`). A project indexed by an
  older extractor can no longer take the delta path after another project is rebuilt. Existing
  databases rebuild each project once; no schema migration is required.
- `search_graph` with a BM25 `query` now rejects structural-only filters (`name_pattern`,
  `qn_pattern`, `relationship`, degree, entry-point, `include_connected`) instead of silently
  ignoring them.
- `search_code` accepts `query` as a documented alias for `pattern`, and reports the true per-file
  `match_count` alongside the returned page size.
- MCP project resolution now handles descendant directories, ignores a deleted remembered project,
  rejects an ambiguous shared root, and names indexed projects plus recovery steps when no project
  matches.
- `delete_project`, `manage_adr` and `ingest_traces` require an explicit project instead of
  defaulting a write or destructive operation to the workspace project.
- Ambiguous `get_code_snippet` names return candidate qualified names instead of “symbol not found”.
- The project lock file is no longer unlinked on release, removing an inode race that could allow
  two concurrent indexes.

### Changed

- Removed a redundant pre-publication orphan scan. Delta publication already reconciles deleted and
  newly ignored files inside its transaction. On the 10k synthetic Go fixture this reduced
  unchanged re-index p50 from 349 ms to 229 ms and one-file edit p50 from 409 ms to 270 ms.
- Discovery always hashes content; README/ARCHITECTURE/CAPABILITIES now describe the exact behavior
  instead of the mtime/size SHA shortcut.
- CLI `gry search` accepts a structural-only query when `--name-pattern` or `--qn-pattern` is given.

### Added

- OMP (`omp`) as an install target. `gry install --client omp` and `--client auto`
  write `~/.omp/agent/mcp.json`, `--client all` includes it, and the agent skill
  is installed to `~/.omp/agent/skills/gry/SKILL.md` when OMP is present.
- Production-readiness audit, remediation, retrieval, performance and agent-interoperability
  reports under `docs/audit/`.

## 1.0.2 — 2026-10-26

### Fixed

- Publishing no longer stalls when a crate exists on crates.io but is missing
  from the sparse registry index. `scripts/publish.sh` now requires index
  visibility, names the dependency that is blocking a publish, and verifies
  that every crate in the workspace is resolvable once the run finishes.
- `gry-mcp` and `gry` were uploaded but never appeared in the registry index,
  which made `cargo publish -p gry` fail with `no matching package named
  'gry-mcp'`. The 1.0.2 release republishes both crates and confirms their
  index entries before finishing.

### Changed

- Internal workspace dependencies now require `1.0.2` instead of `1.0.0`, so a
  published crate cannot resolve to an older sibling from the same workspace.

## 1.0.1 — 2026-10-26

### Added

- crates.io publishing for all seven crates. The CLI package is published as
  `gry`, so `cargo install gry --locked` installs the binary. Release metadata
  (repository, homepage, readme, keywords, categories) is inherited from
  `[workspace.package]`, internal dependencies carry registry versions, and the
  tree-sitter grammar dependencies are pinned because crates.io rejects
  wildcard requirements. `make publish-dry-run` and `CONFIRM=yes make publish`
  drive `scripts/publish.sh`, and a tag-triggered `publish-crates` job uses the
  `CARGO_REGISTRY_TOKEN` secret.
- One-line installer (`scripts/install.sh`) published with each release:
  detects macOS/Linux architecture (including Rosetta) and musl/glibc, verifies
  SHA-256, installs the prebuilt binary, falls back to a Cargo source build,
  configures the shell PATH, and runs `gry install --client auto` plus the agent
  skill setup. Supports `--client`, `--prefix`, `--version`, `--from-source`,
  `--no-mcp`, `--no-skill`, `--index`, `--dry-run`, and `--uninstall`.
- Release workflow builds macOS arm64/x86_64 and Linux glibc/musl on x86_64 and
  aarch64, publishes tarballs, per-asset checksums, `SHA256SUMS`, and the
  installer.
- `gry install --client auto` and `--client all`, and the matching
  `gry uninstall` forms. Auto-detection checks client config files, harness home
  directories, macOS application bundles, and `PATH` executables, so the
  installer can configure only the agents actually in use.
- Platform-aware VS Code family paths: VS Code, Cline, Roo, Kilo, and Trae use
  `~/Library/Application Support` on macOS instead of the XDG path.
- Separate `claude-desktop` adapter using the Claude Desktop app config
  (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS,
  `~/.config/Claude/claude_desktop_config.json` on Linux). The existing `claude`
  adapter remains Claude Code (`~/.claude.json`).
- `gry install --skill` now also writes the skill to
  `~/.claude/skills/gry/SKILL.md` and
  `~/.config/opencode/skills/gry/SKILL.md` when those harness directories exist.
- `gry install --client ohgo` and matching uninstall: writes OHGO's
  `~/.ohgo/settings.json` `mcp.servers` entry with explicit read/mutate
  classifications, bounded response settings, and `GRY_CACHE_DIR`/`GRY_DB`
  environment passthrough. Existing OHGO settings and unrelated servers are
  preserved.
- MCP `tools/list` now paginates from the first page with `nextCursor` instead
  of returning every tool on cursorless calls and paginating only continuations.
  Spec-compliant clients see a bounded first page and follow cursors to the
  complete list; the advertised page size is the same whether or not the client
  sent a cursor.

### Fixed

- crates.io publishing no longer aborts midway when the registry index lags
  behind an upload. `scripts/publish.sh` publishes one crate per invocation,
  waits (up to `WAIT_TIMEOUT` seconds) only for the dependencies a failed
  publish actually needs, resumes interrupted runs by skipping versions that
  are already published, and treats a timed-out upload wait as success once the
  crate appears.

## 1.0.0 — 2026-10-26

First independent release of gry: local-first code intelligence for AI agents.

### Highlights

- Rust workspace (`gry-core`, `gry-store`, `gry-parse`, `gry-index`, `gry-query`, `gry-mcp`,
  `gry-cli`) with a single `gry` binary.
- 17 MCP tools, 2 prompts, `all`/`analysis`/`scout` profiles, JSON-RPC stdio, snapshot cursors and
  bounded output.
- Full and delta indexing: module-sibling and dependency-aware invalidation, nodes-only fast path,
  MessagePack+zstd parse cache, atomic publication.
- 68 statically linked tree-sitter grammars plus a conservative lexical fallback.
- Confidence-ranked call/type resolution with parameter/field/constructor/return-chain inference.
- Structural, BM25 and Nomic semantic search; snippets/outlines; trace; architecture; schema;
  compare; Cypher subset; change impact; conservative test impact; ADR; observed traces.
- Docs/config/infra extraction including PDF text layers, dbt lineage, Helm template/values,
  Kustomize overrides, Terraform defaults and Git-history coupling.
- CLI with first-class commands, one-shot `gry cli <tool>`, async jobs, watcher, doctor and safe
  install/uninstall for 15 clients.
- 118 tests across 19 suites, an optional differential check against the reference implementation,
  and measured benchmarks for 341 dense C files and 10,001 medium files.

### Independence and identity

- gry is now documented and licensed as an independent project (`PizenLabs`) under the
  Apache License 2.0; all organization-specific branding has been removed.
- Added community health files: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, and
  `NOTICE`.
- Repository configuration moved from the former organization-specific names to `.gry.json`,
  `.gry/` (graph exports) and `.gryignore` (discovery). Legacy environment aliases were removed so
  `GRY_*` is the only configuration namespace.
- Documentation was consolidated into a product set: `README.md`, `docs/ARCHITECTURE.md`,
  `docs/CAPABILITIES.md`, `docs/BENCHMARKS.md` and `docs/DEVELOPMENT.md`.

### Limits

The 1.0 release covers the full product scope. Known depth/breadth limits — syntactic type
resolution instead of Hybrid-LSP, 68 grammars plus fallback, 15 client adapters, bounded
Helm/Kustomize/Terraform evaluation, compact JSON output, and semantic edges refreshed on full index
rather than every delta — are recorded in [docs/CAPABILITIES.md](docs/CAPABILITIES.md#known-limits).

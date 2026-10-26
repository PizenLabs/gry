# Changelog

## [Unreleased]

### Added

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

- gry is now documented and licensed as an independent project (`The gry authors`) under the
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

# Capabilities and limits

This document is the coverage map for gry 1.0. It records what is implemented, the evidence behind
it, and the explicit depth or breadth limits of each partial area.

Status vocabulary:

- **VERIFIED** — implemented with acceptance evidence.
- **PARTIAL** — a documented subset with a named limit.
- **EXCLUDED** — deliberately outside the product scope.
- **BLOCKED** — an external blocker (none in 1.0).

Evidence: **U** unit tests, **I** integration tests, **M** MCP/CLI contract tests, **D**
differential check against the reference implementation, **B** benchmark. Commands and results are
in [DEVELOPMENT.md](DEVELOPMENT.md); measurements are in [BENCHMARKS.md](BENCHMARKS.md).

Current result: **60 VERIFIED, 37 PARTIAL, 0 BLOCKED**. Every PARTIAL entry is an accepted limit
with evidence; no required subsystem is silently absent.

## Input coverage

gry indexes code, documents, configuration, packages, infrastructure and runtime observations. The
full list of node labels, edge types and the language set is in the [README](../README.md#what-it-indexes).

- **68 statically linked tree-sitter grammars** plus a conservative lexical fallback for formats
  without a compatible modern grammar. Fallback nodes are explicitly marked.
- Source languages, Markdown/RST/AsciiDoc, PDF text layers, JSON/YAML/TOML/INI/.env, package
  manifests, Dockerfile/Compose, Kubernetes/Helm/Kustomize, Terraform/OpenTofu, Ansible, Make and CI
  workflows are first-class inputs.
- Files that are recognised but unparsed still become `File` nodes and remain searchable.

## Discovery and file lifecycle

| ID | Status | Evidence | Notes / limits |
|---|---|---|---|
| DISC-01 | VERIFIED | U (`discover::tests`), I (index fixture) | Built-in skips, root/nested `.gitignore`, `.gryignore`, global excludes, negation. `.gryignore` may re-include ordinary built-ins, never the safety core |
| DISC-02 | PARTIAL | U | Deterministic order, symlink skipping; `excluded` reports the first 25 sampled directories rather than every skipped subtree |
| DISC-03 | PARTIAL | U, D | 80+ mapped formats; 68 grammars; lexical fallback for grammar-less formats |
| DISC-04 | VERIFIED | U, I | Max files/source MiB fail the request before publication; oversized/read failures are reported |
| DISC-05 | VERIFIED | I, B | mtime/size + SHA reuse; delta rebuild; nodes-only fast path; deletes remove nodes/edges/cache; incremental graph compared with a clean rebuild |
| DISC-06 | VERIFIED | I (`artifact_round_trip`) | SQLite+zstd artifact, sidecar metadata, SHA check, import into a clean project |

## Parsing and language intelligence

| ID | Status | Evidence | Notes / limits |
|---|---|---|---|
| LANG-01 | VERIFIED | U (grammar registry, smoke parse) | 68 grammars compile and parse; malformed input records parse gaps |
| LANG-02 | PARTIAL | U (Rust/Python/JS/C), I | Functions/methods/classes/structs/interfaces/traits/enums/types/fields/variables/constants/macros; extraction depth varies by grammar |
| LANG-03 | VERIFIED | U (`fqn::tests`), D | Extension/`__init__`/`index`/Dockerfile variant/directory modules/file QNs |
| LANG-04 | PARTIAL | U, D | Relative/bare imports, Rust crate paths, manifests; some language-specific forms use generic extraction |
| LANG-05 | PARTIAL | U | Calls/receivers/args for major languages; Rust macro bodies are scanned lexically |
| LANG-06 | PARTIAL | U, I, D | Import/same-file/same-module/unique/suffix/receiver scoring with confidence; annotation, field, constructor and return-chain inference. No external LSP; uncertainty is explicit |
| LANG-07 | PARTIAL | I | INHERITS/IMPLEMENTS/DECORATES/OVERRIDE implemented; trait/generic/interface-default edge cases are incomplete |
| LANG-08 | PARTIAL | I, D | USAGE/CALL_REFERENCE/THROWS/RAISES/WRITES partly implemented; USAGE is broad while throws/writes are minimal |
| LANG-09 | VERIFIED | U (MinHash/profile), I | Deterministic MinHash, AST profile, complexity/importance; not a full language-specific complexity model |
| LANG-10 | PARTIAL | U | Syntactic/type inference instead of Hybrid-LSP; unresolved/inferred/ambiguous states are recorded |

## Graph and storage

| ID | Status | Evidence | Notes / limits |
|---|---|---|---|
| GRAPH-01 | VERIFIED | U, I, M | Schema, QN identity, edge dedup including `local_name`; extra docs/infra labels |
| GRAPH-02 | VERIFIED | I, M | create/list/delete/stats; delete cascades and cleans FTS |
| GRAPH-03 | VERIFIED | I | Single SQLite transaction; failure/cancel before commit leaves the prior generation |
| GRAPH-04 | VERIFIED | U, I | inbound/outbound/both BFS, depth 1–15, edge filters, test exclusion, limits |
| GRAPH-05 | PARTIAL | U, I | calls/data_flow/cross_service/topology/deploy/run modes; infrastructure depth depends on supported forms |
| GRAPH-06 | PARTIAL | I | overview/structure/dependencies/routes/languages/packages/entry_points/hotspots/boundaries/layers/file_tree/clusters/cycles; `system_structure` excluded |
| GRAPH-07 | VERIFIED | I, M | label/edge counts, relationship patterns, property diagnostics |
| GRAPH-08 | VERIFIED | U, I | stable node/edge identities, deterministic additions/removals, exact totals |
| GRAPH-09 | VERIFIED | I, M | coverage rows, path/scope freshness, parse partial/unusable, metadata |
| GRAPH-10 | VERIFIED | I | test markers and TESTS edges (test→product) |
| GRAPH-11 | PARTIAL | I, B | Bottom-k MinHash/LSH `SIMILAR_TO`; Nomic-vector `SEMANTICALLY_RELATED` with rare-token gating. Derived edges are preserved across a delta and refreshed on a full index |
| GRAPH-12 | VERIFIED | U, I | observed calls/paths by QN, run labels, counts, unresolved reporting, age-out |
| GRAPH-13 | PARTIAL | I | `CROSS_*` route/channel matching and run status; service detection depends on parsed Route metadata |
| GRAPH-14 | VERIFIED | I (`git_history_coupling_edges`) | Branch/working-tree nodes + `HAS_BRANCH`; bounded Git-history `FILE_CHANGES_WITH` (≥3 co-changes, 5000-edge cap) |
| GRAPH-15 | VERIFIED | I | importance = weighted degree; projections used by architecture |

## Search and retrieval

| ID | Status | Evidence | Notes / limits |
|---|---|---|---|
| SEARCH-01 | VERIFIED | U, M | all structural filters; exact totals; clear invalid-regex errors |
| SEARCH-02 | VERIFIED | U, M | FTS5 BM25 with camel/snake pre-tokenization; query/semantic exclusivity |
| SEARCH-03 | VERIFIED | U, I, B | Nomic token vectors stored and queried; independent semantic paging; no external service |
| SEARCH-04 | PARTIAL | M, I | compact/full/files, regex, context, raw/result/directory paging; result pages are snapshot-bound, raw-match pagination remains offset-based |
| SEARCH-05 | VERIFIED | U, M | QN/short-name resolution, auto/full/outline, member/file paging, neighbors |
| SEARCH-06 | PARTIAL | M | Deterministic 4-bytes/token ceiling, whole-row truncation; compact JSON only (no tree-text renderer) |

## Documentation, configuration and infrastructure

| ID | Status | Evidence | Notes / limits |
|---|---|---|---|
| DOC-01 | PARTIAL | U, I | Markdown/RST/AsciiDoc sections; PDF text layers via `pdf-extract`, no OCR; heading/page fidelity is lower than native parsers |
| DOC-02 | PARTIAL | I | Docstring refs → MENTIONS, exact/unique resolution; not every language-specific doc-comment parser is implemented |
| DOC-03 | VERIFIED | I | document links → REFERENCES_FILE, relative path resolution |
| DOC-04 | VERIFIED | U, I | ADR detection/facts/supersession + node/sections |
| DOC-05 | PARTIAL | U | Mermaid flowchart nodes/arrows; other diagram grammars are not implemented |
| DOC-06 | VERIFIED | U | OpenAPI/Swagger operations → Route, operationId handler link |
| INFRA-01 | VERIFIED | U | Dockerfile images/stages/ENV/entrypoint; no invented registry |
| INFRA-02 | VERIFIED | U, I | Compose services/images/depends_on/env, declared networks/volumes/secrets/configs, build contexts and links |
| INFRA-03 | PARTIAL | U, I | Multi-doc resources, labels, selectors, images, env, Ingress routes, ConfigMap/Secret name references; cross-file service linking resolves when the definition is indexed, otherwise a labeled placeholder is used |
| INFRA-04 | PARTIAL | U | Chart identity/dependencies, template image/value linking and values defaults; full `values`/`tpl` rendering is not implemented |
| INFRA-05 | PARTIAL | U | Kustomize resources/patches/charts, namePrefix/Suffix/namespace and image OVERRIDE nodes; no full strategic-merge rendering |
| INFRA-06 | PARTIAL | U, I | Terraform variables/defaults, distinct `resource.type.name` addresses, explicit resource/data references, module source/version, tfvars/helm_release and var/local/module USAGE edges; line/regex model, not full HCL evaluation |
| INFRA-07 | PARTIAL | U | Ansible play/task/handler/notify/imports/deploys; core forms only |
| INFRA-08 | PARTIAL | U | Make/CI shell commands → DEPLOYS/SPAWNS/HTTP; dynamic targets remain unresolved |
| INFRA-09 | VERIFIED | U | npm/Cargo/Go/PyPI/Composer/Pub/Maven/Gradle/Hex/RubyGems/Swift manifests |
| INFRA-10 | PARTIAL | I | Config key→symbol and env access; dep→import and file→ref links are partial |
| INFRA-11 | PARTIAL | U | Common route decorators/calls; no complete framework matrix, operator controllers not implemented |
| INFRA-12 | PARTIAL | U | dbt `ref`/`source` lineage; model-test parsing and full source resolution are limited |

## Impact and workflows

| ID | Status | Evidence | Notes / limits |
|---|---|---|---|
| IMPACT-01 | VERIFIED | I, M | Git diff → changed files/hunks/symbols, blast radius, risk, modules, paging |
| IMPACT-02 | PARTIAL | I | Conservative rules, TESTS/reference reachability, admitted coverage, `path_segment:N` filters and C/C++ presets; not every policy precedence or language convention is implemented |
| IMPACT-03 | PARTIAL | — | Artifact + coverage + receipt publish; graph/coverage hash binding; not byte-compatible with legacy bundle formats |
| IMPACT-04 | PARTIAL | U, CLI | Presets, path rules, key/segment rules, unreferenced/unmapped; `path_segment:N` uses conservative fallback; precedence is not exhaustive |
| IMPACT-05 | VERIFIED | U, I | observed edges marked; unresolved names reported |
| IMPACT-06 | VERIFIED | U, M | outline/get/update/set_sections/sections; byte-preserving idempotent splice |
| IMPACT-07 | VERIFIED | I | generation/extractor version, compare, explicit full-rebuild reasons |
| IMPACT-08 | PARTIAL | — | Dead-code analysis is available through Cypher; there is no dedicated dead-code tool |

## Bounded context and lifecycle hooks

| ID | Status | Evidence | Notes / limits |
|---|---|---|---|
| CTX-01 | VERIFIED | U, I, M, CLI | `get_task_context` composes target resolution, bounded traversal, coverage and change evidence; every row has source location, role, distance and provenance |
| CTX-02 | VERIFIED | U, I, M | `context`/`scout`/`auditor` profiles share one assembly path; scout adds a compact map, auditor adds checks, verified issues, risks and missing evidence |
| CTX-03 | VERIFIED | U, I, CLI | `gry hook` maps before_plan/before_edit/after_edit/before_finish to bounded context; JSON stdin/stdout, optional cache, `GRY_HOOK_ACTIVE` recursion guard |
| CTX-04 | VERIFIED | U, I, CLI | Deterministic `max_output_tokens` hard limit with whole-row truncation and `budget.omitted`; explicit `not_indexed`, `unavailable`, `timeout`, `stale`, `not_found`, `empty` and `partial` outcomes |

## Operations

| ID | Status | Evidence | Notes / limits |
|---|---|---|---|
| OPS-01 | VERIFIED | U/I | schema version, format version, transactional publish; corrupt DB errors are reported |
| OPS-02 | VERIFIED | I | full/moderate/fast/cross-repo mode behavior |
| OPS-03 | VERIFIED | M, CLI test | async job state (queued/running/succeeded/failed/cancelled), status polling, detached worker |
| OPS-04 | PARTIAL | M, I | Cancellation checks between phases/files; previous generation preserved on cancel. No hard deadline for one huge file |
| OPS-05 | VERIFIED | code | per-project OS file lock; WAL reads during publish |
| OPS-06 | PARTIAL | CLI, M | Opt-in polling watcher; `auto_index` can start background indexing on MCP initialize when roots are provided; no automatic watch |
| OPS-07 | VERIFIED | CLI test | cache/db environment, settings list/get/set/reset |
| OPS-08 | VERIFIED | U | allowed-root containment refusal |
| OPS-09 | PARTIAL | M, CLI | status/coverage/diagnostics and `gry doctor`; no retained trajectory/snapshot history |
| OPS-10 | PARTIAL | CLI test | Safe JSON/TOML edits for 17 curated clients + `--list-clients`; broader client matrix not implemented |
| OPS-11 | VERIFIED | CLI test | one-shot `gry cli` with flags/args-file/stdin; every tool callable |
| OPS-12 | VERIFIED | U, M | deterministic 4-bytes/token budget, whole-row truncation, snapshot cursors and continuation metadata |

## MCP

| ID | Status | Evidence | Notes |
|---|---|---|---|
| MCP-01 | VERIFIED | M | version negotiation, server info, instructions, capabilities |
| MCP-02 | VERIFIED | M | 18 tools, annotations, `_meta.alwaysLoad`, cursor paging, profiles |
| MCP-03 | VERIFIED | M | 2 prompts with validation |
| MCP-04 | VERIFIED | M, CLI | sync/async/status, modes, persistence, cross-repo targets |
| MCP-05 | VERIFIED | M | list paging/stats, explicit delete |
| MCP-06 | VERIFIED | M | status/coverage |
| MCP-07 | VERIFIED | M | search_graph |
| MCP-08 | VERIFIED | M | Cypher subset + explicit unsupported errors + missed graph |
| MCP-09 | VERIFIED | M | trace_path + alias + modes |
| MCP-10 | VERIFIED | M | snippets/outlines |
| MCP-11 | VERIFIED | M | schema/architecture/compare |
| MCP-12 | PARTIAL | M, I | compact/full/files, regex, context and snapshot result cursors; raw-match pagination remains offset-based |
| MCP-13 | VERIFIED | M | detect_changes |
| MCP-14 | VERIFIED | M | manage_adr |
| MCP-15 | VERIFIED | M | ingest_traces |
| MCP-16 | VERIFIED | M | invalid params/tool/method, notifications, shutdown, batch |
| MCP-17 | PARTIAL | code | Project from explicit arg/last project/roots/cwd; background auto-index on initialize when `auto_index=true`; no automatic watch |
| MCP-18 | VERIFIED | M | `get_task_context` schema, profiles, budget, typed unknown-project error, scout-profile visibility |

## CLI

First-class commands exist for
`index`/`status`/`projects`/`delete`/`search`/`trace`/`snippet`/`outline`/`schema`/`query`/
`architecture`/`coverage`/`changes`/`context`/`hook`/`adr`/`traces`/`compare`/`serve`/`cli`/`watch`/`test-impact`/
`config`/`install`/`uninstall`/`skill`. `gry install` supports a curated client set; `update` is not
implemented (use the package manager or rebuild). `gry version` is provided by clap.

## Known limits

These are the limits behind the PARTIAL rows. They are product decisions or honest depth limits, not
hidden failures.

- **Type resolution** is syntactic/inference-based, with confidence and explicit unresolved states;
  there is no external LSP runtime.
- **Language breadth** is 68 statically linked grammars plus a conservative lexical fallback, not
  every tree-sitter grammar in existence.
- **Derived semantic edges** are preserved across a delta and refreshed on a full index; they can lag
  after a one-file edit by design. Structural edges remain exact.
- **Output** is compact JSON (or pretty JSON); there is no tree-text renderer.
- **Client setup** covers 17 tested JSON/TOML adapters; other clients use `--config`.
- **Infrastructure evaluation** is bounded: Helm template/value linking without full rendering,
  Kustomize overrides without full strategic merge, Terraform refs/defaults without full HCL
  evaluation, and basic dbt `ref`/`source` lineage.
- **Test impact** models C/C++ conventions plus path/key/segment rules; full policy precedence is not
  implemented, and unknown input always falls back to `run_all`.
- **Search pagination**: `query_graph`, `trace_path` and `search_code` results are snapshot-bound;
  raw-match pagination and other tools use offsets.
- **Diagnostics** include status, coverage and `doctor`; there is no retained trajectory/snapshot
  history.
- **Watcher** is opt-in and polling-based; MCP auto-index runs only when configured.
- **Scale**: peak RSS at 100k files is unverified; the memory budget is a pre-allocation estimate.
  Measured: 10k files / 50k nodes at 749 MB.
- **PDF** uses text layers only; OCR is not supported.
- **Dead code** is a Cypher/search workflow, not a dedicated tool.
- **Context** is bounded evidence, not a semantic answer. `inferred` means ambiguous or
  search-derived evidence; it is not a confidence score. Absence of a relationship is not proof of
  safety.
- **Hooks** are an adapter, not an agent runtime. gry does not run planning, permissions, mutation
  approval or execution, and no universal hook compatibility is claimed.
- **Infrastructure** relationships are static declarations. Matching names or ports never creates a
  dependency; Kubernetes placeholders are labeled and contain no secret/config contents.
- **Process model**: no daemon; MCP jobs are in-process and CLI async uses a detached worker.

## Compatibility notes

- The MCP surface grows from 17 to 18 tools with `get_task_context`; existing tool schemas and
  pagination are unchanged. `gry context` and `gry hook` are additive CLI commands.
- Infrastructure edges add `ROUTES_TO`, `USES_CONFIG`, `USES_SECRET`, `USES_NETWORK` and
  `USES_VOLUME`. They are declared relationships, not inferred runtime dependencies.
- Terraform `resource`/`data` nodes now use `terraform.resource.<type>.<name>` and
  `terraform.data.<type>.<name>` so two resources of the same type no longer collide. This changes
  those QNs and requires a re-index; the extractor version bump (27) forces one clean rebuild.
- Compose/Kubernetes/Terraform nodes now retain their specific `ecosystem` property instead of being
  flattened to `infra`. Edge provenance treats those values as `declared`.
- No SQLite schema or artifact format version change. Existing projects remain readable; a full
  re-index is required to gain the new infrastructure relationships.

## Excluded

- Graph viewer, browser UI, HTTP UI endpoints, 3D/galaxy layouts, Monaco, embedded frontend assets.
- `get_architecture(aspects=["system_structure"])` (viewer-only projection).
- UI-only configuration keys and UI-only build/asset pipelines.

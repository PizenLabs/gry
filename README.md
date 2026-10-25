# gry

**Local-first code intelligence for AI agents.**

gry turns a repository into a persistent, queryable knowledge graph spanning source code, tests,
documentation, configuration, packages and infrastructure. It serves that graph to coding agents over
the [Model Context Protocol](https://modelcontextprotocol.io/) (MCP, stdio) and exposes the same
operations through a command-line interface.

gry is an independent, MIT-licensed open-source project. It runs entirely on your machine: no hosted
service, API key, telemetry or mandatory daemon. Everything is stored in a local SQLite database.

> **Status: 1.0.** The 1.0 feature scope is complete. Feature coverage, evidence and explicit
> depth/breadth limits are recorded in [docs/CAPABILITIES.md](docs/CAPABILITIES.md); measured
> performance is in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## Why gry

- **Structural answers, not file dumps.** Ask who calls a function, what a change reaches, how a
  service is deployed, or which tests to run. Results carry symbol identity, source ranges,
  provenance and pagination instead of raw file contents.
- **One graph across the whole repository.** Code, docs, ADRs, OpenAPI operations, package
  manifests, Dockerfiles, Kubernetes, Helm, Kustomize, Terraform, CI and runtime observations live in
  one model.
- **Local and self-contained.** A single binary, bundled SQLite, statically linked grammars and
  static token vectors. No network access at runtime.
- **Fast incremental indexing.** Unchanged files are not read or parsed. A one-file edit on a
  10,000-file repository lands in about 233 ms p50; a full index of the same workload runs at about
  2,800 files/s.
- **Honest uncertainty.** Resolution records exact, inferred, ambiguous and unresolved evidence.
  Unsupported constructs and coverage gaps are reported rather than guessed.

## Build

Requirements: Rust 1.85+ and a C toolchain (tree-sitter grammars and bundled SQLite are compiled by
Cargo). The first build downloads crates; runtime has no network dependency.

```bash
cargo build --release
./target/release/gry --version
```

Optional install:

```bash
cargo install --path crates/gry-cli
```

## Quick start

```bash
# Index a repository (name defaults to the directory name)
gry index /path/to/repo

# Inspect what was indexed
gry projects --stats
gry status my-project
gry schema my-project

# Structural questions
gry search my-project parse --label Method
gry trace my-project my-project.src.Engine.parse --direction outbound
gry snippet my-project my-project.src.Engine.parse
gry query my-project "MATCH (f:Function)-[:CALLS]->(g) RETURN f.name, g.name LIMIT 20"
gry architecture my-project --aspect overview --aspect routes

# Search source text with graph context
gry search-code my-project parse --limit 10

# Change impact
gry changes my-project --scope tests --base-branch main

# Health check
gry doctor
```

Every MCP tool is also a one-shot CLI command:

```bash
gry cli search_graph --project my-project --name-pattern '.*Handler.*' --label Function
gry cli query_graph --project my-project --query 'MATCH (n:Route) RETURN n.name' --format json
gry cli get_code_snippet --project my-project --qualified-name my-project.src.lib.main
gry cli index_repository --repo-path /path/to/repo --mode full
```

`gry cli <tool> --help` prints the tool's JSON schema. Arguments can also be supplied as
`--args-file file.json` or as JSON on stdin.

## MCP setup

`gry serve` speaks MCP over stdio. Configure a client manually:

```json
{
  "mcpServers": {
    "gry": {
      "command": "/absolute/path/to/gry",
      "args": ["serve"]
    }
  }
}
```

Or let gry edit a client configuration safely (backup + atomic write; `--dry-run` supported):

```bash
gry install --client claude
gry install --client cursor
gry install --client opencode
gry install --client codex
gry install --client zed
gry install --client vscode
gry install --client gemini
gry install --client windsurf
gry install --client roo
gry install --client qwen
gry install --list-clients
gry uninstall --client claude
```

15 tested JSON/TOML adapters are covered (Claude, Cursor, Windsurf, Gemini, OpenCode, Codex, Zed,
Cline, Roo, Kilo, VS Code, Trae, Qwen, Augment, Amp). Unknown JSON is preserved; files that are not
valid JSON are refused rather than overwritten. Other clients can be configured with `--config`.

## What it indexes

- **Code**: definitions, fields, methods, imports, calls, usages, inheritance, decorators,
  complexity, MinHash fingerprints and Nomic token embeddings.
- **Documents**: Markdown/RST/AsciiDoc sections, document-to-file/code references, ADRs, Mermaid
  diagrams and OpenAPI operations. PDFs are read for text layers only (no OCR).
- **Configuration and packages**: JSON/YAML/TOML/INI/.env keys without values; package manifests
  for npm, Cargo, Go, PyPI, Composer, Pub, Maven, Gradle, Hex, RubyGems and SwiftPM.
- **Infrastructure**: Dockerfiles, Compose, Kubernetes manifests, Helm charts, Kustomize,
  Terraform/OpenTofu, Ansible, Make, CI workflows and deployment commands.
- **Runtime observations**: `ingest_traces` stores observed caller→callee pairs and journeys by
  qualified name, so they survive reindexing.

Languages with a bundled tree-sitter grammar include Rust, Python, JavaScript/TypeScript/TSX, Go,
C, C++, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Lua, Bash, Elixir, Haskell, OCaml, Dart, R,
Julia, Zig, Nix, HCL, YAML, TOML, JSON, Markdown, Dockerfile, Make, CMake, SQL, GraphQL, HTML, CSS,
SCSS, Vue, Svelte, Groovy, Erlang, F#, Solidity, PowerShell, Pascal, D, Verilog, VHDL, Ada, Common
Lisp, Scheme, Elm, Fortran, CUDA, GLSL, Starlark, INI, Diff, Regex, GDScript, Luau, Nickel, Crystal,
QML, Jinja, Prisma, Properties and more (68 statically linked grammars). Formats without a
compatible modern grammar use a conservative lexical fallback (Perl, Vim, Fish, COBOL, WGSL/HLSL,
IDL formats, Lisp dialects); this is explicitly marked in node properties.

## Graph model

Node labels include `Project`, `Package`, `Folder`, `File`, `Module`, `Section`, `Class`, `Struct`,
`Interface`, `Trait`, `Enum`, `Type`, `Function`, `Method`, `Field`, `Variable`, `Constant`,
`Macro`, `Decorator`, `Route`, `Channel`, `Process`, `EnvVar`, `Resource`, `Chart`, `Image`,
`Branch`, `ADR`, `Diagram` and `DiagramElement`.

Edges include `DEFINES`, `DEFINES_METHOD`, `IMPORTS`, `CALLS`, `CALL_REFERENCE`, `USAGE`,
`INHERITS`, `IMPLEMENTS`, `OVERRIDE`, `DECORATES`, `DATA_FLOWS`, `HTTP_CALLS`, `ASYNC_CALLS`,
`EMITS`, `LISTENS_ON`, `HANDLES`, `SPAWNS`, `CONFIGURES`, `INFRA_MAPS`, `CONNECTS_TO`, `DEPLOYS`,
`DEPENDS_ON`, `REFERENCES_FILE`, `MENTIONS`, `TESTS`, `SIMILAR_TO`, `SEMANTICALLY_RELATED`,
`FILE_CHANGES_WITH`, `HAS_BRANCH`, the `CROSS_*` family and internal diagram/ADR edges. Every edge
keeps provenance/confidence properties where the resolver provides them.

Qualified names follow `<project>.<path>.<scope>.<name>`; File QNs keep the extension
(`<project>.<path>.<file>.__file__`); `__init__`/`index` are dropped when a symbol name is present;
Go/Java modules are directory-based.

## Incremental indexing

The index stores a compressed per-file extraction cache (MessagePack + zstd). On re-index:

- unchanged files are **not read or parsed** (mtime/size + SHA-256 short-circuit);
- the changed file is re-extracted, reusing per-symbol bodies whose hash did not change;
- only the changed files, module siblings, files referencing a changed symbol name, and reverse
  dependencies are re-resolved — with the complete project symbol registry;
- deleted files/edges/cache/coverage disappear; removed symbols are reported and dependent callers
  are re-resolved;
- a nodes-only fast path skips all edge work when a body edit does not move any call/usage/import
  or source range;
- publication is one SQLite transaction, so a failed or cancelled run leaves the previous
  generation queryable.

Measured (see [docs/BENCHMARKS.md](docs/BENCHMARKS.md)): 10,001 medium files — full index 3.57 s
(~2,800 files/s), one-file edit **233 ms p50**; dense 12.6k-line C file — comment-only edit 188 ms,
local structural edit 363 ms. A public-symbol change re-resolves every dependent file by design.
Derived semantic/similarity edges are preserved across deltas and refreshed on a full index.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the pipeline and consistency rules.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `GRY_CACHE_DIR` | `~/.cache/gry` | Cache root for the database, settings, jobs and locks |
| `GRY_DB` | `<cache>/graph.db` | SQLite database file |
| `GRY_ALLOWED_ROOT` | unset | Confine `index_repository` to a directory |
| `GRY_WORKERS` | `min(8, cores)` | Indexing worker bound |
| `GRY_MAX_INDEX_FILES` | unset | Per-run accepted file limit |
| `GRY_MAX_INDEX_SOURCE_MB` | unset | Per-run source-size limit |
| `GRY_MAX_INDEX_MEMORY_MB` | unset | Soft peak-memory estimate; rejects the run before allocation if exceeded |

Runtime settings (`auto_index`, `auto_index_limit`, `auto_watch`, `doc_candidates`,
`watcher_enabled`, `watch_non_git`, `index_max_files`, `index_max_source_mb`,
`index_max_memory_mb`, `semantic_enabled`) are stored in `<cache>/settings.json` and managed with:

```bash
gry config list
gry config get auto_watch
gry config set semantic_enabled false
gry config reset auto_watch
```

### Repository configuration

`.gry.json` at the repository root configures conservative test-impact selection:

```json
{
  "test_impact": {
    "version": 1,
    "tests": { "presets": { "c": true }, "test_globs": ["tests/**"] },
    "rules": [
      { "id": "src", "paths": ["src/**"], "action": "referencing_tests" }
    ],
    "unmapped_files": "run_all"
  }
}
```

`.gitignore`, nested `.gitignore`, `.gryignore` and global git excludes shape discovery. The safety
core (`.git`, `node_modules`, worktrees) cannot be re-included.

## Index modes

| Mode | Behavior |
|---|---|
| `full` | Everything, including semantic relatedness and doc candidates |
| `moderate` | Filtered discovery with semantic relatedness |
| `fast` | Filtered discovery, no semantic relatedness/doc candidates |
| `cross-repo-intelligence` | Link routes and channels across already-indexed projects |

`index_repository(async=true)` starts a background job in the MCP process. One-shot CLI
`--async` starts a detached worker and writes status under `<cache>/jobs/`; poll with
`gry index <repo> --status`. No long-lived daemon is required.

## Test impact

`gry test-impact select` is conservative: it never omits a test suite unless admitted per-test
coverage and a static mapping prove it safe. Missing config, Git history or coverage returns
`run_all`. `publish` writes a graph artifact + coverage + receipt bundle to `--out`.

```bash
gry test-impact select --repo . --base main --coverage ./coverage --platform linux-x86_64
gry test-impact publish --repo . --out ./bundle --coverage ./coverage
```

Coverage format: `meta.json` (`commit`, `platform`, `created_at`), `tests.tsv`
(`function_qn<TAB>test_qn`), optional `functions.tsv`. Narrowing currently models C/C++-style test
frameworks; other languages are accepted as inputs but selection falls back safely.

## Verification

```bash
cargo test --workspace
cargo build --release
tests/differential.sh          # optional: requires the reference binary on PATH
```

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the full verification checklist, test layout and
release process.

## Scope

gry is a code-intelligence engine, not a graph viewer. Browser UI, 3D graph rendering, HTTP UI
endpoints, embedded frontend assets and the UI-only `system_structure` architecture projection are
deliberately out of scope.

## Documentation

| Document | Contents |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System design, storage, indexing, resolution, performance model |
| [docs/CAPABILITIES.md](docs/CAPABILITIES.md) | Feature coverage, supported languages, explicit limits |
| [docs/BENCHMARKS.md](docs/BENCHMARKS.md) | Measured performance, workloads and methodology |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Build, test, verification and release workflow |
| [CHANGELOG.md](CHANGELOG.md) | Release history |
| [THIRD_PARTY.md](THIRD_PARTY.md) | Third-party licenses and attribution |

## License and attribution

gry is MIT licensed; see [LICENSE](LICENSE). It reuses tree-sitter grammar crates, SQLite (bundled by
`rusqlite`) and the Apache-2.0 Nomic `nomic-embed-code` token vectors. Details:
[THIRD_PARTY.md](THIRD_PARTY.md).

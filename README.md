<div align="center">

<img src="https://raw.githubusercontent.com/PizenLabs/onpic/refs/heads/main/gry/gry.png" width="24%" alt="Lynx Logo" />


**Local-first code intelligence for AI agents.**

[![CI](https://github.com/PizenLabs/gry/actions/workflows/ci.yml/badge.svg)](https://github.com/PizenLabs/gry/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/gry.svg)](https://crates.io/crates/gry)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org/)
[![MCP](https://img.shields.io/badge/MCP-stdio-6f42c1.svg)](https://modelcontextprotocol.io/)

</div>

[gry]() turns a repository into a persistent, queryable knowledge graph spanning source code, tests,
documentation, configuration, packages, and infrastructure. It serves that graph to coding agents
over the [Model Context Protocol](https://modelcontextprotocol.io/) (MCP, stdio) and exposes the
same operations through a command-line interface.

[gry]() is an independent, Apache-2.0-licensed open-source project. It runs entirely on your machine:
no hosted service, API key, telemetry, or mandatory daemon. Everything is stored in a local SQLite
database.


> **Status: 1.0.** The 1.0 feature scope is complete. Feature coverage, evidence, and explicit
> depth/breadth limits are recorded in [docs/CAPABILITIES.md](docs/CAPABILITIES.md); measured
> performance is in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## Contents

- [Why gry](#why-gry)
- [At a glance](#at-a-glance)
- [Installation](#installation)
- [Quick start](#quick-start)
- [MCP integration](#mcp-integration)
- [What it indexes](#what-it-indexes)
- [Graph model](#graph-model)
- [Incremental indexing](#incremental-indexing)
- [Configuration](#configuration)
- [Index modes](#index-modes)
- [Test impact](#test-impact)
- [Verification](#verification)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [Security](#security)
- [License](#license)

## Why gry

- **Structural answers, not file dumps.** Ask who calls a function, what a change reaches, how a
  service is deployed, or which tests to run. Results carry symbol identity, source ranges,
  provenance, and pagination instead of raw file contents.
- **One graph across the whole repository.** Code, docs, ADRs, OpenAPI operations, package
  manifests, Dockerfiles, Kubernetes, Helm, Kustomize, Terraform, CI, and runtime observations live
  in one model.
- **Local and self-contained.** A single binary, bundled SQLite, statically linked grammars, and
  static token vectors. No network access at runtime.
- **Fast incremental indexing.** Unchanged files are not read or parsed. A one-file edit on a
  10,000-file repository lands in about 233 ms p50; a full index of the same workload runs at about
  2,800 files/s.
- **Honest uncertainty.** Resolution records exact, inferred, ambiguous, and unresolved evidence.
  Unsupported constructs and coverage gaps are reported rather than guessed.

## At a glance

| | |
|---|---|
| **Interfaces** | MCP over stdio, first-class CLI, one-shot `gry cli <tool>` |
| **Storage** | Local SQLite; no hosted service and no mandatory daemon |
| **Languages** | 68 statically linked tree-sitter grammars plus a conservative lexical fallback |
| **Scope** | Code, tests, docs, config, packages, infrastructure, and runtime traces |
| **Privacy** | No network access, API keys, or telemetry at runtime |
| **Platforms** | macOS and Linux (CI-tested) |

## Installation

### One-line install

```bash
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash
```

The installer detects macOS or Linux and the machine architecture (including Rosetta),
downloads the matching prebuilt binary, verifies its SHA-256, installs it into
`~/.local/bin`, and configures the MCP server for the agent harnesses it detects
(Claude Code, Claude Desktop, Cursor, Codex, OpenCode, Gemini CLI,
VS Code/Cline/Roo/Kilo, Zed, Windsurf, and others). It falls back to a Cargo
source build when no release asset matches.

```bash
# Configure specific harnesses instead of auto-detection.
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash -s -- --client claude,opencode

# Pin a release and install directory.
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash -s -- --version v1.0.0 --prefix "$HOME/bin"

# Install the binary without touching MCP configuration.
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash -s -- --no-mcp

# MCP + native lifecycle hooks (currently OpenCode V2).
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash -s -- --client opencode --hooks

# Show what gry owns and whether a managed file was edited.
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash -s -- --status

# Remove gry, its MCP entries, managed hook adapter, and the PATH block.
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash -s -- --uninstall
```

`--client auto` is the default; `gry install --list-clients` prints every id. The
installer also installs the gry agent skill and adds the install directory to your
shell PATH when needed (`--no-skill` and `--no-modify-path` opt out). Native hooks
are opt-in with `--hooks` and are not activated by auto-detection.

See [docs/INTEGRATIONS.md](docs/INTEGRATIONS.md) for the runtime matrix, event
coverage, ownership rules, and troubleshooting.

### Requirements

- Rust 1.85+ and Cargo (source installs and builds)
- A C toolchain (tree-sitter grammars and bundled SQLite are compiled by Cargo)

### From crates.io

```bash
cargo install gry --locked
```

This builds the `gry` binary and installs it into `~/.cargo/bin`. Use
`cargo install gry --locked --force` to upgrade an existing install. The
supporting libraries are published as `gry-core`, `gry-store`, `gry-parse`,
`gry-index`, `gry-query`, and `gry-mcp`.

### From source

```bash
git clone https://github.com/PizenLabs/gry.git
cd gry
cargo build --release
./target/release/gry --version
```

Install the binary into your Cargo bin directory:

```bash
cargo install --path crates/gry-cli
```

Or install it into `~/.local/bin` with the Makefile:

```bash
make install
```

Prebuilt binaries for macOS (arm64/x86_64) and Linux (glibc/musl, x86_64/aarch64)
are published with each release. Verify any install with `gry doctor`.

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

# Bounded task context and lifecycle hooks
gry context my-project --target my-project.src.Engine.parse --profile scout
gry hook before_edit --project my-project --target my-project.src.Engine.parse

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

## MCP integration

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

Or let gry edit a client configuration safely (backup and atomic write; `--dry-run` supported):

```bash
gry install --client auto      # detect harnesses on this machine
gry install --client all       # configure every supported harness
gry install --client claude
gry install --client claude-desktop
gry install --client cursor
gry install --client opencode
gry install --client omp
gry install --client ohgo
gry install --client codex
gry install --client zed
gry install --client vscode
gry install --client gemini
gry install --client windsurf
gry install --client roo
gry install --client qwen
gry install --list-clients
gry uninstall --client auto
gry uninstall --client claude
```

`--client auto` checks existing configuration files, harness home directories, applications, and
`PATH` executables, then configures only what it finds. It is what the one-line installer uses.

Eighteen tested JSON/TOML adapters are covered (Claude Code, Claude Desktop, Cursor,
Windsurf, Gemini, OpenCode, OMP, OHGO, Codex, Zed, Cline, Roo, Kilo, VS Code, Trae,
Qwen, Augment, and Amp). Paths
follow the platform: VS Code and its extensions use `~/Library/Application Support/Code/User` on
macOS and `~/.config/Code/User` on Linux; Claude Desktop uses its platform app config. The OHGO
adapter writes its `mcp.servers` entry with explicit read/mutate classifications. Unknown JSON is
preserved; files that are not valid JSON are refused rather than overwritten. Other clients can be
configured with `--config`.

### Bounded context and lifecycle hooks

`get_task_context` returns a small, evidence-backed payload instead of a repository dump. It
resolves targets by qualified name, symbol name, file or bounded BM25 search, then adds callers,
callees, imports, service relationships, coverage, optional test-gap evidence and optional change
evidence. Every row keeps its source location, role, distance and provenance (`verified`,
`declared`, `inferred` or `observed`). `project` is optional when the workspace root is indexed.
`include_source` adds exact bounded source for the most relevant callables so the caller does not
need a second snippet/read round trip. `max_output_tokens` is a hard limit with whole-row
truncation and explicit omitted counts.

Short names resolve deterministically: production code is preferred over generated mocks and
tests, and an ambiguous short name returns candidates instead of silently tracing the first match.
`Type.Method`, a unique suffix and a full qualified name are accepted by `trace_path` and
`get_code_snippet`.

Three profiles share one implementation:

- `context` — focused evidence for known targets;
- `scout` — a compact map of symbols, files and services plus follow-up queries;
- `auditor` — changed files, checks performed, verified issues, possible risks and missing evidence.

For agent runtimes, `gry hook <stage>` adapts the same engine at `before_plan`, `before_edit`,
`after_edit` and `before_finish`. It reads JSON on stdin and returns `{hook, context, failure}`.
Hooks are optional, time-bounded, cached by project generation and inputs, and guarded against
recursion with `GRY_HOOK_ACTIVE`. When GRY is unavailable or a project is not indexed, the result
says so explicitly; it is never reported as a verified empty result. Planning, permissions,
mutation approval and execution stay in the consuming agent runtime.

## What it indexes

- **Code**: definitions, fields, methods, imports, calls, usages, inheritance, decorators,
  complexity, MinHash fingerprints, and Nomic token embeddings.
- **Documents**: Markdown/RST/AsciiDoc sections, document-to-file/code references, ADRs, Mermaid
  diagrams, and OpenAPI operations. PDFs are read for text layers only (no OCR).
- **Configuration and packages**: JSON/YAML/TOML/INI/.env keys without values; package manifests
  for npm, Cargo, Go, PyPI, Composer, Pub, Maven, Gradle, Hex, RubyGems, and SwiftPM.
- **Infrastructure**: Dockerfiles, Compose (services, networks, volumes, secrets/configs, build
  contexts), Kubernetes manifests (workloads, services, Ingress routes, ConfigMap/Secret name
  references), Helm charts, Kustomize, Terraform/OpenTofu (distinct resource addresses, explicit
  references, modules), Ansible, Make, CI workflows, and deployment commands. Secret and config
  contents are never indexed.
- **Runtime observations**: `ingest_traces` stores observed caller→callee pairs and journeys by
  qualified name, so they survive reindexing.

Languages with a bundled tree-sitter grammar include Rust, Python, JavaScript/TypeScript/TSX, Go,
C, C++, Java, C#, Ruby, PHP, Kotlin, Swift, Scala, Lua, Bash, Elixir, Haskell, OCaml, Dart, R,
Julia, Zig, Nix, HCL, YAML, TOML, JSON, Markdown, Dockerfile, Make, CMake, SQL, GraphQL, HTML, CSS,
SCSS, Vue, Svelte, Groovy, Erlang, F#, Solidity, PowerShell, Pascal, D, Verilog, VHDL, Ada, Common
Lisp, Scheme, Elm, Fortran, CUDA, GLSL, Starlark, INI, Diff, Regex, GDScript, Luau, Nickel, Crystal,
QML, Jinja, Prisma, Properties, and more (68 statically linked grammars). Formats without a
compatible modern grammar use a conservative lexical fallback (Perl, Vim, Fish, COBOL, WGSL/HLSL,
IDL formats, Lisp dialects); this is explicitly marked in node properties.

## Graph model

Node labels include `Project`, `Package`, `Folder`, `File`, `Module`, `Section`, `Class`, `Struct`,
`Interface`, `Trait`, `Enum`, `Type`, `Function`, `Method`, `Field`, `Variable`, `Constant`,
`Macro`, `Decorator`, `Route`, `Channel`, `Process`, `EnvVar`, `Resource`, `Chart`, `Image`,
`Branch`, `ADR`, `Diagram`, and `DiagramElement`.

Edges include `DEFINES`, `DEFINES_METHOD`, `IMPORTS`, `CALLS`, `CALL_REFERENCE`, `USAGE`,
`INHERITS`, `IMPLEMENTS`, `OVERRIDE`, `DECORATES`, `DATA_FLOWS`, `HTTP_CALLS`, `ASYNC_CALLS`,
`EMITS`, `LISTENS_ON`, `HANDLES`, `SPAWNS`, `CONFIGURES`, `INFRA_MAPS`, `CONNECTS_TO`, `ROUTES_TO`,
`USES_CONFIG`, `USES_SECRET`, `USES_NETWORK`, `USES_VOLUME`, `DEPLOYS`, `DEPENDS_ON`,
`REFERENCES_FILE`, `MENTIONS`, `TESTS`, `SIMILAR_TO`, `SEMANTICALLY_RELATED`,
`FILE_CHANGES_WITH`, `HAS_BRANCH`, the `CROSS_*` family, and internal diagram/ADR edges. Every edge
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
| `GRY_CACHE_DIR` | `~/.cache/gry` | Cache root for the database, settings, jobs, and locks |
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

`.gitignore`, nested `.gitignore`, `.gryignore`, and global git excludes shape discovery. The
safety core (`.git`, `node_modules`, worktrees) cannot be re-included.

## Index modes

| Mode | Behavior |
|---|---|
| `full` | Everything, including semantic relatedness and doc candidates |
| `moderate` | Filtered discovery with semantic relatedness |
| `fast` | Filtered discovery, no semantic relatedness/doc candidates |
| `cross-repo-intelligence` | Link routes and channels across already-indexed projects |

`index_repository(async=true)` starts a background job in the MCP process. One-shot CLI `--async`
starts a detached worker and writes status under `<cache>/jobs/`; poll with
`gry index <repo> --status`. No long-lived daemon is required.

## Test impact

`gry test-impact select` is conservative: it never omits a test suite unless admitted per-test
coverage and a static mapping prove it safe. Missing config, Git history, or coverage returns
`run_all`. `publish` writes a graph artifact + coverage + receipt bundle to `--out`.

```bash
gry test-impact select --repo . --base main --coverage ./coverage --platform linux-x86_64
gry test-impact publish --repo . --out ./bundle --coverage ./coverage
```

Coverage format: `meta.json` (`commit`, `platform`, `created_at`), `tests.tsv`
(`function_qn<TAB>test_qn`), and optional `functions.tsv`. Narrowing currently models C/C++-style
test frameworks; other languages are accepted as inputs but selection falls back safely.

## Verification

```bash
cargo test --workspace
cargo build --release
tests/differential.sh          # optional: requires the reference binary on PATH
```

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the full verification checklist, test layout,
and release process.

## Documentation

| Document | Contents |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System design, storage, indexing, resolution, performance model |
| [docs/CAPABILITIES.md](docs/CAPABILITIES.md) | Feature coverage, supported languages, explicit limits |
| [docs/BENCHMARKS.md](docs/BENCHMARKS.md) | Measured performance, workloads, and methodology |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Build, test, verification, and release workflow |
| [CHANGELOG.md](CHANGELOG.md) | Release history |
| [THIRD_PARTY.md](THIRD_PARTY.md) | Third-party licenses and attribution |

## Contributing

gry welcomes contributions of all kinds: bug reports, feature proposals, documentation, tests, and
code. Read [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, commit conventions, the
verification checklist, and the pull-request process.

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). By
participating, you agree to uphold it.

## Security

Please do not report security vulnerabilities in public issues. Use GitHub's private
vulnerability reporting or contact the maintainers as described in [SECURITY.md](SECURITY.md).

## License

gry is licensed under the [Apache License, Version 2.0](LICENSE). See [NOTICE](NOTICE) for
attribution.

gry reuses tree-sitter grammar crates, SQLite (bundled by `rusqlite`), and Apache-2.0 Nomic
`nomic-embed-code` token vectors. Details: [THIRD_PARTY.md](THIRD_PARTY.md).

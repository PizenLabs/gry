# GRY End-to-End Test Results — 2026-11-22

All functional tests below were executed through the live MCP server (`gry serve`) using OpenCode as
the MCP client. Two restart/interruption experiments used a second real `gry serve` process over
stdio. Source reads were used only to verify MCP claims.

Legend: **PASS** = behavior and evidence meet the objective. **PARTIAL** = usable result with a
documented limitation. **BLOCKED** = could not be executed. **FAIL** = reproduced defect before the
fix; the “after” column records the re-test.

## Group A — Project management

| ID | Objective | Tool / arguments | Expected | Actual after fixes | Result |
|---|---|---|---|---|---|
| A01 | List indexed projects | `list_projects` detail=identity | Deterministic list | 7 projects returned; no duplicates | PASS |
| A02 | Retrieve ecommerce | `index_status` project=ecommerce | Project, root, generation | ecommerce, absolute root, generation `1795297591042-…`, root_available=true | PASS |
| A03 | Resolve by actual root path | `search_graph` project=`/Users/.../ecommerce` | Resolve to ecommerce | 3 ProductStorage results; same as name-based lookup | PASS (failed before fix) |
| A03b | Resolve by descendant path | project=`.../ecommerce/backend` | Resolve to ecommerce | Same project, same results | PASS |
| A04 | Invalid project | project=`nope-xyz` | Actionable error, no fallback | “project not found or not indexed”, lists all projects and next step | PASS |
| A05 | Omit project argument | `search_graph` name=ProductStorage | Default to indexed workspace root | Defaulted to ecommerce | PASS |
| A06 | Delete disposable project | `delete_project` | Deletes only target | Deleted `ecommerce-cold-audit`, `gry-unavailable`, `gry-audit-fixture`; ecommerce intact | PASS |
| A07 | Restart persistence | New `gry serve` process after OpenCode MCP reload | Projects/symbols persist | ecommerce and symbols available after restart | PASS |

## Group B — Code search

| ID | Objective | Tool / arguments | Expected | Actual | Result |
|---|---|---|---|---|---|
| B01 | Common function name | `search_graph` name_pattern=`^New$` | Multiple declarations, bounded | 20 total, functions from several bounded contexts | PASS |
| B02 | Unique function | name_pattern=`AdminCreateVariantProduct` | One exact declaration | One method at `layout/http_admin_products.go:408-557` | PASS |
| B03 | Struct name | name_pattern=`^Product$` | Struct returned | 5 matches; `cart` and `productcatalog` Struct nodes present | PASS (0 structs before fix) |
| B04 | Interface name | name_pattern=`^ProductStorage$` | Interface returned | 3 matches, production interface first | PASS (0 before fix) |
| B05 | Constant | name_pattern=`defaultSessionSecret` | Constant + location | `cmd/web/config.go:148` | PASS |
| B06 | Configuration key | `search_graph` label=EnvVar; `search_code` SESSION_SECRET | Find key | `search_code` found 3 files / 5 matches; `EnvVar` nodes do not include Go conf tags | PARTIAL |
| B07 | Error message | `search_code` “the name cannot be empty” | Exact source hit | `productcatalog/domain/product.go`, line 112 | PASS |
| B08 | HTTP route | name_pattern=`api/v1/products` | Route node | `__route__ANY__/api/v1/products` at `layout/http.go:234` | PASS |
| B09 | Database operation | `search_code` “INSERT INTO auth_admin” | SQL hits | 3 files / 3 matches including migration 000038 | PASS |
| B10 | Business identifier | name_pattern=`AddVariantToProduct` | Interface + implementation | `catalogService.AddVariantToProduct` and `ProductService.AddVariantToProduct` | PASS |
| B11 | Partial identifier | name_pattern=`Variant` | Bounded ranked page | 99 matches; interface/implementation declarations ranked before template/file noise | PASS |
| B12 | Nonexistent symbol | name_pattern=`ZzzNoSuchSymbol` | Empty with total 0 | Empty, `total:0`, no error | PASS |
| B13 | Case variation | name_pattern=`productstorage` | Deterministic behavior | 3 matches (case-insensitive resolution) | PASS |
| B14 | Path-restricted | file_pattern=`backend/productcatalog/app/product.go` | Only that file | 2 symbols in that file | PASS |
| B15 | Special characters | name_pattern=`/product/\{productID\}` | Route matches | 3 route nodes | PASS |
| B16 | Large result set | `search_graph` limit=500 | Bounded/truncated with metadata | `total:2384`, 49 rows returned by output budget, `has_more:true`, next offset | PASS |
| B17 | Template text | `search_code` `{{define` | Templates searchable | 55 files / 98 matches including 54 `.gohtml` files | PASS (0 before fix) |
| B18 | Path traversal in outline | `get_file_outline` `../etc/passwd` | Reject | “invalid repository-relative file_path” | PASS |

## Group C — Symbol navigation

| ID | Objective | Tool / arguments | Expected | Actual | Result |
|---|---|---|---|---|---|
| C01 | Symbol lookup | `search_graph` ProductStorage | Exact declaration | Interface at `productcatalog/app/product.go:53-104` | PASS |
| C02 | Qualified name | `get_code_snippet` full QN | Exact source | ProductService struct source and line range | PASS |
| C03 | Package recognition | `search_graph` label=Package | Package nodes | 131 package nodes (project + external), 50 project packages in architecture | PASS |
| C04 | Declaration locations | `get_file_outline` product.go | Source-order declarations | Struct/field/method hierarchy with exact lines | PASS |
| C05 | Parent-child | outline ProductService / Product | Type → fields/methods | Fields scoped under the struct (e.g. `ProductService.storage`) | PASS (fields package-scoped before fix) |
| C06 | Method receiver | trace ProductService.Add | Method identity + caller/callee | Method node, correct receiver context | PASS (methods labeled Function before fix) |
| C07 | Interface implementations | `trace_path` / query | Implementations identified | Interface + consumers found; no `IMPLEMENTS` edges for implicit Go implementations | PARTIAL |
| C08 | Cross-package references | trace ProductStorage.Add inbound | Consumers | `ProductService.Add` / `AddVariantProduct` found | PASS |
| C09 | Ambiguous symbol | `get_code_snippet` NewProduct | Candidates, no guess | “ambiguous symbol … pass one qualified name” with 2 QNs | PASS |
| C10 | Missing symbol | `get_code_snippet` full fake QN | Clear error | “symbol not found” with search hint | PASS |

## Group D — Graph intelligence

| ID | Objective | Tool / arguments | Expected | Actual | Result |
|---|---|---|---|---|---|
| D01 | Incoming references | trace catalogService.Add inbound | True callers | 1 caller: `AdminCreateProduct` (confidence 1.00) | PASS |
| D02 | Outgoing references | trace AdminCreateVariantProduct | Callees | Service calls + parse helpers; no logger false edge | PASS |
| D03 | Function calls | trace ProductService.Add | Call graph | `ProductStorage.Add`, domain constructors, reindex helper | PASS |
| D04 | Package dependencies | `get_architecture` dependencies | Import/dependency view | Module→package dependency counts | PASS |
| D05 | Type relationships | `DEFINES_METHOD`, fields | Type→member edges | 2,919 `DEFINES_METHOD` edges | PASS |
| D06 | Cross-module relationships | imports | Module edges | 828 `IMPORTS` edges | PASS |
| D07 | Depth limits | trace depth 1–3; depth 99 | Bounded; invalid rejected | Depth 99 rejected by schema validation | PASS |
| D08 | Cycles | architecture cycles | Report cycles | Empty cycle set over CALLS; caveat emitted | PASS |
| D09 | Disconnected nodes | Cypher no incoming CONTAINS_FILE | Explain isolated nodes | Two referenced-path placeholders with `placeholder:true` | PASS |
| D10 | Missing relationships | inspect interface implementation links | Honest completeness | Interface dispatch conservative; implicit implementations absent | PARTIAL |
| D11 | Invalid graph query | `MATCH` / `DETACH DELETE` | Explicit rejection | “unsupported: expected '('” / “unsupported clause” | PASS |
| D12 | False relationships | query HANDLES + CALLS targets | No false edges | logger HANDLES 0; CALLS targets only Function/Method | PASS (after fix) |
| D13 | Aggregates | count functions / absent label | Exact counts | 2,384 functions; absent-label count returns one row `0` | PASS |

## Group E — Code inspection

| ID | Objective | Tool / arguments | Expected | Actual | Result |
|---|---|---|---|---|---|
| E01 | Existing function | snippet ProductService.Add | Exact source | Full function, lines 586–621 | PASS |
| E02 | Existing struct | snippet ProductService | Source | Struct + four fields | PASS (not found before fix) |
| E03 | Existing interface | snippet ProductStorage outline | Member list | 45 methods, paged, 41 returned with continuation | PASS (not found before fix) |
| E04 | Existing method | snippet ProductService.AddVariantToProduct | Source | Exact source | PASS |
| E05 | Multiple declarations | snippet NewProduct | Ambiguity error | Two candidates returned | PASS |
| E06 | Nested declarations | outline product.go | Fields/methods under types | Correct nesting and line numbers | PASS |
| E07 | Nonexistent symbol | snippet fake QN | Error | “symbol not found” | PASS |
| E08 | Deleted/moved file | unavailable-root project | Clear unavailable error | Source/outline/search_code error; graph metadata still readable | PASS |
| E09 | Invalid line range | start_line=99999 | Error | “recorded source range … outside the current file … stale” | PASS |
| E10 | Large source | main.main auto | Bounded source + continuation | 120 lines, `truncated:true`, `next_start_line:178` | PASS (no source before fix) |
| E11 | Stale source | fixture modified after index | No wrong success | Explicit “changed since it was indexed” error | PASS (wrong source before fix) |
| E12 | Missing outline | `missing.go` | Error | “file not found in the index or on disk” | PASS (empty success before fix) |
| E13 | Unavailable source | missing project root | Explicit error | “source unavailable … re-index” | PASS |
| E14 | Template outline | `show.gohtml` | Indexed, no declarations | `total:0`, indexed file, no error | PASS |

## Group F — Context retrieval

| ID | Question | Evidence used | Assessment | Result |
|---|---|---|---|---|
| F01 | Main architecture | `get_architecture` overview/structure/dependencies/packages | Project shape, language counts, package/module dependencies, layers; broad `get_task_context` task-only targets were noisy | PASS (use architecture for shape) |
| F02 | HTTP server initialization | `search_graph` HTTPServer, snippet `App.HTTPServer`, `main.main` source | `App.HTTPServer` returns `app.httpServer`; main source around DB/server setup retrieved | PASS |
| F03 | API route registration | `search_graph` MuxRegister, query HANDLES | 131 handler edges; `/auth/login` maps to Login/HandleLogin | PASS (logger false edges before fix) |
| F04 | Authentication | trace HandleLogin → authService.Login; concrete `auth.auth.Login` | Interface call + concrete implementation + CustomerStorage/SessStorage calls | PASS with implementation-link limitation |
| F05 | Database connectivity | `search_code` otelsql.Open; main snippet | `main.go:140-151`: connection string, `otelsql.Open`, pool settings, `dependency.NewSQL` | PASS |
| F06 | Handler → DB flow | traces Product/AdminCreate + dependency symbols | Core service calls are visible; concrete interface implementations are not traversed automatically | PARTIAL |
| F07 | Business packages | architecture packages/structure | 50 project packages; productcatalog/app/domain/adapter visible | PASS |
| F08 | Functions depending on service | trace inbound service/interface methods | Service consumers listed with provenance/confidence | PASS |
| F09 | Error handling | `search_code` `errors.Is(` | 77 files / 180 matches, including HTTP boundary translations | PASS |
| F10 | Major dependencies | architecture dependencies | Direct module/package dependency counts | PASS |

## Group G — Incremental indexing and freshness

Fixture: a disposable Go repository outside ecommerce (`/private/var/folders/.../gry-audit-fixture`).

| ID | Step | Actual | Result |
|---|---|---|---|
| G01 | Record initial state | 4 files, 17 nodes, 33 edges | PASS |
| G02 | External fixture only | Created under the approved temp directory, never in ecommerce | PASS |
| G03 | Add source file | `extra.go` with `Extra` calling `Add` | PASS |
| G04 | Index change | Incremental: 1 file parsed, 3 reused | PASS |
| G05 | Modify file | Comment/line insertion and body edit | PASS |
| G06 | Detect before re-index | `check_index_coverage` → `stale`; snippet/outline → explicit errors | PASS |
| G07 | Rename file | `util.go` → `math.go` | PASS |
| G08 | Old/new references | Old QN gone; new QN present; callers resolve to new file | PASS |
| G09 | Delete file | `math.go` removed | PASS |
| G10 | Stale records | Add symbol gone; File nodes and call edges cleaned | PASS |
| G11 | Re-index consistency | File list, nodes, edges match clean rebuild for fixture | PASS |
| G12 | Restart | New MCP server process sees the project and updated graph | PASS |
| G13 | Same-size edit with preserved mtime | Coverage hash detects change (`stale`, not fresh) | PASS (after fix) |
| G14 | Concurrent async starts | Three starts join one job (`started_at` identical) | PASS |
| G15 | Queries during indexing | Old generation served; job reports running; no corruption | PASS |

## Group H — Error handling and resilience

| ID | Scenario | Actual | Result |
|---|---|---|---|
| H01 | Missing project | Actionable error with indexed projects | PASS |
| H02 | Invalid project | Same, no silent fallback | PASS |
| H03 | Missing source file | Snippet/outline/search_code explicit errors | PASS |
| H04 | Corrupt database | Server starts; tools return `storage error: file is not a database` | PASS |
| H05 | Invalid arguments | Schema errors for depth/limit; unknown args ignored by MCP (client validation) | PASS |
| H06 | Empty query | `search_code` rejects; `search_graph` without filters lists bounded nodes | PARTIAL (documented behavior) |
| H07 | Extremely long query | 10k pattern scans; 100k pattern returns budget fallback with hint | PASS |
| H08 | Large result limit | Whole-row truncation + `has_more`/next offset | PASS |
| H09 | Interrupted indexing | Persisted `running` status + `stale_status`; no synthetic success | PASS (re-run required) |
| H10 | Concurrent requests | 12 parallel MCP calls: 12/12 correct, 424 ms total | PASS |
| H11 | Server restart | Data/tools available; project selection stable | PASS |
| H12 | Unavailable repository root | `index_status` root_missing; source tools error; graph reads remain | PASS |

## Group I — Performance

| ID | Measurement | Actual | Result |
|---|---|---|---|
| I01 | Initial full index | 1,503 ms cold (495 files, 6,493 nodes, 28,715 edges) | PASS |
| I02 | Incremental index | 39–53 ms for one changed file (fixture) | PASS |
| I03 | Search latency | search_graph 30 ms; search_code 18 ms median (warm) | PASS |
| I04 | Symbol lookup | snippet 5 ms; outline 5 ms median | PASS |
| I05 | Graph query | trace 8 ms; architecture 59 ms; Cypher 58 ms | PASS |
| I06 | Context retrieval | get_task_context 12 ms median (warm) | PASS |
| I07 | Source retrieval | 4–5 ms; large snippet 4 ms | PASS |
| I08 | MCP response size | Deterministic budget; page metadata and truncation | PASS |
| I09 | Memory usage | 337 MB RSS after sustained use; 8 MB idle (no before baseline) | PARTIAL |
| I10 | Database growth | 274 MB shared across 7 projects; no longitudinal baseline | PARTIAL |

## Phase 5 — Agent usability scenarios

| ID | Scenario | Scores (accuracy / evidence / navigation / relationships / context / errors / usability) | Result |
|---|---|---|---|
| S1 | Repository understanding | 4 / 4 / 4 / 4 / 3 / 5 / 4 | PASS — `get_architecture` works; task-only context is noisy for broad questions |
| S2 | Feature discovery (product create/retrieve) | 4 / 4 / 5 / 3 / 4 / 5 / 4 | PARTIAL — flows correct; interface→implementation hop missing |
| S3 | Authentication dependencies | 4 / 4 / 4 / 3 / 3 / 5 / 4 | PARTIAL — interface/consumer evidence good; implicit implementation link missing |
| S4 | ProductStorage change impact | 4 / 3 / 4 / 3 / 3 / 5 / 4 | PARTIAL — consumers and changed symbols found; implementations not enumerated |
| S5 | DB initialization source | 5 / 5 / 5 / 4 / 5 / 5 / 5 | PASS |
| S6 | Uncertainty handling | 5 / 5 / 5 / 5 / 5 / 5 / 5 | PASS after fixes — ambiguity, missing symbols/files and stale source are explicit |
| S7 | Multi-step investigation | 4 / 4 / 4 / 4 / 4 / 5 / 4 | PASS — context remains project-consistent across calls |

## Baseline / final suite

| Suite | Baseline | Final |
|---|---|---|
| `cargo test --workspace` | 247 passed, 0 failed | 264 passed, 0 failed |
| `cargo fmt --all -- --check` | pass | pass |
| `cargo clippy ... -D warnings` | pass | pass |
| `cargo build --release` | pass | pass |

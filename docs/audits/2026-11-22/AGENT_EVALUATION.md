# GRY as an AI Coding-Agent Tool — Evaluation

**Date:** 2026-11-22
**Agent / MCP client:** OpenCode 2.3.2
**Server:** `gry serve` 1.0.4 (audited build)
**Target:** ecommerce Go repository (read-only)

## Method

Each scenario was executed as a multi-call MCP investigation, not as a single query. The agent used
discovery (`search_graph`, `get_architecture`), evidence (`trace_path`, `query_graph`), and source
retrieval (`get_code_snippet`, `get_file_outline`). Claims were checked against the repository with
native reads or `search_code`; a convincing answer was not accepted unless the evidence supported
it.

Scores (0–5) reflect the tool evidence available to an agent, not the prose quality of any model
answer. “Errors” measures whether unavailable/stale/ambiguous evidence is explicit.

## Scenario results

### S1 — Repository understanding

**Task:** explain the architecture, major components and dependency relationships.

- `get_architecture` returned languages (305 Go, 77 SQL, 73 templates, 19 Markdown), package count,
  entry points, layers and module/package dependencies.
- Source verification confirmed the layout: `cmd/web`, `cmd/cli`, bounded contexts
  (`auth`, `cart`, `checkout`, `fulfillment`, `payments`, `productcatalog`, `promo`, `repricing`,
  `reviews`, `search`, `shippinginfo`, `store`, `wishlist`), each with `domain/app/adapter` layers,
  and `internal/` infrastructure.
- A broad task-only `get_task_context(profile=scout)` selected weak targets
  (`Money.Display`, `newEventsCmd`) because generic words such as “architecture”, “component” and
  “dependency” match many nodes. `get_architecture` is the correct tool for this question.

Scores: accuracy 4, evidence 4, navigation 4, relationships 4, context 3, errors 5, usability 4.
**Overall 4/5.**

### S2 — Feature discovery (product creation and retrieval)

**Task:** find where product creation and retrieval are implemented and explain the flow.

- Creation: `AdminCreateProduct` and `AdminCreateVariantProduct` in
  `backend/layout/http_admin_products.go` → `catalogService.Add` /
  `catalogService.AddVariantProduct` → (on the concrete service)
  `ProductService.Add` / `AddVariantProduct` → `ProductStorage` methods.
- Retrieval: `httpHandler.Product` → `catalogService.Find` plus reviews/wishlist services and
  `renderTemplate`.
- `detect_changes base_branch=HEAD~2` identified `NewProduct` as the changed symbol and 12 impacted
  callers at hops 1–2.
- Limitation: the interface call does not automatically continue into the concrete adapter
  (`postgres`/`inMemory`) because Go implicit implementations are not linked.

Scores: accuracy 4, evidence 4, navigation 5, relationships 3, context 4, errors 5, usability 4.
**Overall 4/5.**

### S3 — Authentication dependencies

**Task:** identify the major dependencies of authentication and how they interact.

- `trace_path HandleLogin` → `authService.Login` (interface) and rate limiting; the concrete
  `auth.app.auth.auth.Login` trace → `CustomerStorage.Find`, `SessStorage.Store`, domain
  `NewCustomer`/`NewSession`, and metrics.
- Source verified that `authService` is the HTTP-facing port and `CustomerStorage`/`SessStorage`
  are persistence ports.
- Limitation: the interface → concrete implementation hop is not a graph edge, so the agent must
  know or discover the concrete type (`auth.auth`) before continuing. Source code confirms the
  relationship, but GRY does not enumerate it automatically.

Scores: accuracy 4, evidence 4, navigation 4, relationships 3, context 3, errors 5, usability 4.
**Overall 4/5, with the implementation-link limitation.**

### S4 — Change impact analysis

**Task:** if the product repository interface changes, which implementations and consumers are
affected?

- `ProductStorage` is now an indexed interface with 45 methods and five consumers; its method
  callers (service methods) are traceable.
- `detect_changes` correctly mapped the last two commits to `.gitignore` and
  `backend/productcatalog/domain/product.go`, identified `NewProduct` and 12 impacted symbols.
- Limitation: the concrete `postgres` and `inMemory` implementations are not enumerated as
  `IMPLEMENTS`; an agent must inspect candidate types/method sets. This is the most significant
  remaining gap for this scenario.

Scores: accuracy 4, evidence 3, navigation 4, relationships 3, context 3, errors 5, usability 4.
**Overall 4/5, with the implementation-enumeration limitation.**

### S5 — Targeted source investigation (database initialization)

**Task:** find database initialization and retrieve enough source.

- `search_code` found `otelsql.Open` in `cmd/web/main.go` and `cmd/cli/main.go`.
- `get_code_snippet` on `main.main` with `start_line=140` returned the connection setup:
  `connString`, `otelsql.Open("postgres", …)`, pool limits, `dependency.NewSQL(db)`.
- `get_graph_schema` diagnostics now exposes node/edge property names for follow-up Cypher.

Scores: accuracy 5, evidence 5, navigation 5, relationships 4, context 5, errors 5, usability 5.
**Overall 5/5.**

### S6 — Uncertainty handling

**Task:** ask about nonexistent symbols/files, ambiguous names and stale source.

Observed after fixes:

- Nonexistent symbol → `symbol not found` with a search hint.
- Ambiguous `NewProduct` → lists the two qualified names; no silent choice.
- Missing/unindexed file → explicit “not found in the index or on disk”.
- Stale file → explicit “changed since it was indexed … re-index”; never wrong source.
- Unavailable project root → explicit source-unavailable error; graph metadata remains readable.
- Explicit context target `NopeSymbolXyz` → `not_found`, not a plausible unrelated symbol.

Scores: 5 across accuracy, evidence, navigation, relationships, context, errors, usability.
**Overall 5/5.**

### S7 — Multi-step investigation

**Task:** investigate an unfamiliar subsystem progressively.

A representative sequence (product creation) used:

1. `get_task_context`/`search_graph` to find the handler.
2. `get_code_snippet` for the handler source.
3. `trace_path` outbound for the service/interface calls.
4. `search_graph`/`get_code_snippet` for the concrete service and adapter types.
5. `query_graph` for the route `HANDLES` edge and `detect_changes` for impact.
6. `check_index_coverage` to validate cited paths.

Project selection remained consistent across calls (default ecommerce; explicit root/descendant
also accepted). Evidence was bounded and the result reported truncation and inference. The missing
interface-implementation hop required manual type discovery but did not produce a false conclusion.

Scores: accuracy 4, evidence 4, navigation 4, relationships 4, context 4, errors 5, usability 4.
**Overall 4/5.**

## Usability observations

- **Tool selection matters.** `get_architecture` is the right entry point for system shape;
  `get_task_context` is strongest when targets are explicit.
- **Errors are actionable.** After the fixes, errors name the next step (re-index, pass a qualified
  name, use `search_code`), which prevents agents from guessing.
- **Truncation is usable.** Responses include totals, `has_more`, next offsets and continuation
  fields; the agent can page instead of silently losing evidence.
- **OpenCode signature rendering.** `search_code` displays as `search_code(unknown)` because its
  valid `anyOf` schema is not rendered as a parameter signature. The tool description and runtime
  still accept `pattern`/`query`, but this is a client-side usability limitation.
- **No fabricated success.** The most important improvement for agent trust is that unavailable or
  stale source and interrupted jobs no longer look like successful current evidence.

## Overall assessment

GRY is useful to an AI coding agent for real Go repository work, particularly discovery,
navigation, source retrieval, route handling, service call exploration and change impact on
concrete changes. Its conservative graph is preferable to a plausible but false one. The main
remaining agent-facing gap is enumerating implicit Go interface implementations, followed by
conservative dispatch through untyped locals; both are documented.

**Overall agent-tool rating: 4/5.**

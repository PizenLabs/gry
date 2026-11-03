# GRY agent-interoperability report — 2026-11-03

## 1. Surfaces tested

| Surface | How tested | Result |
|---|---|---|
| MCP over stdio | Direct JSON-RPC runs with the release binary; Rust contract suite | pass |
| CLI one-shot tools | `gry cli <tool>` / `gry <command>` integration tests | pass |
| OpenCode plugin | Node contract suite + live `opencode debug agents` | pass |
| OpenCode MCP client | Live service with isolated HOME/XDG dirs; log assertion `server=gry tools=18` | pass |
| Claude Code | Not installed/configured in this environment | not run |
| Codex | Not installed/configured in this environment | not run |

No claim is made for clients that were not executed.

## 2. Actual MCP contract

`initialize` advertises:

```json
{
  "serverInfo": {"name": "gry", "version": "1.0.3", "title": "gry code intelligence"},
  "capabilities": {"tools": {"listChanged": false}, "prompts": {"listChanged": false}},
  "_meta": {"toolProfile": "all"}
}
```

18 tools, in discovery order:

| Page | Tools |
|---|---|
| 1 | `index_repository`, `search_graph`, `query_graph`, `trace_path`, `get_code_snippet`, `get_file_outline` |
| 2 | `get_graph_schema`, `compare_graphs`, `get_architecture`, `search_code`, `list_projects`, `delete_project` |
| 3 | `index_status`, `check_index_coverage`, `get_task_context`, `detect_changes`, `manage_adr`, `ingest_traces` |

Every tool has a non-empty description, an object input schema, required keys declared in
`properties`, and annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`,
`openWorldHint`). Core tools also carry `_meta["anthropic/alwaysLoad"] = true`.

Tool profiles:

- `all`: all 18.
- `analysis`: read-only tools only.
- `scout`: fast/provisional subset including `get_task_context`, `search_graph`, `trace_path`,
  `get_code_snippet`, `get_file_outline`, `get_architecture`, `search_code`,
  `get_graph_schema`, `check_index_coverage`, `list_projects`, `index_status`.

### Project parameter

Read tools declare `project` optional with the description “defaults to the indexed workspace
root.” Resolution now accepts:

1. explicit `project`;
2. the last project used in the session, only while it still exists;
3. MCP roots (exact or canonical);
4. the exact working directory;
5. any indexed ancestor directory, deepest first, if unique;
6. the single indexed project, if exactly one exists.

If none applies, the error names the current workspace, up to 10 indexed projects with root
availability, and both recovery actions. A tie for the same root is an explicit ambiguity error,
not an arbitrary choice.

Write tools (`delete_project`, `manage_adr`, `ingest_traces`) require an explicit project in schema
and handler.

## 3. Reported OpenCode failures: cause and status

### 3.1 `search_graph` rejected because `project` was missing

**Observed:** the agent called `search_graph` without `project`; GRY returned
`invalid argument: project is required; call list_projects to see indexed projects`.

**Cause:** GRY-side default resolution. The session's working directory
(`.../PizenLabs/gry`) did not match the stored `gry` index root (`.../RnD/gry`), and the database
contained five projects, so no default could be chosen. The stored root no longer existed. The
error did not name any candidate, so the agent could not recover without another tool call.

**Not the cause:** client schema drift or stale metadata. The exposed schema correctly declared
`project` optional.

**Status:** fixed. Live reproduction after the change returns the workspace, the five projects with
`root missing` markers, and both recovery instructions. Descendant-directory resolution also now
works when the repository is actually indexed.

### 3.2 `search_code` rejected `query`

**Observed:** the agent sent `{"query": "..."}` to `search_code`; GRY returned
`missing required argument: pattern`.

**Cause:** GRY parameter naming. `search_graph` uses `query`, `search_code` used `pattern`, and no
alias was documented.

**Status:** fixed. `search_code` accepts `pattern` or `query`; the schema declares both and uses
`anyOf` to require one. `gry cli search_code --query ...` works through the same handler.

### 3.3 The agent did not call `list_projects`

`list_projects` is on page 2 of `tools/list`. The live OpenCode runtime loads all three pages
(`tools=18`), so it was discoverable. The failure was recovery behavior: the error told the agent to
call a tool instead of naming the available projects. The new error removes that dependency.

Pagination remains a risk for clients that do not follow `nextCursor`; that is documented in the
audit. No client was observed failing on it in this environment.

## 4. Live OpenCode results

Command:

```sh
GRY_OPENCODE_LIVE=1 cargo test -p gry --test opencode_runtime -- --nocapture
```

Result: 3 passed.

- `plugin_contract_suite_passes`: generated plugin contract suite.
- `plugin_loads_in_the_real_runtime`: OpenCode discovers the plugin; log contains
  `plugins/gry.js`; no “failed to load plugin.”
- `mcp_config_connects_in_the_real_runtime`: isolated config starts the gry binary; log contains
  `mcp connected` and `server=gry tools=18`.

The test uses isolated `HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_STATE_HOME`, a random
service port, and stops the service afterward.

## 5. Protocol-level workflows

The Rust contract suite (`crates/gry-mcp/tests/contract.rs`) exercises all 18 tools against a real
Git fixture: index, list, status, coverage, search, Cypher, trace, snippet, outline, schema, compare,
architecture, source search, context (scout/auditor/empty), change detection, ADR, trace ingestion
and deletion. It also covers:

- unknown tool and missing argument errors;
- unknown project errors with candidate names;
- ambiguous and omitted project behavior;
- `search_code` alias parity;
- empty context with `unknowns.code=no_targets`;
- prompt validation;
- profile visibility.

This is a protocol-level workflow, not an autonomous agent reasoning test. The agent-level behavior
was exercised through the live OpenCode connection and the failure reproductions above.

## 6. Tool-call overhead

- `initialize` + three `tools/list` pages: 20 ms wall time (3 runs), including process startup.
- Payload: 20,599 bytes for initialize + 3 pages.
- First page is 6 tools; `get_task_context` and `list_projects` require a second/third page.
- `get_task_context` is marked `alwaysLoad`, so clients that support the annotation can keep it
  available even when they paginate discovery.

## 7. Remaining integration limitations

- Claude Code and Codex were not executed; no live compatibility claim is made.
- Tool pagination is spec-compliant but optional for clients. A client that ignores `nextCursor`
  sees six tools. Raising the page size would reduce this risk; it was not changed because the
  live OpenCode client already loads all pages.
- MCP roots are read from `initialize` parameters. A server-initiated `roots/list` request is not
  implemented; cwd/ancestor resolution covers the common case.
- The live test verifies discovery and connection, not model reasoning or multi-turn recovery.
- Tool descriptions are bounded (schema test enforces ≤700 bytes each) but still consume context.

## 8. Recommended next steps

1. Add a page-size configuration or return all 18 tools when the count is below a threshold, then
   re-test against clients that ignore cursors.
2. Add an automated agent workflow test for a client that can run without credentials, covering
   the recovery path after a missing project.
3. Verify Claude Code and Codex adapters in an environment where those clients are installed.
4. Add a regression test that deletes a project between calls and asserts that the stale
   `last_project` is ignored.

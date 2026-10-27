# Agent integrations

gry provides read-only repository evidence. It never approves an edit,
authorizes a tool, or executes a mutation. This page describes what gry
installs, what each runtime actually supports, and how to inspect or remove it.

## Responsibilities

| Component | Owns |
|---|---|
| gry | Indexing, graph queries, evidence freshness/provenance, MCP tools, the `gry hook` CLI |
| The agent runtime | Lifecycle, planning, tools, approvals, permissions, execution, completion |
| The gry installer | The gry binary, gry's MCP entry, and (opt-in) the gry OpenCode plugin |

The GRY installer configures only entries it writes. Agent configuration paths,
formats, and merge rules are runtime-specific and are never shared through one
generic writer.

## Supported runtimes

Verified on 2026-10-27 against the versions shown.

| Runtime | MCP | Native lifecycle hook | Events | Verification level |
|---|---|---|---|---|
| OpenCode 2.1.4 | Yes, `mcp.servers.<name>` (`local`, stdio) | Yes, a local V2 plugin | `before_plan`, `before_edit`, `after_edit`; `before_finish` **not available** | Plugin loaded by the real runtime; MCP server connected and listed 18 tools; hook callbacks covered by a fake-gry contract suite |
| OHGO (headless) | Yes, `mcp.servers[]` | Yes, opt-in subprocess adapter in OHGO | `before_plan`, `before_edit`, `after_edit`, `before_finish` | OHGO's own tests and the existing controlled end-to-end run; see OHGO `docs/gry-hooks.md` |

Everything else in `gry install --list-clients` is MCP-only. Do not read that
list as native-hook support.

## Install

The one-line installer installs the binary and configures MCP for detected
harnesses. Native hooks are **opt-in**:

```bash
# MCP only (the existing default)
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash

# MCP + the OpenCode V2 lifecycle plugin
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh \
  | bash -s -- --client opencode --hooks

# Inspect what gry owns
curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh \
  | bash -s -- --status
```

The equivalent CLI commands are:

```bash
gry install --client opencode --hooks
gry integrations --verify
gry uninstall --client opencode
```

`--hooks` is currently supported for OpenCode only. In a multi-client install
(`--client auto` or `all`) the other clients still receive MCP configuration
and report `hooks: unsupported`; a single-client install fails instead of
pretending the hook was installed.

The plugin is written to
`$XDG_CONFIG_HOME/opencode/plugins/gry.js` (default
`~/.config/opencode/plugins/gry.js`) with the absolute gry binary path
embedded. OpenCode discovers it automatically; no `plugins` array entry is
needed.

## OpenCode events

| Logical event | OpenCode mechanism | Behavior |
|---|---|---|
| `before_plan` | `session.context` on the first model request of a session | Runs `gry hook before_plan`, injects the envelope into the system context |
| `before_edit` | `tool.execute.before` for `edit`, `write`, `patch` | Runs `gry hook before_edit` with repository-relative paths |
| `after_edit` | `tool.execute.after` when the edit succeeded | Runs `gry hook after_edit`; evidence is attached to the next `session.context` |
| `before_finish` | none | Not implemented. OpenCode V2 exposes no finish hook; the adapter does not map it to an unrelated event. |

The plugin does **not** observe `shell` as an edit event: a shell command may
mutate files, but its target cannot be established from the tool input. Edits
performed only through `shell` therefore do not refresh evidence. This is a
documented gap, not a silent claim.

OpenCode may call `session.context` before every model request. The adapter
memoizes `before_plan` per prompt/session and deduplicates identical edit
events, so evidence is not re-injected or re-fetched on every turn. A failed
hook is cached for 30 seconds per event to avoid a retry storm; the failure is
rendered explicitly rather than treated as empty success.

## Evidence envelope

The adapter injects the `gry hook` envelope unchanged, bounded by
`GRY_MAX_TOKENS` (default 4000) and a 12,000-character presentation cap:

```xml
<gry_evidence stage="before_plan" trust="untrusted-repository-evidence">
{"hook":{...},"context":{...},"failure":null}
</gry_evidence>
```

`<` in the JSON is escaped, so repository text cannot close the tag or forge a
system instruction. The text states that the payload is evidence, not
permission. `status`, `evidence.complete_for_scope`,
`evidence.repository_scan`, freshness reasons, provenance, and failures are
preserved. A bounded result never becomes a repository-wide completeness claim.

## Configuration

The plugin reads the environment of the OpenCode server process:

| Variable | Default | Meaning |
|---|---|---|
| `GRY_BIN` | absolute path embedded at install | Binary to invoke |
| `GRY_PROJECT` | `auto` | GRY project name, or `auto` |
| `GRY_OPENCODE_HOOK` | `1` | `0` disables the plugin without removing it |
| `GRY_TIMEOUT_MS` | `1500` | Per-hook timeout (100–30000) |
| `GRY_MAX_TOKENS` | `4000` | Evidence budget (128–1000000) |
| `GRY_OPENCODE_DEBUG` | unset | One bounded status line per observation |

`GRY_PROJECT=auto` resolves the indexed project whose root contains OpenCode's
working directory. It fails closed when no indexed project contains it; it
never borrows another project's evidence.

## Status, upgrade, and uninstall

`gry integrations --verify` reads `~/.config/gry/integrations.json` (the
gry-owned manifest), checks the MCP entry shape and the plugin hash, and reports
`installed`, `missing`, `modified`, `conflict`, `malformed`, or `unreadable`.
`--verify` re-hashes the plugin; without it, only presence is reported.

- Reinstalling converges: an unchanged plugin is not rewritten and the MCP
  entry is replaced in place.
- Upgrading an unmodified managed plugin overwrites it.
- A plugin edited by the user is a conflict. Use `--force` only after reviewing
  the diff.
- Uninstall removes the plugin only when its hash matches the manifest, and the
  MCP entry only when it still has the gry shape. Unrelated keys, servers, and
  user hooks are preserved. A malformed or foreign entry fails with instructions
  and leaves the file unchanged.

Backups are written next to a modified JSON/TOML configuration as
`<name>.gry-bak` before the atomic write. Manifest writes are atomic and the
temporary file is created with mode `0600`.

## Security

- `spawn`/`Command` with an argument vector; no shell and no string
  interpolation of task text, paths, or repository content.
- The child receives only `PATH`, `HOME`, `TMPDIR`, locale, `GRY_DB`, and
  `GRY_CACHE_DIR`. Provider credentials and unrelated secrets are not inherited.
- stdout is capped at 128 KiB and stderr at 4 KiB; timeouts kill the process.
- Paths are made relative to the project, and absolute paths outside it,
  `..` segments, NUL bytes, and oversized values are rejected.
- Diagnostics never print evidence bodies, prompts, or environment values.
- The generated config is written atomically; symlinked plugin targets are
  refused.
- gry output cannot grant permission. OpenCode's permission hooks, approvals,
  and tool execution remain authoritative.

## Troubleshooting

| Symptom | Check |
|---|---|
| `before_finish` never fires in OpenCode | Expected: no finish hook exists. Use OHGO for the full four-stage lifecycle. |
| Evidence says `not_indexed` | Index the project: `gry index . --name <project>`, then set `GRY_PROJECT` if auto-detection is ambiguous. |
| Evidence is `stale` after an edit | Re-index or wait for the watcher. A successful hook is not a completed reindex. |
| Plugin not loaded | Run `opencode debug agents` and inspect `~/.local/share/opencode/log/opencode.log` for `plugins/gry.js` and `failed to load plugin`. |
| `modified` in `gry integrations --verify` | The user edited the managed plugin. Review, then reinstall with `--force` or remove it. |
| MCP server missing | `gry integrations --verify`; for OpenCode the entry must be under `mcp.servers.gry`, not the deprecated `mcp.gry`. |

## Local tests

```bash
bash scripts/install_test.sh                        # deterministic installer fixtures
node --test crates/gry-cli/tests/opencode_plugin_test.mjs
cargo test -p gry --test opencode_runtime
GRY_OPENCODE_LIVE=1 cargo test -p gry --test opencode_runtime -- --nocapture
```

The last command starts a real OpenCode service in isolated `HOME`/XDG
directories, asserts the plugin loads, and, in the MCP test, asserts the server
connects with its tool catalog. It never reads or writes the user's real
configuration.

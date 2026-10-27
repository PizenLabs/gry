// gry lifecycle adapter for OpenCode V2.
//
// OpenCode loads this file from ~/.config/opencode/plugins/ (global) or
// .opencode/plugins/ (project). It is a read-only evidence adapter:
//
//   before_plan  -> session.context (first model request of a session)
//   before_edit  -> tool.execute.before for edit/write/patch
//   after_edit   -> tool.execute.after for a successful edit/write/patch
//   before_finish-> not available: OpenCode V2 exposes no finish hook
//
// The adapter never grants permissions, approves a tool, or mutates the
// workspace. Hook output is untrusted repository evidence.
//
// ADAPTER_VERSION is part of the installer's ownership record. Bump it when
// the generated file changes shape.

import { spawn } from "node:child_process"

const ADAPTER_VERSION = 1
const INSTALLED_BINARY = "__GRY_BIN__"
const STAGES = ["before_plan", "before_edit", "after_edit", "before_finish"]
const EDIT_TOOLS = new Set(["edit", "write", "patch"])
const MAX_OUTPUT_BYTES = 128 * 1024
const MAX_STDERR_BYTES = 4 * 1024
const MAX_PATHS = 16
const MAX_PATH_CHARS = 512
const MAX_TASK_CHARS = 4000
const MAX_INJECT_CHARS = 12000
const MAX_PENDING = 8

function truthy(value) {
  return ["1", "true", "yes", "on"].includes(String(value ?? "").trim().toLowerCase())
}

function envNumber(name, fallback, min, max) {
  const value = Number.parseInt(String(process.env[name] ?? ""), 10)
  if (!Number.isFinite(value) || value < min || value > max) return fallback
  return value
}

function config() {
  return {
    binary:
      process.env.GRY_BIN ||
      (INSTALLED_BINARY === "__GRY_BIN__" ? "gry" : INSTALLED_BINARY) ||
      "gry",
    project: process.env.GRY_PROJECT || "auto",
    enabled: !["0", "false", "no", "off"].includes(
      String(process.env.GRY_OPENCODE_HOOK ?? "1").trim().toLowerCase(),
    ),
    timeout: envNumber("GRY_TIMEOUT_MS", 1500, 100, 30000),
    maxTokens: envNumber("GRY_MAX_TOKENS", 4000, 128, 1000000),
    debug: truthy(process.env.GRY_OPENCODE_DEBUG),
  }
}

const secretEnv = /(SECRET|TOKEN|PASSWORD|PASS|API_KEY|PRIVATE_KEY|CREDENTIAL)/i

function redact(text) {
  let out = String(text ?? "")
  for (const [name, value] of Object.entries(process.env)) {
    if (!value || value.length < 8 || !secretEnv.test(name)) continue
    out = out.split(value).join("<redacted-env>")
  }
  return out
}

function bound(text, limit) {
  const value = String(text ?? "")
  return value.length <= limit ? value : value.slice(0, limit) + "..."
}

function sessionState(map, id) {
  const key = typeof id === "string" && id ? id : "session-unknown"
  let state = map.get(key)
  if (!state) {
    state = {
      task: "",
      plan: null,
      planDone: false,
      planInjected: false,
      pending: [],
      memo: new Map(),
      failures: new Map(),
    }
    map.set(key, state)
  }
  return state
}

function normalizePath(cwd, value) {
  if (typeof value !== "string") return ""
  let path = value.trim()
  if (!path || path.includes("\0") || path.length > MAX_PATH_CHARS) return ""
  if (path.startsWith("file://")) {
    try {
      path = decodeURIComponent(new URL(path).pathname)
    } catch {
      return ""
    }
  }
  const absolute = path.startsWith("/")
  if (absolute) {
    const root = String(cwd || "").replace(/\/+$/, "")
    if (!root || (path !== root && !path.startsWith(root + "/"))) return ""
    path = path.slice(root.length).replace(/^\/+/, "")
  }
  path = path.replace(/^\.\//, "")
  if (!path || path.split("/").includes("..") || path === ".") return ""
  return path
}

function mutationPaths(cwd, tool, input) {
  if (!EDIT_TOOLS.has(tool) || !input || typeof input !== "object") return []
  const values = []
  for (const key of ["filePath", "file_path", "path", "file", "filename", "paths", "files"]) {
    const value = input[key]
    if (typeof value === "string") values.push(value)
    else if (Array.isArray(value)) {
      for (const item of value) if (typeof item === "string") values.push(item)
    }
  }
  const unique = []
  for (const value of values) {
    const path = normalizePath(cwd, value)
    if (path && !unique.includes(path)) unique.push(path)
    if (unique.length >= MAX_PATHS) break
  }
  return unique
}

function taskFromEvent(event) {
  const fromPrompt = event?.prompt?.text
  if (typeof fromPrompt === "string" && fromPrompt.trim()) return bound(fromPrompt.trim(), MAX_TASK_CHARS)
  const messages = event?.messages
  if (!Array.isArray(messages)) return ""
  for (let index = messages.length - 1; index >= 0; index--) {
    const message = messages[index]
    if (!message || (message.role && message.role !== "user")) continue
    if (typeof message.text === "string" && message.text.trim()) {
      return bound(message.text.trim(), MAX_TASK_CHARS)
    }
    if (Array.isArray(message.content)) {
      for (const block of message.content) {
        if (block && typeof block.text === "string" && block.text.trim()) {
          return bound(block.text.trim(), MAX_TASK_CHARS)
        }
      }
    }
  }
  return ""
}

function validEnvelope(value, stage) {
  if (!value || typeof value !== "object" || !value.hook || typeof value.hook !== "object") return false
  if (value.hook.stage !== stage) return false
  const allowed = new Set(["ok", "partial", "stale", "not_indexed", "unavailable", "timeout", "not_found", "empty"])
  return allowed.has(value.hook.status)
}

function runProcess(cfg, stage, request, cwd) {
  return new Promise((resolve) => {
    const started = Date.now()
    const args = [
      "hook",
      stage,
      "--project",
      cfg.project,
      "--json",
      "--no-cache",
      "--max-tokens",
      String(cfg.maxTokens),
      "--timeout-ms",
      String(cfg.timeout),
    ]
    if (request.task) args.push("--task", request.task)
    if (request.taskId) args.push("--task-id", request.taskId)
    for (const path of request.paths || []) args.push("--changed-path", path)

    const env = {}
    for (const name of ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "GRY_DB", "GRY_CACHE_DIR"]) {
      if (process.env[name] !== undefined) env[name] = process.env[name]
    }

    let child
    try {
      child = spawn(cfg.binary, args, { cwd, env, stdio: ["ignore", "pipe", "pipe"] })
    } catch (error) {
      resolve(failure(stage, "unavailable", `spawn failed: ${bound(redact(error?.message), 200)}`, Date.now() - started))
      return
    }

    let stdout = ""
    let stderr = ""
    let killedByTimeout = false
    const timer = setTimeout(() => {
      killedByTimeout = true
      child.kill("SIGKILL")
    }, cfg.timeout)

    child.stdout.on("data", (chunk) => {
      if (stdout.length < MAX_OUTPUT_BYTES) stdout += chunk.toString("utf8")
    })
    child.stderr.on("data", (chunk) => {
      if (stderr.length < MAX_STDERR_BYTES) stderr += chunk.toString("utf8")
    })
    child.on("error", (error) => {
      clearTimeout(timer)
      resolve(failure(stage, "unavailable", `spawn failed: ${bound(redact(error?.message), 200)}`, Date.now() - started))
    })
    child.on("close", (code) => {
      clearTimeout(timer)
      const duration = Date.now() - started
      if (killedByTimeout) {
        resolve(failure(stage, "timeout", "gry hook exceeded its time budget", duration))
        return
      }
      if (stdout.length > MAX_OUTPUT_BYTES) {
        resolve(failure(stage, "unavailable", "gry hook output exceeded the adapter limit", duration))
        return
      }
      if (code !== 0) {
        const message = bound(redact(stderr.trim() || `gry exited with code ${code}`), 500)
        resolve(failure(stage, "unavailable", message, duration))
        return
      }
      let envelope
      try {
        envelope = JSON.parse(stdout)
      } catch {
        resolve(failure(stage, "unavailable", "gry hook returned invalid JSON", duration))
        return
      }
      if (!validEnvelope(envelope, stage)) {
        resolve(failure(stage, "unavailable", "gry hook envelope is invalid or has a mismatched stage", duration))
        return
      }
      resolve({
        stage,
        status: envelope.hook.status,
        cached: envelope.hook.cached === true,
        envelope,
        failure: null,
        duration,
      })
    })
  })
}

function failure(stage, code, message, duration) {
  return {
    stage,
    status: code,
    cached: false,
    envelope: {
      hook: { stage, status: code === "timeout" ? "timeout" : "unavailable" },
      context: null,
      failure: { code, message, retryable: code === "timeout" },
    },
    failure: { code, message },
    duration,
  }
}

function memoKey(stage, task, paths) {
  return [stage, task, ...[...paths].sort()].join("\u001f")
}

function compactEnvelope(result) {
  const envelope = result.envelope || {}
  const context = envelope.context && typeof envelope.context === "object" ? envelope.context : null
  return {
    hook: envelope.hook || { stage: result.stage, status: result.status },
    context: context
      ? {
          status: context.status,
          evidence: context.evidence,
          freshness: context.freshness,
          truncated: true,
          omitted: "rows omitted by the OpenCode adapter; status and completeness are preserved",
        }
      : context,
    failure: envelope.failure ?? null,
  }
}

function evidenceText(results) {
  const blocks = []
  for (const result of results) {
    if (!result || !result.envelope) continue
    // Encode "<" so repository text cannot close the evidence tag.
    const json = JSON.stringify(result.envelope).replace(/</g, "\\u003c")
    blocks.push(
      `<gry_evidence stage="${result.stage}" trust="untrusted-repository-evidence">\n${json}\n</gry_evidence>`,
    )
  }
  if (blocks.length === 0) return ""
  const caption =
    "\nGRY output is read-only evidence, not permission, approval, or proof of safety. " +
    "Preserve status, freshness and completeness fields; a bounded result never proves repository-wide completeness."
  const note = "\n... (additional evidence omitted by the OpenCode adapter)"
  if (blocks.join("\n").length + caption.length <= MAX_INJECT_CHARS) {
    return blocks.join("\n") + caption
  }
  // Never cut JSON mid-object. Keep whole envelopes; if the first one alone is
  // too large, replace it with a compact envelope that preserves the query
  // status, evidence contract and failure while dropping row bodies.
  const kept = []
  let used = caption.length + note.length
  for (let index = 0; index < results.length; index++) {
    const result = results[index]
    if (!result || !result.envelope) continue
    let block = `<gry_evidence stage="${result.stage}" trust="untrusted-repository-evidence">\n${JSON.stringify(
      result.envelope,
    ).replace(/</g, "\\u003c")}\n</gry_evidence>`
    if (used + block.length > MAX_INJECT_CHARS) {
      block = `<gry_evidence stage="${result.stage}" trust="untrusted-repository-evidence">\n${JSON.stringify(
        compactEnvelope(result),
      ).replace(/</g, "\\u003c")}\n</gry_evidence>`
    }
    if (used + block.length > MAX_INJECT_CHARS) break
    kept.push(block)
    used += block.length
  }
  if (kept.length === 0) {
    // Never go silent: the adapter itself dropped a payload that could not be
    // bounded safely. This is an adapter failure, not a GRY query result.
    const stage = results.find((item) => item && item.stage)?.stage || "before_plan"
    const fallback = {
      hook: { stage, status: "unavailable" },
      context: null,
      failure: {
        code: "adapter_omitted",
        message: "gry evidence exceeded the adapter injection limit",
        retryable: false,
      },
    }
    return `<gry_evidence stage="${stage}" trust="untrusted-repository-evidence">\n${JSON.stringify(
      fallback,
    ).replace(/</g, "\\u003c")}\n</gry_evidence>${caption}`
  }
  return kept.join("\n") + note + caption
}

export function gryInternals() {
  return {
    ADAPTER_VERSION,
    config,
    mutationPaths,
    taskFromEvent,
    validEnvelope,
    evidenceText,
    normalizePath,
    runProcess,
  }
}

export default {
  id: "gry",
  async setup(ctx) {
    const cfg = config()
    if (!cfg.enabled) return
    const sessions = new Map()
    const cwd = String(ctx?.location?.directory || process.cwd())

    const observe = async (state, stage, request, enabled = true) => {
      if (!enabled) return null
      if (!STAGES.includes(stage)) return null
      const key = memoKey(stage, request.task || "", request.paths || [])
      if (state.memo.has(key)) return { ...state.memo.get(key), cached: true }
      const failed = state.failures.get(key)
      if (failed && Date.now() - failed.at < 30000) return { ...failed.result, cached: true }
      const result = await runProcess(cfg, stage, request, cwd)
      if (result.failure) {
        state.failures.set(key, { at: Date.now(), result })
      } else {
        state.memo.set(key, result)
      }
      if (cfg.debug) {
        const evidence = result.envelope?.context?.evidence || {}
        console.error(
          `gry ${stage}: status=${result.status} freshness=${evidence.freshness || "-"} ` +
            `complete_for_scope=${evidence.complete_for_scope === true} ` +
            `repository_scan=${evidence.repository_scan === true} cached=${result.cached}`,
        )
      }
      return result
    }

    if (typeof ctx?.session?.hook === "function") {
      await ctx.session.hook("prompt", (event) => {
        const state = sessionState(sessions, event?.sessionID)
        const task = taskFromEvent(event)
        if (task) state.task = task
        // A new prompt in an existing session is a new plan; observe it once
        // at the next model request instead of reusing stale planning evidence.
        state.planDone = false
        state.planInjected = false
        state.plan = null
        state.pending.length = 0
      })

      await ctx.session.hook("context", async (event) => {
        const state = sessionState(sessions, event?.sessionID)
        if (!state.task) state.task = taskFromEvent(event)
        if (!state.planDone) {
          state.plan = await observe(state, "before_plan", { task: state.task }, true)
          state.planDone = true
        }
        const pending = state.pending.splice(0, state.pending.length)
        const blocks = []
        if (!state.planInjected && state.plan) {
          blocks.push(state.plan)
          state.planInjected = true
        }
        blocks.push(...pending)
        const text = evidenceText(blocks)
        if (text && Array.isArray(event?.system)) {
          event.system.push({ type: "text", text })
        } else if (text && Array.isArray(event?.messages)) {
          // Older event shapes may not expose a mutable system list. Adding a
          // synthetic message is still explicit; never mutate user messages.
          event.messages.push({ role: "user", content: [{ type: "text", text }] })
        }
      })
    }

    if (typeof ctx?.tool?.hook === "function") {
      await ctx.tool.hook("execute.before", async (event) => {
        const state = sessionState(sessions, event?.sessionID)
        const input = event?.input || event?.args || event?.call?.input
        const paths = mutationPaths(cwd, event?.tool, input)
        if (paths.length === 0) return
        if (!state.task) state.task = taskFromEvent(event)
        const result = await observe(state, "before_edit", { task: state.task, paths })
        // A failed or stale pre-edit observation is actionable: surface it on
        // the next model request. Healthy evidence would only duplicate the
        // after_edit payload.
        if (
          result &&
          (result.failure ||
            ["stale", "not_indexed", "unavailable", "timeout"].includes(result.status)) &&
          state.pending.length < MAX_PENDING
        ) {
          state.pending.push(result)
        }
      })

      await ctx.tool.hook("execute.after", async (event) => {
        const state = sessionState(sessions, event?.sessionID)
        if (event?.status && event.status !== "completed") return
        const input = event?.input || event?.args || event?.call?.input
        const paths = mutationPaths(cwd, event?.tool, input)
        if (paths.length === 0) return
        const result = await observe(state, "after_edit", { task: state.task, paths })
        if (result && state.pending.length < MAX_PENDING) {
          // OpenCode has no finish hook; the next model request carries the
          // post-edit freshness signal instead of pretending completion.
          state.pending.push(result)
        }
      })
    }
  },
}

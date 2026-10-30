// impeccable-design-hook: the Impeccable design detector as an OpenCode plugin.
//
// OpenCode has no command-hook manifest, so this plugin feeds the engine's
// ``hook`` verb the Claude-shaped events it already understands, and appends the
// findings to the tool result the model reads next. The engine owns everything
// else (UI-file filtering, ``hook.enabled``, dedupe between passes, ignore
// state).
//
// One file serves both plugin APIs, because each loader accepts a different
// shape of the same object:
//
// - v1 calls ``server(input)`` and uses the v1 hook names; its loader takes any
//   export that is a function or ``{ server() }``. The edit and write tools pass
//   the file as ``args.filePath``.
// - v2 calls ``setup(ctx)`` and takes a default export of ``{ id, setup }``; the
//   tools pass it as ``input.path`` and the event carries ``status``/``result``.
//
// v2 ignores the extra ``server`` key and v1 ignores ``setup``, so the default
// export is one object both loaders accept. Nothing else is exported: an older
// v1 loader calls *every* export as a plugin, so a helper export would be one.
//
// There is no Stop counterpart in either version. OpenCode has no pre-finish
// gate that can keep a turn going; the closest surface, ``session.idle``, can
// only start a new turn, which would make the agent loop self-driving. The
// per-edit pass is the whole hook here.
//
// Fail-open: any error, timeout, or missing launcher leaves OpenCode untouched.

import { execFileSync } from "node:child_process"
import { existsSync } from "node:fs"
import { dirname, extname, join, resolve } from "node:path"
import { homedir } from "node:os"

const ID = "impeccable-design-hook"
const EDIT_TOOLS = new Set(["edit", "write", "patch", "multiedit", "apply_patch", "create"])
// The engine decides whether a file is UI; this list only spares it a process
// spawn for edits that provably are not markup or stylesheet.
const UI_EXT = new Set([
  ".html", ".htm", ".css", ".scss", ".sass", ".less",
  ".jsx", ".tsx", ".vue", ".svelte", ".astro", ".mdx", ".marko", ".elm",
])
const TIMEOUT_MS = 4000
const LAUNCHER = process.platform === "win32" ? "impeccable.cmd" : "impeccable"
// A project install is the pinned version for that repo, so it wins over the
// user-level one; the fallbacks are the layouts the installer and the manual
// copy paths produce.
const PROJECT_DIRS = [".opencode", ".agents", ".claude", ".codex", ".pi"]
const GLOBAL_DIRS = [
  join(homedir(), ".agents"),
  join(homedir(), ".config", "opencode"),
  join(homedir(), ".claude"),
]

// Token budget. Every byte appended to a tool result stays in the transcript
// and is re-read on each later turn, so a session that edits twenty UI files
// must not re-inject the same reminder twenty times. The engine already caps
// its own repeated nudges per file; this bounds the transcript as a whole.
const LIMITS = { messages: 12, bytes: 12_000, sessions: 100 }
const memory = new Map()
const DISABLED = new Set(["1", "true", "yes", "on"])

function launcherIn(base) {
  const candidate = join(base, "skills", "impeccable", "scripts", LAUNCHER)
  return existsSync(candidate) ? candidate : null
}

function findLauncher(fromFile) {
  let current = dirname(resolve(fromFile))
  for (;;) {
    for (const dir of PROJECT_DIRS) {
      const found = launcherIn(join(current, dir))
      if (found) return found
    }
    const parent = dirname(current)
    if (parent === current) break
    current = parent
  }
  for (const dir of GLOBAL_DIRS) {
    const found = launcherIn(dir)
    if (found) return found
  }
  return null
}

function editedPath(input) {
  if (!input || typeof input !== "object") return null
  // v2's built-in edit and write take `path`; v1's take `filePath`. The rest are
  // third-party spellings, kept so a renamed tool still gets scanned.
  const raw = input.path ?? input.filePath ?? input.file_path ?? input.file
  return typeof raw === "string" && raw ? raw : null
}

function disabled() {
  return DISABLED.has(String(process.env.IMPECCABLE_HOOK_DISABLED ?? "").toLowerCase())
}

/** The findings text for one edit, or "" when the engine has nothing to say. */
function findings(filePath, cwd, sessionID, tool) {
  if (!filePath || !UI_EXT.has(extname(filePath).toLowerCase())) return ""
  const target = resolve(cwd, filePath)
  const launcher = findLauncher(target)
  if (!launcher) return ""
  const payload = {
    hook_event_name: "PostToolUse",
    cwd,
    tool_name: String(tool) === "write" ? "Write" : "Edit",
    tool_input: { file_path: target },
    session_id: String(sessionID ?? ""),
  }
  const stdout = execFileSync(launcher, ["hook"], {
    input: JSON.stringify(payload),
    encoding: "utf8",
    timeout: TIMEOUT_MS,
    maxBuffer: 1 << 20,
    cwd,
    // The event and reply shapes this plugin speaks are Claude's, which is the
    // engine's base shape; naming it keeps the engine from guessing.
    env: { ...process.env, IMPECCABLE_HOOK_HARNESS: "claude" },
    stdio: ["pipe", "pipe", "ignore"],
  })
  const text = stdout?.trim()
  if (!text) return ""
  const context = JSON.parse(text)?.hookSpecificOutput?.additionalContext
  if (typeof context !== "string" || !context) return ""
  // A clean scan is reassurance, not a finding. In a harness whose hook output
  // is a tool result, appending it to every edit spends context on nothing.
  if (context.includes("No deterministic design-quality issues found")) return ""
  return context
}

function budget(sessionID) {
  let entry = memory.get(sessionID)
  if (!entry) {
    entry = { seen: new Set(), bytes: 0, messages: 0 }
    memory.set(sessionID, entry)
    if (memory.size > LIMITS.sessions) memory.delete(memory.keys().next().value)
  }
  return entry
}

/** True when this exact text is new to the session and the session can afford it. */
function admits(sessionID, text) {
  const entry = budget(sessionID)
  if (entry.seen.has(text)) return false
  if (entry.messages >= LIMITS.messages) return false
  if (entry.bytes + text.length > LIMITS.bytes) return false
  entry.seen.add(text)
  entry.messages += 1
  entry.bytes += text.length
  return true
}

/** The v2 event: `execute.after` on a `{ tool, input, result, status }` object. */
async function handleV2(event, cwd) {
  try {
    if (disabled()) return
    if (!event || event.status !== "completed") return
    if (!EDIT_TOOLS.has(String(event.tool))) return
    const text = findings(editedPath(event.input), cwd ?? process.cwd(), event.sessionID, event.tool)
    if (!text || !admits(String(event.sessionID ?? ""), text)) return
    const result = { ...(event.result ?? {}) }
    if (typeof result.content === "string") result.content = `${result.content}\n\n${text}`
    else if (Array.isArray(result.content)) result.content = [...result.content, { type: "text", text }]
    else if (typeof result.output === "string") result.output = `${result.output}\n\n${text}`
    else result.content = text
    event.result = result
  } catch {
    // Fail-open by design: see the header.
  }
}

/** The v1 hook: `tool.execute.after` on `(input, output)` with `input.args`. */
async function handleV1(event, output, cwd) {
  try {
    if (disabled()) return
    if (!event || !EDIT_TOOLS.has(String(event.tool))) return
    const text = findings(editedPath(event.args), cwd ?? process.cwd(), event.sessionID, event.tool)
    if (!text || !admits(String(event.sessionID ?? ""), text)) return
    if (output && typeof output.output === "string") output.output = `${output.output}\n\n${text}`
    else if (output) output.output = text
  } catch {
    // Fail-open by design: see the header.
  }
}

// The default export both loaders read. v1 calls `server`, v2 calls `setup`.
const plugin = {
  id: ID,

  async server(input) {
    const cwd = input?.directory || process.cwd()
    return {
      "tool.execute.after": (event, output) => handleV1(event, output, cwd),
    }
  },

  async setup(ctx) {
    const cwd = ctx?.location?.directory || process.cwd()
    const registration = await ctx.tool.hook("execute.after", (event) => handleV2(event, cwd))
    return () => registration?.dispose?.()
  },
}

export default plugin

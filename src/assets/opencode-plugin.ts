import { Plugin } from "@opencode/plugin"
import { appendFileSync, closeSync, fchmodSync, mkdirSync, openSync, renameSync, statSync, writeSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { createHash } from "node:crypto"

const DBG = process.env.SKILL_GRAPH_DEBUG === "1" ? "/tmp/sg-oc-plugin.log" : ""
const dbg = (m: string) => { if (DBG) try { appendFileSync(DBG, m + "\n") } catch {} }

// skill-graph adapter for OpenCode.
//
// Emits Skill Usage Events (contracts/v1/skill-usage-event.schema.json) to a
// JSONL log and injects recommendations via the `context` hook, mirroring the
// shell-hook adapters. Detection (`tool.hook("execute.after")`) and
// recommendation (`session.hook("prompt")`) are separate paths: non-skill
// tool calls return before any CLI invocation, and no command lines, tool
// responses, or prompt bodies are persisted — prompts are SHA-256 hashed.
//
// UNVERIFIED fields are read defensively (optional chaining); the plugin
// fails open on any error.

const SKILL_GRAPH_BIN = process.env.SKILL_GRAPH_BIN ?? "skill-graph"
const EVENT_LOG =
  process.env.SKILL_GRAPH_EVENT_LOG ?? join(tmpdir(), "skill-graph-events.jsonl")
// Parity with the Rust hook: SKILL_GRAPH_HOOK_NOSTATE=1 suppresses all
// event writes; SKILL_GRAPH_EVENT_LOG_MAX bounds the log (rotate to .1).
const NOSTATE = process.env.SKILL_GRAPH_HOOK_NOSTATE === "1"
// Mirror the Rust hook's env parsing: positive integers only, anything
// else falls back to the default.
function envUint(name: string, fallback: number): number {
  const n = Number(process.env[name])
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : fallback
}
const EVENT_LOG_MAX = envUint("SKILL_GRAPH_EVENT_LOG_MAX", 64 * 1024 * 1024)
// Bound in-memory state: this plugin lives for the whole OpenCode server
// process, so unbounded maps would grow forever.
const MAX_EMITTED = 10_000
const MAX_PENDING_SESSIONS = 200
const MAX_SESSION_CTX = 200
const MAX_SESSION_NAMES = 32
const emitted = new Set<string>()
const sessionCtx = new Map<string, string[]>()

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex")
}

// Mirror parser::valid_skill_name — runtime-supplied names are untrusted
// and must never reach the event contract verbatim.
function validSkillName(name: string): boolean {
  return /^[a-z0-9-]{1,64}$/.test(name) && !name.startsWith("-") && !name.endsWith("-")
}

function emit(event: Record<string, unknown>) {
  if (NOSTATE) return
  try {
    mkdirSync(dirname(EVENT_LOG), { recursive: true })
    try {
      if (statSync(EVENT_LOG).size > EVENT_LOG_MAX) {
        renameSync(EVENT_LOG, EVENT_LOG + ".1")
      }
    } catch {}
    // Open with mode 0600 so a freshly created log is owner-only, matching
    // the Rust hook's permissions.
    const fd = openSync(EVENT_LOG, "a", 0o600)
    try {
      // mode only applies at creation; tighten pre-existing logs too.
      fchmodSync(fd, 0o600)
      writeSync(fd, JSON.stringify(event) + "\n")
    } finally {
      closeSync(fd)
    }
  } catch (e) {
    dbg("emit failed: " + e)
  }
}

function base(kind: string) {
  return {
    schema_version: 1,
    kind,
    runtime: "opencode",
    observed_via: "plugin",
    timestamp: new Date().toISOString(),
  }
}

async function recommendNames(
  query: string,
  max = 5,
  context = "",
): Promise<{ name: string; digest?: string }[]> {
  const roots = (process.env.SKILL_GRAPH_ROOTS ?? "")
    .split(":")
    .filter(Boolean)
    .flatMap((r) => ["--root", r])
  // The query travels on stdin (--query-stdin), not argv: prompts may
  // begin with `-` and prompt bodies should not sit in process listings.
  const args = [
    SKILL_GRAPH_BIN, "recommend", "--query-stdin",
    ...roots, "--max", String(max), "--format", "json",
  ]
  if (context) args.push("--context", context)
  const proc = Bun.spawn(args, {
    stdin: "pipe",
    stdout: "pipe",
    stderr: "ignore",
  })
  proc.stdin.write(query)
  proc.stdin.end()
  const out = await new Response(proc.stdout).text()
  await proc.exited
  try {
    const recs = JSON.parse(out) as { skill: { name: string; digest?: string } }[]
    return recs.map((r) => ({ name: r.skill.name, digest: r.skill.digest }))
  } catch {
    return []
  }
}

function resultText(result: unknown): string {
  if (typeof result === "string") return result
  try {
    const r = result as any
    const parts: string[] = []
    const walk = (v: any) => {
      if (!v || typeof v !== "object") return
      if (typeof v.text === "string") parts.push(v.text)
      if (typeof v.content === "string") parts.push(v.content)
      if (Array.isArray(v.content)) v.content.forEach(walk)
      if (Array.isArray(v.output)) v.output.forEach(walk)
      if (v.output && typeof v.output === "object") walk(v.output)
    }
    walk(r)
    return parts.join("\n")
  } catch {
    return ""
  }
}

async function detectLoad(event: any): Promise<Record<string, any>[]> {
  // Cheap pre-filter: only SKILL.md-touching tools reach the binary.
  const tool = String(event.tool ?? "")
  const input = event.input ?? {}
  const inputJson = JSON.stringify(input ?? {})
  const isSkillTool = tool.toLowerCase() === "skill"
  const touches = inputJson.includes("SKILL.md") ||
    (typeof input?.path === "string" && input.path.endsWith("SKILL.md"))
  if (!isSkillTool && !touches) return []

  const payload = {
    tool_name: tool,
    tool_input: input,
    tool_response: resultText(event.result),
    session_id: event.sessionID ?? "",
  }
  try {
    const proc = Bun.spawn(
      [SKILL_GRAPH_BIN, "detect", "--runtime", "opencode", "--via", "plugin"],
      { stdin: "pipe", stdout: "pipe", stderr: "ignore" },
    )
    proc.stdin.write(JSON.stringify(payload))
    proc.stdin.end()
    const out = await new Response(proc.stdout).text()
    await proc.exited
    const trimmed = out.trim()
    if (!trimmed) return []
    // `detect` emits one JSON object per line (a payload may touch
    // several SKILL.md files); parse each line independently.
    const events: Record<string, any>[] = []
    for (const line of trimmed.split("\n")) {
      try {
        events.push(JSON.parse(line))
      } catch {
        dbg("detect: unparseable line: " + line.slice(0, 120))
      }
    }
    return events
  } catch (e) {
    dbg("detect failed: " + e)
    return []
  }
}

const DEP_TRIGGER = new Set(["file_read", "path_reference", "native_invocation", "inferred", "applied"])

export default Plugin.define({
  id: "skill-graph",
  async setup(ctx) {
    dbg("setup called")
    // sessionID -> queued injections for the next context hook
    const pending = new Map<string, { kind: "prompt" | "deps"; names: string[] }[]>()
    // sessionID -> skills already dependency-recommended (parity with the
    // Rust hook's `skill:<name>` dedup markers).
    const loadDedup = new Map<string, Set<string>>()

    const queue = (sessionID: string, kind: "prompt" | "deps", names: string[]) => {
      if (!names.length) return
      if (!pending.has(sessionID) && pending.size >= MAX_PENDING_SESSIONS) {
        // FIFO-evict the oldest session; its queued context is dropped.
        const oldest = pending.keys().next().value
        if (oldest !== undefined) pending.delete(oldest)
      }
      const list = pending.get(sessionID) ?? []
      list.push({ kind, names })
      pending.set(sessionID, list)
    }

    // Session continuity parity with the Rust hook (issue #128): names a
    // session's prompt mode surfaced become context for later skill-load
    // dependency lookups. Bounded like `pending`.
    const rememberSession = (sessionID: string, names: string[]) => {
      if (!names.length) return
      // Accumulate like the Rust hook's .recommended file: dedup, keep
      // the most recent MAX_SESSION_NAMES names per session.
      const merged = [...(sessionCtx.get(sessionID) ?? [])]
      for (const n of names) {
        if (validSkillName(n) && !merged.includes(n)) merged.push(n)
      }
      const capped = merged.slice(-MAX_SESSION_NAMES)
      if (!capped.length) return
      if (!sessionCtx.has(sessionID) && sessionCtx.size >= MAX_SESSION_CTX) {
        const oldest = sessionCtx.keys().next().value
        if (oldest !== undefined) sessionCtx.delete(oldest)
      }
      sessionCtx.set(sessionID, capped)
    }

    await ctx.session.hook("prompt", async (event) => {
      const text = event.prompt?.text ?? ""
      const sessionID = event.sessionID ?? "unknown"
      if (text) {
        emit({
          ...base("prompt_submit"),
          session_id: sessionID,
          prompt_hash: sha256(text),
        })
      }
      // Native skill selection is a weaker signal than an actual load.
      const skills = (event.prompt as any)?.skills
      if (Array.isArray(skills) && skills.length > 0) {
        for (const s of skills) {
          const name = typeof s === "string" ? s : s?.name ?? s?.id
          if (typeof name === "string" && name && validSkillName(name)) {
            emit({
              ...base("skill_load"),
              session_id: sessionID,
              skill_name: name,
              detection: "inferred",
              confidence: "medium",
              tool: "prompt.skills",
            })
          }
        }
      }
      // Empty prompts carry no signal — the Rust hook exits before
      // emitting any recommendation event for them.
      if (!text) return
      const key = sha256(`${sessionID}\n${text}`)
      if (emitted.has(key)) return
      if (emitted.size >= MAX_EMITTED) {
        const oldest = emitted.values().next().value
        if (oldest !== undefined) emitted.delete(oldest)
      }
      emitted.add(key)
      const recs = await recommendNames(text, 3).catch(() => [])
      // recommended[] must equal the injected set for funnel integrity.
      const names = recs.map((r) => r.name)
      rememberSession(sessionID, names)
      emit({
        ...base("recommendation"),
        session_id: sessionID,
        trigger: "prompt_submit",
        prompt_hash: text ? sha256(text) : undefined,
        recommended: recs,
        emitted: names.length > 0,
        reason: names.length > 0 ? "recommended" : "no_recommendations",
      })
      if (names.length > 0) {
        queue(sessionID, "prompt", names)
        console.log(`[skill-graph] recommended: ${names.join(", ")}`)
      }
    })

    await ctx.tool.hook("execute.after", async (event) => {
      const detected = await detectLoad(event)
      if (!detected.length) return
      const sessionID = (event.sessionID ?? "unknown") as string
      for (const det of detected) {
        emit(det)
        const detection = det.detection as string | undefined
        const skillName = det.skill_name as string | undefined
        if (!skillName || !DEP_TRIGGER.has(detection ?? "")) continue

        // Dependency lookup for the just-loaded skill (same contract as
        // the shell adapters' skill-load mode, including session-context
        // continuity). recommended[] records exactly the injected set
        // for funnel integrity.
        // Skip skills this session already got a dependency pass for.
        let done = loadDedup.get(sessionID)
        if (!done) {
          if (loadDedup.size >= MAX_PENDING_SESSIONS) {
            const oldest = loadDedup.keys().next().value
            if (oldest !== undefined) loadDedup.delete(oldest)
          }
          done = new Set()
          loadDedup.set(sessionID, done)
        }
        if (done.has(skillName)) continue
        done.add(skillName)
        const sessionNames = sessionCtx.get(sessionID) ?? []
        const deps = await recommendNames(
          skillName,
          5,
          sessionNames.join(" "),
        ).catch(() => [])
        const injected = deps.filter((d) => d.name !== skillName).slice(0, 3)
        const names = injected.map((r) => r.name)
        emit({
          ...base("recommendation"),
          session_id: sessionID,
          trigger: "skill_load",
          recommended: injected,
          emitted: names.length > 0,
          reason: names.length > 0 ? "recommended" : "no_recommendations",
        })
        if (names.length > 0) queue(sessionID, "deps", names)
      }
    })

    await ctx.session.hook("context", (event) => {
      const list = pending.get(event.sessionID)
      if (!list?.length) return
      pending.delete(event.sessionID)
      for (const entry of list) {
        const text =
          entry.kind === "deps"
            ? `skill-graph: a skill declares dependencies — ${entry.names.join(", ")}. Read the matching SKILL.md files if the task needs them.`
            : `skill-graph: skills relevant to this task — ${entry.names.join(", ")}. Read the matching SKILL.md before relying on it.`
        event.system.push({ type: "text", text })
      }
    })
  },
})

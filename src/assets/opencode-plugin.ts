import { Plugin } from "@opencode/plugin"
import { closeSync, constants, fchmodSync, fstatSync, lstatSync, mkdirSync, openSync, renameSync, writeSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, isAbsolute, join } from "node:path"
import { createHash } from "node:crypto"

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

// Empty string is treated as unset, matching the Rust adapter's
// SKILL_GRAPH_BIN handling (#144 r15).
const SKILL_GRAPH_BIN = process.env.SKILL_GRAPH_BIN || "skill-graph"
// Parity with the Rust hook's env_absolute_path: env-supplied paths
// must be absolute — a relative value resolves against the OpenCode
// server process cwd (an arbitrary project tree) (#144 r13/r14).
// Node/Bun decode non-UTF-8 env bytes to U+FFFD rather than failing.
// Rejecting any value containing the replacement char treats those as
// unset — the unavoidable false positive is a legitimate path whose
// own bytes encode U+FFFD (the Rust hook accepts it via env::var),
// an accepted divergence documented in contracts.md (#144 r15/r16).
const absPath = (v: string | undefined): string | undefined =>
  v !== undefined && v !== "" && !v.includes("\uFFFD") && isAbsolute(v)
    ? v
    : undefined
const EVENT_LOG =
  absPath(process.env.SKILL_GRAPH_EVENT_LOG) ??
  join(absPath(tmpdir()) ?? "/tmp", "skill-graph-events.jsonl")
// Parity with the Rust hook: SKILL_GRAPH_HOOK_NOSTATE=1 suppresses all
// event writes; SKILL_GRAPH_EVENT_LOG_MAX bounds the log (rotate to .1).
const NOSTATE = process.env.SKILL_GRAPH_HOOK_NOSTATE === "1"
// Debug diagnostics go to stderr only — never to a file. The old
// fixed-path debug log under /tmp bypassed every emit() hardening
// (O_NOFOLLOW, 0600, uid check, TMPDIR, NOSTATE); issue #169 removed it.
// NOSTATE suppresses debug output too: no diagnostics are persisted.
const DBG = process.env.SKILL_GRAPH_DEBUG === "1" && !NOSTATE
const dbg = (m: string) => { if (DBG) try { console.error(`[skill-graph] ${m}`) } catch {} }
// Mirror the Rust hook's env parsing: `parse::<u64>()` accepts an
// optional leading '+' and the full u64 range, so anything else —
// floats, hex, exponents, whitespace, negatives — falls back to the
// default on both entry points. Values above 2^53-1 saturate: file
// sizes never approach that, so precision loss cannot change behavior.
function envUint(name: string, fallback: number): number {
  const raw = process.env[name]
  if (raw === undefined || !/^\+?[0-9]+$/.test(raw)) return fallback
  const n = BigInt(raw)
  if (n > 0xffffffffffffffffn) return fallback
  return n > BigInt(Number.MAX_SAFE_INTEGER) ? Number.MAX_SAFE_INTEGER : Number(n)
}
const EVENT_LOG_MAX = envUint("SKILL_GRAPH_EVENT_LOG_MAX", 64 * 1024 * 1024)
// Bound in-memory state: this plugin lives for the whole OpenCode server
// process, so unbounded maps would grow forever.
const MAX_EMITTED = 10_000
const MAX_PENDING_SESSIONS = 200
// prompt.skills bounds: entries scanned per prompt event and distinct
// names retained per session for dedup (#144 r14).
const MAX_PROMPT_SKILLS = 1_024
const MAX_SEEN_SKILLS = 4_096
const MAX_SESSION_CTX = 200
const MAX_SESSION_NAMES = 32
// detectLoad spawns one `skill-graph detect` subprocess per
// SKILL.md-touching tool call; bound the count per session so a
// pathological session cannot churn unbounded subprocesses. Legit
// sessions stay far below this; past it detection fails open (no
// events) rather than hanging the event loop.
const MAX_DETECT_PER_SESSION = 512
const emitted = new Set<string>()
const sessionCtx = new Map<string, string[]>()
const detectCounts = new Map<string, number>()

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex")
}

// Mirror parser::valid_skill_name — runtime-supplied names are untrusted
// and must never reach the event contract verbatim.
function validSkillName(name: string): boolean {
  return /^[a-z0-9-]{1,64}$/.test(name) && !name.startsWith("-") && !name.endsWith("-")
}

// Mirror events.rs's sanitize_session_id contract: absent/empty ids
// stay unattributed (""), charset-valid ids pass through bounded, and
// distinct all-invalid ids hash apart instead of collapsing into one
// "unknown" funnel session.
function safeSessionID(id: unknown): string {
  const s = typeof id === "string" ? id : ""
  const filtered = s.replace(/[^A-Za-z0-9._-]/g, "")
  // Distinct ids sharing a 128-char prefix must not collapse: bounded
  // head plus a hash suffix of the original (parity with Rust).
  if (filtered.length > 128) {
    return `${filtered.slice(0, 119)}-${createHash("sha256").update(s, "utf8").digest("hex").slice(0, 8)}`
  }
  if (filtered) return filtered
  if (!s) return ""
  return `unknown-${createHash("sha256").update(s, "utf8").digest("hex").slice(0, 16)}`
}

function emit(event: Record<string, unknown>): boolean {
  if (NOSTATE) return false
  try {
    mkdirSync(dirname(EVENT_LOG), { recursive: true })
    try {
      // lstat: a symlinked log must not be rotated through (the open
      // below refuses it anyway via O_NOFOLLOW).
      const st = lstatSync(EVENT_LOG)
      // isFile parity with the Rust hook: a directory must not be
      // renamed away as if it were an oversized log.
      if (!st.isSymbolicLink() && st.isFile() && st.size > EVENT_LOG_MAX) {
        renameSync(EVENT_LOG, EVENT_LOG + ".1")
      }
    } catch {}
    // Open with mode 0600 so a freshly created log is owner-only, matching
    // the Rust hook's permissions. O_NOFOLLOW refuses a pre-planted
    // symlink in the shared TMPDIR and O_NONBLOCK keeps a planted FIFO
    // from blocking this synchronous open forever (open fails ENXIO
    // with no reader) — parity with the Rust hook's open(2) hardening;
    // failures land in the catch and the emit is skipped.
    const fd = openSync(
      EVENT_LOG,
      constants.O_WRONLY | constants.O_APPEND | constants.O_CREAT | constants.O_NOFOLLOW | constants.O_NONBLOCK,
      0o600,
    )
    try {
      // A pre-existing log owned by another user (planted 0666 file in a
      // shared TMPDIR) must not receive our events: the open succeeds and
      // fchmod would fail, but the append would still land in a file we
      // do not own. Skip the write unless the file is ours. Non-regular
      // files (FIFO/socket that slipped past O_NONBLOCK) are refused too.
      const st = fstatSync(fd)
      if (!st.isFile()) return false
      if (typeof process.getuid === "function" && st.uid !== process.getuid()) return false
      // mode only applies at creation; tighten pre-existing logs too.
      // Isolated: on filesystems where fchmod fails post-open, the event
      // write must still succeed (0600 was already applied at O_CREAT).
      try { fchmodSync(fd, 0o600) } catch {}
      writeSync(fd, JSON.stringify(event) + "\n")
      return true
    } finally {
      closeSync(fd)
    }
  } catch (e) {
    dbg("emit failed: " + e)
    return false
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

// The plugin lives inside the long-lived OpenCode server: a wedged
// skill-graph child (stalled FS, huge corpus, hung wrapper) must not stall
// prompt handling or accumulate processes. Read stdout fully (avoids
// pipe-buffer deadlock), but kill the child after PROC_TIMEOUT_MS — the
// kill closes stdout so the read resolves with whatever arrived.
const PROC_TIMEOUT_MS = 20_000
// After the timeout kill, give the child a short drain window, then
// escalate to SIGKILL: a process in uninterruptible sleep must not leave
// collectProc pending forever.
const PROC_DRAIN_MS = 2_000
const MAX_LOAD_RECOMMENDS = 8
// Cap collected stdout: a runaway child must not buffer unbounded
// output into plugin memory before the timeout fires.
const MAX_PROC_OUTPUT_BYTES = 4 * 1024 * 1024

async function collectProc(proc: ReturnType<typeof Bun.spawn>): Promise<string> {
  // Attach a catch immediately: a stream error before the read resolves
  // would otherwise be an unhandled rejection.
  const readAll = (async () => {
    const reader = proc.stdout.getReader()
    const chunks: Uint8Array[] = []
    let total = 0
    try {
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        if (!value) continue
        const remaining = MAX_PROC_OUTPUT_BYTES - total
        if (remaining <= 0) break
        chunks.push(value.length > remaining ? value.subarray(0, remaining) : value)
        total += Math.min(value.length, remaining)
        if (total >= MAX_PROC_OUTPUT_BYTES) break
      }
    } catch {}
    try { proc.kill() } catch {}
    const merged = new Uint8Array(total)
    let off = 0
    for (const c of chunks) {
      const take = Math.min(c.length, merged.length - off)
      if (take <= 0) break
      merged.set(c.subarray(0, take), off)
      off += take
    }
    return new TextDecoder().decode(merged)
  })().catch(() => "")
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    await Promise.race([
      proc.exited,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
          try { proc.kill() } catch {}
          reject(new Error("skill-graph timed out"))
        }, PROC_TIMEOUT_MS)
      }),
    ])
  } catch {
    // timed out and killed; fall through to collect partial output
  } finally {
    if (timer) clearTimeout(timer)
  }
  // The kill closes stdout for well-behaved children; bound the drain so
  // a stuck stream cannot hang the hook, then escalate to SIGKILL.
  const drained = await Promise.race([
    readAll,
    new Promise<string>((resolve) =>
      setTimeout(() => {
        try { proc.kill("SIGKILL") } catch {}
        resolve("")
      }, PROC_DRAIN_MS),
    ),
  ])
  return drained
}

async function recommendNames(
  query: string,
  max = 5,
  context = "",
): Promise<{ name: string; digest?: string }[]> {
  // No --root args: the spawned CLI inherits SKILL_GRAPH_ROOTS, and
  // resolve_roots gives that env var precedence over explicit args, so
  // argv roots would be dead whenever the env is set (#144 r17).
  // The query travels on stdin (--query-stdin), not argv: prompts may
  // begin with `-` and prompt bodies should not sit in process listings.
  const args = [
    SKILL_GRAPH_BIN, "recommend", "--query-stdin",
    "--max", String(max), "--format", "json",
  ]
  if (context) args.push("--context", context)
  const proc = Bun.spawn(args, {
    stdin: "pipe",
    stdout: "pipe",
    stderr: "ignore",
  })
  proc.stdin.write(query)
  proc.stdin.end()
  const out = await collectProc(proc).catch(() => "")
  try {
    const recs = JSON.parse(out) as { skill: { name: string; digest?: string } }[]
    // `recommend` output is operator-trusted but still normalized like
    // every other plugin path: names must satisfy the skill-name
    // contract and the list is bounded before it can reach context.
    return recs
      .filter((r) => validSkillName(r.skill.name))
      .slice(0, 64)
      .map((r) => ({ name: r.skill.name, digest: r.skill.digest }))
  } catch {
    return []
  }
}

// The plugin host is long-lived: an uncapped tool result or input
// would be buffered here (parts → join → JSON.stringify → stdin),
// several× amplified, before the child's own 16 MiB/4 MiB stdin caps
// ever see it (#144 r18). Legitimate hook/plugin payloads are
// KB-scale, so these bounds are generous.
const MAX_RESULT_TEXT_BYTES = 64 * 1024
const MAX_INPUT_JSON_BYTES = 1024 * 1024
const MAX_PAYLOAD_BYTES = 1024 * 1024

function resultText(result: unknown): string {
  if (typeof result === "string") return result.slice(0, MAX_RESULT_TEXT_BYTES)
  try {
    const r = result as any
    const parts: string[] = []
    let total = 0
    const push = (s: string) => {
      if (total >= MAX_RESULT_TEXT_BYTES) return
      const room = MAX_RESULT_TEXT_BYTES - total
      parts.push(s.length > room ? s.slice(0, room) : s)
      total += Math.min(s.length, room)
    }
    const walk = (v: any) => {
      if (total >= MAX_RESULT_TEXT_BYTES || !v || typeof v !== "object") return
      if (typeof v.text === "string") push(v.text)
      if (typeof v.content === "string") push(v.content)
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
  try {
    // Cheap pre-filter: only SKILL.md-touching tools reach the binary.
    const tool = String(event.tool ?? "")
    const input = event.input ?? {}
    const inputJson = JSON.stringify(input ?? {})
    // An oversized input cannot fit the payload budget below anyway —
    // skip before the SKILL.md scan so a giant object is not walked
    // twice (#144 r18).
    if (inputJson.length > MAX_INPUT_JSON_BYTES) return []
    const isSkillTool = tool.toLowerCase() === "skill"
    const touches = inputJson.includes("SKILL.md") ||
      (typeof input?.path === "string" && input.path.endsWith("SKILL.md"))
    if (!isSkillTool && !touches) return []

    // Bound actual `detect` subprocess spawns per session. Counting
    // happens HERE, after the prefilter — charging every tool call
    // would let 512 unrelated executions silently disable detection
    // for the rest of a long session.
    const sid = safeSessionID(event.sessionID)
    const detects = detectCounts.get(sid) ?? 0
    if (detects >= MAX_DETECT_PER_SESSION) return []
    if (detectCounts.size >= MAX_PENDING_SESSIONS && !detectCounts.has(sid)) {
      const oldest = detectCounts.keys().next().value
      if (oldest !== undefined) detectCounts.delete(oldest)
    }
    detectCounts.set(sid, detects + 1)

    const payload = {
      tool_name: tool,
      tool_input: input,
      tool_response: resultText(event.result),
      session_id: sid,
      // Forwarded defensively so the Rust-side error-status guard also
      // applies to plugin-originated payloads (the field name is
      // UNVERIFIED against the OpenCode event API).
      status: (event as any).status,
      success: (event as any).success,
      error: (event as any).error,
    }
    const body = JSON.stringify(payload)
    // The child's stdin parse cap is 4 MiB; anything near it would be
    // rejected after a wasted spawn, and the forwarded status/error
    // fields are unbounded event data. Skip detection instead — the
    // plugin fails open by design (#144 r18).
    if (body.length > MAX_PAYLOAD_BYTES) return []
    const proc = Bun.spawn(
      [SKILL_GRAPH_BIN, "detect", "--runtime", "opencode", "--via", "plugin"],
      { stdin: "pipe", stdout: "pipe", stderr: "ignore" },
    )
    proc.stdin.write(body)
    proc.stdin.end()
    const out = await collectProc(proc).catch(() => "")
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
    // sessionID -> skills already reported via prompt.skills; the same
    // skill re-listed on every prompt would otherwise emit a fresh
    // load-grade event per prompt.
    const promptSkillSeen = new Map<string, Set<string>>()

    // Per-session entry cap: MAX_PENDING_SESSIONS bounds distinct
    // sessions, but a session whose context hook never drains would
    // otherwise grow `list` for its whole lifetime (#144 r18).
    const MAX_PENDING_ENTRIES_PER_SESSION = 64
    const queue = (sessionID: string, kind: "prompt" | "deps", names: string[]) => {
      if (!names.length) return
      if (!pending.has(sessionID) && pending.size >= MAX_PENDING_SESSIONS) {
        // FIFO-evict the oldest session; its queued context is dropped.
        const oldest = pending.keys().next().value
        if (oldest !== undefined) pending.delete(oldest)
      }
      const list = pending.get(sessionID) ?? []
      list.push({ kind, names })
      if (list.length > MAX_PENDING_ENTRIES_PER_SESSION) {
        list.splice(0, list.length - MAX_PENDING_ENTRIES_PER_SESSION)
      }
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
      const sessionID = safeSessionID(event.sessionID)
      if (text) {
        emit({
          ...base("prompt_submit"),
          session_id: sessionID,
          prompt_hash: sha256(text),
        })
      }
      // Native skill selection is a weaker signal than an actual load.
      // Dedup per (session, skill): prompt.skills may reflect cumulative
      // session state, and re-emitting per prompt would inflate
      // load-grade counts.
      const skills = (event.prompt as any)?.skills
      if (Array.isArray(skills) && skills.length > 0) {
        let seen = promptSkillSeen.get(sessionID)
        if (!seen) {
          if (promptSkillSeen.size >= MAX_PENDING_SESSIONS) {
            const oldest = promptSkillSeen.keys().next().value
            if (oldest !== undefined) promptSkillSeen.delete(oldest)
          }
          seen = new Set()
          promptSkillSeen.set(sessionID, seen)
        }
        // Bound both the scan and the retained dedup set: prompt.skills
        // is runtime-supplied and unbounded (#144 r14).
        for (const s of skills.slice(0, MAX_PROMPT_SKILLS)) {
          const name = typeof s === "string" ? s : s?.name ?? s?.id
          if (typeof name !== "string" || !name || !validSkillName(name)) continue
          if (seen.has(name)) continue
          if (seen.size >= MAX_SEEN_SKILLS) continue
          // Register the dedup key only when the event was actually
          // written — a transient log failure must not suppress retries.
          if (emit({
            ...base("skill_load"),
            session_id: sessionID,
            skill_name: name,
            detection: "inferred",
            confidence: "medium",
            tool: "prompt.skills",
          })) seen.add(name)
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
      // execute.after fires for failed calls too; a skill that errored
      // never loaded, so it must not emit skill_load or queue dependency
      // recommendations (parity with load-grade detection intent).
      if ((event as any).status === "error") return
      const sessionID = safeSessionID(event.sessionID)
      const detected = await detectLoad(event)
      if (!detected.length) return
      // Parity with the Rust hook's MAX_LOAD_RECOMMENDS: one tool call
      // touching many SKILL.md files must not spawn a recommend process
      // per skill — each spawn is a full corpus scan.
      let recommendBudget = MAX_LOAD_RECOMMENDS
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
        // Budget check before marking done: a skill skipped for budget
        // was never recommended, so a later payload may still surface it
        // (same ordering as the Rust hook).
        if (recommendBudget <= 0) continue
        done.add(skillName)
        recommendBudget -= 1
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
      const sessionID = safeSessionID(event.sessionID)
      const list = pending.get(sessionID)
      if (!list?.length) return
      pending.delete(sessionID)
      // The context-hook payload shape is runtime-defined; `system` may
      // be absent or non-array. Fail open rather than throw inside the
      // plugin host — this is the only unguarded path left.
      if (!Array.isArray(event.system)) {
        try {
          ;(event as { system?: unknown }).system = []
        } catch {
          return
        }
      }
      const system = event.system as { type: string; text: string }[]
      for (const entry of list) {
        const text =
          entry.kind === "deps"
            ? `skill-graph: a skill declares dependencies — ${entry.names.join(", ")}. Read the matching SKILL.md files if the task needs them.`
            : `skill-graph: skills relevant to this task — ${entry.names.join(", ")}. Read the matching SKILL.md before relying on it.`
        system.push({ type: "text", text })
      }
    })
  },
})

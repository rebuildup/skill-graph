import { Plugin } from "@opencode/plugin"
import { appendFileSync, mkdirSync } from "node:fs"
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
const emitted = new Set<string>()

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex")
}

function emit(event: Record<string, unknown>) {
  try {
    mkdirSync(dirname(EVENT_LOG), { recursive: true })
    appendFileSync(EVENT_LOG, JSON.stringify(event) + "\n")
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

async function recommendNames(query: string, max = 5): Promise<{ name: string; digest?: string }[]> {
  const roots = (process.env.SKILL_GRAPH_ROOTS ?? "")
    .split(":")
    .filter(Boolean)
    .flatMap((r) => ["--root", r])
  const proc = Bun.spawn(
    [SKILL_GRAPH_BIN, "recommend", query, ...roots, "--max", String(max), "--format", "json"],
    { stdout: "pipe", stderr: "ignore" },
  )
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

async function detectLoad(event: any): Promise<Record<string, any> | null> {
  // Cheap pre-filter: only SKILL.md-touching tools reach the binary.
  const tool = String(event.tool ?? "")
  const input = event.input ?? {}
  const inputJson = JSON.stringify(input ?? {})
  const isSkillTool = tool.toLowerCase() === "skill"
  const touches = inputJson.includes("SKILL.md") ||
    (typeof input?.path === "string" && input.path.endsWith("SKILL.md"))
  if (!isSkillTool && !touches) return null

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
    if (!trimmed) return null
    return JSON.parse(trimmed)
  } catch (e) {
    dbg("detect failed: " + e)
    return null
  }
}

const DEP_TRIGGER = new Set(["file_read", "path_reference", "native_invocation", "inferred", "applied"])

export default Plugin.define({
  id: "skill-graph",
  async setup(ctx) {
    dbg("setup called")
    // sessionID -> queued injections for the next context hook
    const pending = new Map<string, { kind: "prompt" | "deps"; names: string[] }[]>()

    const queue = (sessionID: string, kind: "prompt" | "deps", names: string[]) => {
      if (!names.length) return
      const list = pending.get(sessionID) ?? []
      list.push({ kind, names })
      pending.set(sessionID, list)
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
          if (typeof name === "string" && name) {
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
      const key = `${sessionID}:${text.length}:${text.slice(0, 64)}`
      if (emitted.has(key)) return
      emitted.add(key)
      const recs = await recommendNames(text, 3).catch(() => [])
      const names = recs.map((r) => r.name)
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
      if (!detected) return
      emit(detected)
      const detection = detected.detection as string | undefined
      const skillName = detected.skill_name as string | undefined
      const sessionID = (event.sessionID ?? detected.session_id ?? "unknown") as string
      if (!skillName || !DEP_TRIGGER.has(detection ?? "")) return

      // Dependency lookup for the just-loaded skill (same contract as the
      // shell adapters' skill-load mode).
      const deps = await recommendNames(skillName, 5).catch(() => [])
      const names = deps.map((r) => r.name).filter((n) => n !== skillName).slice(0, 3)
      emit({
        ...base("recommendation"),
        session_id: sessionID,
        trigger: "skill_load",
        recommended: deps,
        emitted: names.length > 0,
        reason: names.length > 0 ? "recommended" : "no_recommendations",
      })
      if (names.length > 0) queue(sessionID, "deps", names)
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

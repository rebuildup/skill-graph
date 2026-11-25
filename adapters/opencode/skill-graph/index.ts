import { Plugin } from "@opencode/plugin"
import { appendFileSync } from "node:fs"
const DBG = process.env.SKILL_GRAPH_DEBUG === "1" ? "/tmp/sg-oc-plugin.log" : ""
const dbg = (m: string) => { if (DBG) try { appendFileSync(DBG, m + "\n") } catch {} }

// skill-graph adapter for OpenCode.
//
// Design (see docs/evidence/2026-11-25-v020-opencode-adapter-feasibility.md):
// - session.hook("prompt") sees the mutable prompt draft, including selected
//   `skills`. We run the skill-graph CLI on the prompt text and, on a hit,
//   append a system-style note via the `context` hook is NOT possible here —
//   instead we record recommendations and let the `context` hook inject them
//   as system text on the next model call.
// - All state is in-memory per plugin instance; dedup mirrors the shell
//   adapter (one emission per session+prompt hash).
//
// UNVERIFIED: written against the documented plugin API
// (ctx.session.hook("prompt"/"context"), event.prompt.text/skills,
// event.system.push). Requires live verification against a real OpenCode
// server before release.

const SKILL_GRAPH_BIN = process.env.SKILL_GRAPH_BIN ?? "skill-graph"
const emitted = new Set<string>()

async function recommend(promptText: string): Promise<string[]> {
  const roots = (process.env.SKILL_GRAPH_ROOTS ?? "")
    .split(":")
    .filter(Boolean)
    .flatMap((r) => ["--root", r])
  const proc = Bun.spawn(
    [SKILL_GRAPH_BIN, "recommend", promptText, ...roots, "--max", "3", "--format", "json"],
    { stdout: "pipe", stderr: "ignore" },
  )
  const out = await new Response(proc.stdout).text()
  await proc.exited
  try {
    const recs = JSON.parse(out) as { skill: { name: string } }[]
    return recs.map((r) => r.skill.name)
  } catch {
    return []
  }
}

export default Plugin.define({
  id: "skill-graph",
  async setup(ctx) {
    dbg("setup called, bun=" + (typeof Bun))
    const pending = new Map<string, string[]>()

    await ctx.session.hook("prompt", async (event) => {
      dbg("prompt hook keys=" + JSON.stringify(Object.keys(event)) + " sid=" + JSON.stringify((event as any).sessionID))
      const text = event.prompt.text ?? ""
      const key = `${event.sessionID}:${text.length}:${text.slice(0, 64)}`
      if (emitted.has(key)) return
      emitted.add(key)
      const names = await recommend(text).catch((e) => { dbg("recommend err " + e); return [] })
      dbg("recommend -> " + JSON.stringify(names))
      if (names.length > 0) {
        pending.set(event.sessionID, names)
        console.log(`[skill-graph] recommended: ${names.join(", ")}`)
      }
    })

    await ctx.session.hook("context", (event) => {
      dbg("context hook")
      const names = pending.get(event.sessionID)
      if (!names?.length) return
      pending.delete(event.sessionID)
      event.system.push({
        type: "text",
        text: `skill-graph: skills relevant to this task — ${names.join(", ")}. Read the matching SKILL.md before relying on it.`,
      })
    })
  },
})

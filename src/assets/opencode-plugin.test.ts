import { describe, expect, mock, test } from "bun:test"
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { createHash } from "node:crypto"
import { tmpdir } from "node:os"
import { join } from "node:path"

// The plugin module registers itself via @opencode/plugin at import
// time; stub the module so the file can be imported for its pure
// helpers. Plugin.define receives the config object unchanged.
mock.module("@opencode/plugin", () => ({
  Plugin: { define: (config: unknown) => config },
}))

// Env vars are read at module load: point the plugin at a fake CLI
// (fixed `recommend` result, empty `detect`) and a temp event log so
// the session-isolation tests below observe real emit/queue behavior
// without a built binary or the shared /tmp log (#225 re-review).
const testDir = mkdtempSync(join(tmpdir(), "sg-plugin-test-"))
const fakeBin = join(testDir, "skill-graph")
writeFileSync(
  fakeBin,
  '#!/bin/sh\ncat >/dev/null 2>&1 || true\n' +
    'if [ "$1" = "recommend" ]; then ' +
    'printf \'%s\' \'[{"skill":{"name":"alpha","digest":"d1"}},{"skill":{"name":"beta","digest":"d2"}}]\'\n' +
    "fi\n",
)
chmodSync(fakeBin, 0o755)
process.env.SKILL_GRAPH_BIN = fakeBin
process.env.SKILL_GRAPH_EVENT_LOG = join(testDir, "events.jsonl")

const { default: plugin, safeSessionID } = await import("./opencode-plugin")

const sha256 = (s: string) => createHash("sha256").update(s, "utf8").digest("hex")

describe("safeSessionID — parity with events::sanitize_session_id", () => {
  test("empty and absent ids stay unattributed", () => {
    expect(safeSessionID("")).toBe("")
    expect(safeSessionID(undefined)).toBe("")
    expect(safeSessionID(null)).toBe("")
    expect(safeSessionID(42)).toBe("")
  })

  test("charset-valid ids pass through unchanged", () => {
    expect(safeSessionID("s-1.ok_2")).toBe("s-1.ok_2")
    expect(safeSessionID("sess1")).toBe("sess1")
  })

  test("distinct all-invalid ids hash apart deterministically", () => {
    const a = safeSessionID("!!!")
    const b = safeSessionID("???")
    expect(a).toBe(`unknown-${sha256("!!!").slice(0, 16)}`)
    expect(b).toBe(`unknown-${sha256("???").slice(0, 16)}`)
    expect(a).not.toBe(b)
    expect(safeSessionID("!!!")).toBe(a)
  })

  test("stripped chars get an original-bytes hash suffix", () => {
    const a = safeSessionID("sess!1")
    const b = safeSessionID("sess?1")
    expect(a).toBe(`sess1-${sha256("sess!1").slice(0, 8)}`)
    expect(b).toBe(`sess1-${sha256("sess?1").slice(0, 8)}`)
    expect(a).not.toBe(b)
    expect(safeSessionID("sess!1")).toBe(a)
  })

  test("over-128-char ids truncate to head + hash, never exceeding 128", () => {
    const raw = "x".repeat(200)
    const out = safeSessionID(raw)
    expect(out).toBe(`${"x".repeat(119)}-${sha256(raw).slice(0, 8)}`)
    expect(out.length).toBe(128)
    // Exactly 128 valid chars pass through bare.
    const exact = "y".repeat(128)
    expect(safeSessionID(exact)).toBe(exact)
    // 129 valid chars is already overflow.
    const out129 = safeSessionID("z".repeat(129))
    expect(out129.length).toBe(128)
    expect(out129).not.toBe("z".repeat(129))
  })

  test("stripped-char suffix never exceeds the 128-char contract", () => {
    // #225 re-review: a 128-char filtered id plus "-" + hash emitted
    // 137 chars before the head cap.
    for (const raw of [
      `${"x".repeat(128)}!`,
      `${"x".repeat(120)}?`,
      `${"x".repeat(200)}!`,
      "sess!1",
    ]) {
      const out = safeSessionID(raw)
      expect(out.length).toBeLessThanOrEqual(128)
      // Idempotent: an already-sanitized id is a fixed point.
      expect(safeSessionID(out)).toBe(out)
    }
    // Long ids sharing a 119-char filtered head still separate.
    const a = `${"x".repeat(128)}!a`
    const b = `${"x".repeat(128)}!b`
    expect(safeSessionID(a)).not.toBe(safeSessionID(b))
  })

  test("hostile multi-MB ids stay bounded", () => {
    const huge = `${"a".repeat(4 * 1024 * 1024)}!${"b".repeat(1024)}`
    const out = safeSessionID(huge)
    expect(out.length).toBe(128)
    expect(safeSessionID(huge)).toBe(out)
  })
})

// Session-state isolation contract (#225 re-review): a session whose id
// cannot be attributed (sanitized to "") must share NO per-session
// state — no dedup, no recommendation queue, no session continuity —
// parity with the Rust hook's state_disabled. Before the fix, all
// anonymous sessions shared one "" bucket: identical anonymous prompts
// were dedup-suppressed across sessions and queued recommendations
// leaked into whichever anonymous context hook drained them.
type HookFn = (event: any) => unknown

function captureCtx() {
  const handlers: Record<string, HookFn> = {}
  const ctx = {
    session: {
      hook: async (name: string, fn: HookFn) => {
        handlers[`session:${name}`] = fn
      },
    },
    tool: {
      hook: async (name: string, fn: HookFn) => {
        handlers[`tool:${name}`] = fn
      },
    },
  }
  return { handlers, ctx }
}

async function setupPlugin() {
  const { handlers, ctx } = captureCtx()
  await (plugin as any).setup(ctx)
  return handlers
}

function recommendationEvents() {
  return readFileSync(process.env.SKILL_GRAPH_EVENT_LOG!, "utf8")
    .split("\n")
    .filter((l) => l.trim())
    .map((l) => JSON.parse(l))
    .filter((e) => e.kind === "recommendation")
}

describe("anonymous session state isolation (#225)", () => {
  test("unattributed prompts share no dedup and queue nothing", async () => {
    const handlers = await setupPlugin()
    const prompt = handlers["session:prompt"]
    const context = handlers["session:context"]

    // Two prompts with NO session id and identical text: under the old
    // shared "" bucket the second was dedup-suppressed and the first
    // queued a recommendation any anonymous context hook would drain.
    await prompt({ sessionID: undefined, prompt: { text: "anon-unique-aaa" } })
    await prompt({ sessionID: undefined, prompt: { text: "anon-unique-aaa" } })

    // Dedup is per-session state: anonymous prompts must NOT suppress
    // each other — both emitted a recommendation event.
    const recs = recommendationEvents().filter((e) => e.session_id === "")
    expect(recs.length).toBe(2)

    // Nothing was queued: an anonymous context hook injects nothing.
    const ev = { sessionID: undefined, system: [] as unknown[] }
    await context(ev)
    expect(ev.system).toHaveLength(0)
  })

  test("attributed sessions keep dedup, queue, and context injection", async () => {
    const handlers = await setupPlugin()
    const prompt = handlers["session:prompt"]
    const context = handlers["session:context"]

    await prompt({ sessionID: "s-attr-1", prompt: { text: "attr-unique-bbb" } })
    // Identical repeat is dedup-suppressed for an attributed session.
    await prompt({ sessionID: "s-attr-1", prompt: { text: "attr-unique-bbb" } })

    const own = recommendationEvents().filter(
      (e) => e.session_id === "s-attr-1" && e.trigger === "prompt_submit",
    )
    expect(own.length).toBe(1)

    const ev = { sessionID: "s-attr-1", system: [] as { type: string; text: string }[] }
    await context(ev)
    expect(ev.system.length).toBe(1)
    expect(ev.system[0].text).toContain("alpha")

    // A different session must not see s-attr-1's queued context.
    const other = { sessionID: "s-attr-2", system: [] as unknown[] }
    await context(other)
    expect(other.system).toHaveLength(0)
  })

  test("anonymous and attributed sessions do not share queues", async () => {
    const handlers = await setupPlugin()
    const prompt = handlers["session:prompt"]
    const context = handlers["session:context"]

    await prompt({ sessionID: "s-attr-3", prompt: { text: "attr-unique-ccc" } })
    await prompt({ sessionID: undefined, prompt: { text: "anon-unique-ddd" } })

    // The attributed queue drains only for its own session; the
    // anonymous prompt queued nothing, so an anonymous context hook
    // sees an empty system even though a queue exists in the plugin.
    const anon = { sessionID: undefined, system: [] as unknown[] }
    await context(anon)
    expect(anon.system).toHaveLength(0)

    const attr = { sessionID: "s-attr-3", system: [] as unknown[] }
    await context(attr)
    expect(attr.system.length).toBe(1)
  })
})

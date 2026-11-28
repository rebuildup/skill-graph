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
  // detect stdin is captured to <fakeBin>.capture: Bun.spawn does not
  // see runtime process.env mutations, so a fixed sibling path is the
  // reliable observation point.
  '#!/bin/sh\ninput="$(cat 2>/dev/null || true)"\n' +
    'if [ "$1" = "detect" ]; then printf \'%s\' "$input" > "$0.capture"; fi\n' +
    'if [ "$1" = "recommend" ]; then ' +
    'printf \'%s\' \'[{"skill":{"name":"alpha","digest":"d1"}},{"skill":{"name":"beta","digest":"d2"}}]\'\n' +
    "fi\n",
)
chmodSync(fakeBin, 0o755)
process.env.SKILL_GRAPH_BIN = fakeBin
process.env.SKILL_GRAPH_EVENT_LOG = join(testDir, "events.jsonl")

const {
  default: plugin,
  safeSessionID,
  boundJsonValue,
  BOUND_FAIL,
  MAX_INPUT_NODES,
  resultText,
  utf8Slice,
} = await import("./opencode-plugin")

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
    expect(a).toBe(`u-${sha256("!!!").slice(0, 32)}`)
    expect(b).toBe(`u-${sha256("???").slice(0, 32)}`)
    expect(a).not.toBe(b)
    expect(safeSessionID("!!!")).toBe(a)
  })

  test("stripped chars get an original-bytes hash suffix", () => {
    const a = safeSessionID("sess!1")
    const b = safeSessionID("sess?1")
    expect(a).toBe(`sess1-${sha256("sess!1").slice(0, 32)}`)
    expect(b).toBe(`sess1-${sha256("sess?1").slice(0, 32)}`)
    expect(a).not.toBe(b)
    expect(safeSessionID("sess!1")).toBe(a)
    // Fixed cross-implementation vector: events.rs asserts the same
    // literal — the sanitize rules are a shared event contract.
    expect(a).toBe("sess1-f1437b8fb22261cc21b7a104ca00e1b1")
  })

  test("over-128-char ids truncate to head + hash, never exceeding 128", () => {
    const raw = "x".repeat(200)
    const out = safeSessionID(raw)
    expect(out).toBe(`${"x".repeat(95)}-${sha256(raw).slice(0, 32)}`)
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
      // Idempotency is intentionally NOT a contract (#225 r5):
      // injectivity and idempotency are mutually exclusive.
    }
    // Long ids sharing a 95-char filtered head still separate.
    const a = `${"x".repeat(128)}!a`
    const b = `${"x".repeat(128)}!b`
    expect(safeSessionID(a)).not.toBe(safeSessionID(b))
  })

  test("reserved transform shape cannot collide with literal ids (#225 r5)", () => {
    // A charset-valid literal equal to another id's transform output
    // must itself be transformed — the "-" + 32-hex shape is reserved,
    // so the two namespaces can never overlap.
    const transformed = safeSessionID("sess!1")
    const literal = transformed // valid charset, <=128 chars
    expect(safeSessionID(literal)).not.toBe(transformed)
    // Hand-crafted reserved-shape ids are transformed too.
    const crafted = `sess1-${"0123456789abcdef".repeat(2)}`
    expect(safeSessionID(crafted)).not.toBe(crafted)
    // Non-reserved canonical ids still pass through.
    expect(safeSessionID("sess-0123456789abcdef")).toBe("sess-0123456789abcdef")
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

describe("detect payload contract (#225 r7)", () => {
  test("detect stdin carries the RAW session id — detect normalizes once", async () => {
    // `skill-graph detect` is an ingest boundary: it runs
    // SessionId::normalize on whatever session_id it receives. If the
    // plugin forwarded its already-normalized `sid`, a non-canonical id
    // would be transformed a SECOND time (transform outputs match the
    // reserved shape) and detect-emitted events would carry a different
    // session_id than plugin-emitted ones — splitting funnel state.
    const capture = `${fakeBin}.capture`
    const handlers = await setupPlugin()
    const after = handlers["tool:execute.after"]

    const raw = "sess!1" // non-canonical: safeSessionID transforms it
    await after({
      sessionID: raw,
      tool: "read",
      input: { path: "/tmp/skills/foo/SKILL.md" },
      result: "file contents",
    })

    const body = JSON.parse(readFileSync(capture, "utf8"))
    expect(body.session_id).toBe(raw)
    expect(body.session_id).not.toBe(safeSessionID(raw))
  })

  test("raw session id slice always contains the full identity domain (#225 r9)", async () => {
    // normalize compares the first 8192 code points; the transport
    // slice (32 KiB) must contain ALL of them even for 4-byte chars,
    // so safeSessionID(slice) === safeSessionID(full) and detect's
    // normalize(raw) lands on the same sid — for ANY host id size.
    const capture = `${fakeBin}.capture`
    const handlers = await setupPlugin()
    const after = handlers["tool:execute.after"]

    // Worst case for the byte→char ratio: all 4-byte code points.
    const huge = "sess-" + "\u{1F600}".repeat(20 * 1024)
    await after({
      sessionID: huge,
      tool: "read",
      input: { path: "/tmp/skills/foo/SKILL.md" },
      result: "ok",
    })

    const body = JSON.parse(readFileSync(capture, "utf8"))
    expect(body.session_id).not.toBe(huge)
    expect(Buffer.byteLength(body.session_id, "utf8")).toBeLessThanOrEqual(32 * 1024)
    // The forwarded slice must normalize to the SAME sid the plugin
    // used for state keys — i.e. safeSessionID(slice) equals
    // safeSessionID(full): the domain is inside the slice.
    expect(safeSessionID(body.session_id)).toBe(safeSessionID(huge))
    // And it must actually contain ≥8192 code points (the whole
    // identity domain), proving the 32 KiB budget is sufficient.
    expect([...body.session_id].length).toBeGreaterThanOrEqual(8192)
  })

  test("session ids differing only beyond the 8192-char domain share identity BY CONTRACT", () => {
    // The domain is a normalize-level contract shared by every
    // producer — not a plugin-local truncation (#225 r9). Two ids
    // that agree on the first 8192 chars are the same session
    // everywhere; this is documented, deterministic behavior.
    const prefix = "p".repeat(9000)
    expect(safeSessionID(prefix + "AAA")).toBe(safeSessionID(prefix + "BBB"))
    // Within the domain, distinctness is preserved.
    const p2 = "q".repeat(8191)
    expect(safeSessionID(p2 + "X" + "z".repeat(100))).not.toBe(
      safeSessionID(p2 + "Y" + "z".repeat(100)),
    )
  })
})

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
    // each other — both emitted a recommendation event. And because an
    // unattributed session can never receive the queued injection,
    // `emitted` must record false — claiming true would falsify the
    // funnel (#225 re-review).
    const recs = recommendationEvents().filter((e) => e.session_id === "")
    expect(recs.length).toBe(2)
    for (const e of recs) {
      expect(e.emitted).toBe(false)
      expect(e.reason).toBe("recommended")
    }

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
    // Attributed sessions DO get the injection queued, so emitted is
    // truthful here — the anonymous/attributed asymmetry is the point.
    expect(own[0].emitted).toBe(true)

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

// ------------------------------------------------------------------
// Property / boundary tests for the input-bounding internals (#225
// re-review). The budgets are contracts: work must be O(budget), not
// O(input). Proxy traps make the *amount of iteration* observable —
// a test that only checks output would pass even if the walk scanned
// the whole hostile input.
// ------------------------------------------------------------------

// Deterministic PRNG (mulberry32) — property tests must be
// reproducible; no external dep.
function mulberry32(seed: number) {
  let a = seed >>> 0
  return () => {
    a |= 0; a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

describe("input-bounding properties (#225)", () => {
  test("resultText stops iterating primitives at the node budget", () => {
    // A flat array of primitives: the OLD walk counted only objects,
    // so `walked` never moved and every element was visited. The
    // iterator trap counts actual next() calls — the contract is
    // O(MAX_INPUT_NODES), not O(array length).
    let iters = 0
    const src = new Array(200_000).fill("x")
    const proxy = new Proxy(src, {
      get(t, p) {
        if (p === Symbol.iterator) {
          // [Symbol.iterator]() must return an ITERATOR OBJECT whose
          // next() we wrap — returning a bare function makes for..of
          // throw (and resultText's catch would vacuously pass).
          return () => {
            const it = t[Symbol.iterator]()
            return {
              next: () => {
                iters++
                return it.next()
              },
            }
          }
        }
        return (t as unknown as Record<PropertyKey, unknown>)[p]
      },
    })
    resultText({ content: proxy })
    // Lower bound proves the walk actually iterated (a broken proxy
    // would vacuously satisfy the upper bound with iters == 0).
    expect(iters).toBeGreaterThan(100)
    expect(iters).toBeLessThanOrEqual(MAX_INPUT_NODES + 16)
  })

  test("boundJsonValue does not enumerate all keys of a huge object", () => {
    // Object.keys() materialized every key before the node budget
    // applied. for..in + early break must bound getOwnPropertyDescriptor
    // calls — the observable cost of enumeration.
    let gopd = 0
    const KEYS = 50_000
    const keys = Array.from({ length: KEYS }, (_, i) => `k${i}`)
    const proxy = new Proxy(
      {},
      {
        getPrototypeOf: () => Object.prototype,
        ownKeys: () => keys,
        getOwnPropertyDescriptor: () => {
          gopd++
          return { enumerable: true, configurable: true, value: 1 }
        },
        get: () => 1,
      },
    )
    const r = boundJsonValue(proxy, 0, { nodes: 0, strBytes: 0 })
    expect(r).toBe(BOUND_FAIL)
    // Enumeration must stop at the node budget: ~8192 keys × (for-in
    // gOPD + hasOwnProperty gOPD) — nowhere near the 50k-key object.
    // Object.keys() would have paid ~50k gOPD calls up front.
    expect(gopd).toBeGreaterThan(1_000)
    expect(gopd).toBeLessThan(40_000)
  })

  test("boundJsonValue reads array length exactly once", () => {
    // A Proxy get trap answering differently per read must not let the
    // allocation bound and the iteration bound diverge (#225 r5): the
    // first read passes the length check, a second read would see a
    // huge value. The implementation must read `length` ONCE and use
    // that value for both the cap check and the loop.
    let lengthReads = 0
    let indexReads = 0
    const backing = [11, 22, 33]
    const proxy = new Proxy(backing, {
      get(t, p, recv) {
        if (p === "length") {
          lengthReads++
          // First read: small (passes the bound). Any later read:
          // absurdly large — a second read would size `new Array()`
          // or the loop far beyond the checked bound.
          return lengthReads === 1 ? 3 : 100_000_000
        }
        if (typeof p === "string" && /^\d+$/.test(p)) indexReads++
        return Reflect.get(t, p, recv)
      },
    })
    const r = boundJsonValue(proxy, 0, { nodes: 0, strBytes: 0 })
    expect(lengthReads).toBe(1)
    expect(r).toEqual([11, 22, 33])
    expect(indexReads).toBe(3)
  })

  test("boundJsonValue rejects over-budget array length before indexing", () => {
    // length > MAX_INPUT_NODES must fail at the length check — the
    // element get trap must never fire (no O(len) work, no allocation
    // sized by the hostile value).
    let indexReads = 0
    const proxy = new Proxy([1, 2, 3], {
      get(t, p, recv) {
        if (typeof p === "string" && /^\d+$/.test(p)) indexReads++
        if (p === "length") return MAX_INPUT_NODES + 1
        return Reflect.get(t, p, recv)
      },
    })
    const r = boundJsonValue(proxy, 0, { nodes: 0, strBytes: 0 })
    expect(r).toBe(BOUND_FAIL)
    expect(indexReads).toBe(0)
  })

  test("boundJsonValue rejects over-budget key counts", () => {
    const big: Record<string, number> = {}
    for (let i = 0; i < MAX_INPUT_NODES + 1; i++) big[`k${i}`] = i
    const st = { nodes: 0, strBytes: 0 }
    const r = boundJsonValue(big, 0, st)
    expect(r).toBe(BOUND_FAIL)
  })

  test("boundJsonValue bounds INHERITED enumerable keys too (#225 r9)", () => {
    // for..in yields enumerable props from the prototype chain; the
    // proto check admits Object.prototype itself, so a polluted
    // Object.prototype could otherwise force unbounded enumeration —
    // hasOwnProperty-skipped keys consumed no budget before.
    const proto = Object.prototype as Record<string, unknown>
    const saved: Record<string, unknown> = {}
    for (let i = 0; i < MAX_INPUT_NODES + 10; i++) {
      const k = `inh${i}`
      saved[k] = proto[k]
      Object.defineProperty(proto, k, { value: i, enumerable: true, configurable: true, writable: true })
    }
    try {
      const st = { nodes: 0, strBytes: 0 }
      const r = boundJsonValue({ own: 1 }, 0, st)
      expect(r).toBe(BOUND_FAIL)
    } finally {
      for (const k of Object.keys(saved)) delete proto[k]
    }
  })

  test("boundJsonValue clones in-budget JSON faithfully", () => {
    const input = {
      a: [1, "two", null, true, { b: { c: [3.5] } }],
      d: "x".repeat(100),
    }
    const st = { nodes: 0, strBytes: 0 }
    const r = boundJsonValue(input, 0, st)
    expect(r).toEqual(input)
    expect(st.nodes).toBeGreaterThan(0)
  })

  test("boundJsonValue preserves __proto__ as an own data property", () => {
    // JSON.parse creates "__proto__" as an own enumerable data
    // property; a plain `out[k] = c` clone would hit the prototype
    // setter and lose the key or mutate the clone's prototype (#225 r6).
    const input = JSON.parse('{"__proto__":{"polluted":true},"ok":1}') as Record<string, unknown>
    const st = { nodes: 0, strBytes: 0 }
    const r = boundJsonValue(input, 0, st) as Record<string, unknown>
    expect(Object.prototype.hasOwnProperty.call(r, "__proto__")).toBe(true)
    expect((r as any).__proto__).toEqual({ polluted: true })
    expect(Object.getPrototypeOf(r)).toBe(Object.prototype)
    expect(({} as any).polluted).toBeUndefined()
  })

  test("boundJsonValue rejects non-JSON and oversized strings", () => {
    const st = () => ({ nodes: 0, strBytes: 0 })
    expect(boundJsonValue(undefined, 0, st())).toBe(BOUND_FAIL)
    expect(boundJsonValue(() => {}, 0, st())).toBe(BOUND_FAIL)
    expect(boundJsonValue("x".repeat(2 * 1024 * 1024), 0, st())).toBe(BOUND_FAIL)
    // Deep nesting beyond MAX_INPUT_DEPTH fails.
    let deep: unknown = 1
    for (let i = 0; i < 100; i++) deep = { v: deep }
    expect(boundJsonValue(deep, 0, st())).toBe(BOUND_FAIL)
  })

  test("utf8Slice never exceeds budget and never splits code points", () => {
    const rand = mulberry32(0x225)
    const alphabet = ["a", "é", "あ", "😀", "x".repeat(3)]
    for (let i = 0; i < 500; i++) {
      const len = Math.floor(rand() * 200)
      let s = ""
      for (let j = 0; j < len; j++) s += alphabet[Math.floor(rand() * alphabet.length)]
      const budget = Math.floor(rand() * 300)
      const out = utf8Slice(s, budget)
      expect(Buffer.byteLength(out, "utf8")).toBeLessThanOrEqual(budget)
      expect(s.startsWith(out)).toBe(true)
      // Output must be valid UTF-8 by construction (string) and a
      // strict prefix — code points are never split because for..of
      // iterates them whole.
    }
  })

  test("resultText output is always within the byte budget", () => {
    const rand = mulberry32(0x226)
    const mk = (depth: number): unknown => {
      if (depth <= 0) return rand() < 0.5 ? "t".repeat(Math.floor(rand() * 5000)) : rand()
      const kind = rand()
      if (kind < 0.4) return { text: "x".repeat(Math.floor(rand() * 3000)), content: mk(depth - 1) }
      if (kind < 0.7) return { output: Array.from({ length: Math.floor(rand() * 20) }, () => mk(depth - 1)) }
      return { content: Array.from({ length: Math.floor(rand() * 10) }, () => ({ text: "y".repeat(Math.floor(rand() * 2000)) })) }
    }
    for (let i = 0; i < 200; i++) {
      const out = resultText(mk(4))
      expect(Buffer.byteLength(out, "utf8")).toBeLessThanOrEqual(64 * 1024 + 64)
    }
    // String input path.
    expect(Buffer.byteLength(resultText("z".repeat(1_000_000)), "utf8")).toBeLessThanOrEqual(64 * 1024)
  })

  test("safeSessionID property: contract holds for arbitrary inputs", () => {
    const rand = mulberry32(0x227)
    const pieces = ["a", "Z", "0", "-", ".", "_", "!", "?", " ", "/", "\n", "é", "\u0000"]
    const seen = new Map<string, string>()
    for (let i = 0; i < 2000; i++) {
      const len = Math.floor(rand() * 300)
      let raw = ""
      for (let j = 0; j < len; j++) raw += pieces[Math.floor(rand() * pieces.length)]
      const out = safeSessionID(raw)
      // Contract: ≤128 chars, deterministic. Idempotency is NOT a
      // contract (#225 r5): injectivity and idempotency are mutually
      // exclusive for a charset-lossy map.
      expect(out.length).toBeLessThanOrEqual(128)
      expect(safeSessionID(raw)).toBe(out)
      // Charset: output is "" or [A-Za-z0-9._-] only — canonical
      // passthrough and "<head>-<32hex>" transform shapes share the
      // same safe alphabet.
      if (out !== "") expect(/^[A-Za-z0-9._-]+$/.test(out)).toBe(true)
      // Distinct raws that survive sanitization distinctly must not
      // collapse — hash suffixes make collisions cryptographically
      // improbable; assert on a sample basis.
      if (i < 500) seen.set(raw, out)
    }
    const outs = [...seen.values()]
    expect(new Set(outs).size).toBe(outs.length)
  })
})

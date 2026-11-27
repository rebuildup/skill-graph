import { describe, expect, mock, test } from "bun:test"
import { createHash } from "node:crypto"

// The plugin module registers itself via @opencode/plugin at import
// time; stub the module so the file can be imported for its pure
// helpers. Plugin.define receives the config object unchanged.
mock.module("@opencode/plugin", () => ({
  Plugin: { define: (config: unknown) => config },
}))

const { safeSessionID } = await import("./opencode-plugin")

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

# #70 — OpenCode adapter live verification

Date: 2026-11-25
Runtime: opencode v2.3.2 (`--standalone`), auth: stored "OpenCode Go" credential,
model `hungerg-free` (free tier — no paid API usage)
Test project: `/tmp/sg-oc-proj` (`opencode.json` → `plugins: ["file://.../adapters/opencode/skill-graph"]`)

## Verified live

| Item | Result |
|---|---|
| Plugin load | ✅ `loading plugin .../index.ts` in server logs; `setup()` executed, runtime is **Bun** (`typeof Bun === "object"`) — `Bun.spawn` path valid |
| `session.hook("prompt")` fires | ✅ event keys observed: `sessionID, messageID, prompt, metadata, delivery` — field names in skeleton are correct |
| Recommendation | ✅ prompt "implement a new Rust CLI tool..." → CLI returned `[alpha-skill, secrets-management, beta-skill]`; recorded into `pending` |
| `session.hook("context")` fires | ✅ ran per model call (2× observed); first call consumed `pending` and pushed system text; second call no-op — per-recommendation dedup works |
| Fail-open | ✅ "say OK" → `recommend -> []`, no injection, exit clean |
| No-save / state | in-memory only; nothing persisted by plugin itself |

## Partially verified / open

- **Injection visibility — VERIFIED (follow-up)**: asked the model to quote
  `skill-graph:` lines verbatim; it reproduced
  `skill-graph: skills relevant to this task — alpha-skill, beta-skill.`
  exactly. `event.system.push` delivery confirmed end-to-end on
  `hungerg-free`.
- **`event.prompt.skills` direct injection**: not tested; remains the
  preferred upgrade if the ID format is stable.
- Skill-load observation: OpenCode `tool.execute.*` events exist; whether
  skill invocation surfaces as a tool event is unverified (same open
  question as Codex — likely file reads, not a Skill tool).
- TUI vs `run` parity, plugin reload on file change (watcher observed
  active), and multi-session dedup across restarts (in-memory only → resets
  per server lifetime; acceptable, documented).

## Runtime-specific notes

- Plugin config: `"plugins": ["file:///abs/path"]` in `opencode.json` —
  PATH-free already (absolute path, no global install).
- Debug instrumentation added behind `SKILL_GRAPH_DEBUG=1` →
  `/tmp/sg-oc-plugin.log`; off by default.
- Free-tier model (`hungerg-free`) sufficed for hook-path verification;
  recommendation quality on this model is not evaluated here.

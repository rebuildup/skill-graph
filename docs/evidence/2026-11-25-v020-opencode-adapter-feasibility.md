# v0.2.0 #52 — OpenCode adapter 実現可能性

Date: 2026-11-25
Issue: #52
Sources: opencode.ai/docs/build/plugins (+ /server), plugins event list,
plugins migration guide.

## Feasibility verdict: feasible, different adapter shape

OpenCode has no `hooks.json` shell-hook mechanism. Its extension point is a
TypeScript plugin (`@opencode/plugin`, `Plugin.define({ id, setup(ctx) })`)
loaded via the `plugins` array in `opencode.json`. The plugin runs inside the
OpenCode server and can subscribe to session lifecycle hooks.

## Verified from documentation

| Requirement | OpenCode support |
|---|---|
| Per-prompt recommendation trigger | `ctx.session.hook("prompt", ...)` — intercepts admitted user prompts before attachment/skill resolution; receives mutable draft with `prompt.text`, `files`, `agents`, `skills` |
| Context injection | `ctx.session.hook("context", ...)` — `event.system.push({type:"text",text})` adds system text to outgoing model calls |
| Skill-load observation | `ctx.event.subscribe` exposes `tool.execute.before/after`, `message.*`, `session.*` events; whether skill invocation surfaces as a tool event is unverified |
| Direct skill selection | `event.prompt.skills` is mutable — a hook can add selected skills, potentially a stronger integration than text advice (needs live verification of ID format) |
| Skill registry access | `ctx.skill.list()` / `ctx.skill.transform(...)` — plugin can enumerate OpenCode's own skills |
| Dedup / fail-open | Plugin-side in-memory state; CLI errors → no injection |
| PATH-free | Plugin spawns the `skill-graph` binary by absolute path (`SKILL_GRAPH_BIN` env) |

## Constraints and differences vs Claude Code/Codex

- **Language/runtime**: plugin is TypeScript running in the OpenCode server
  (Bun-based; `Bun.spawn` used in the skeleton — Node fallback may be needed).
  This is the first non-shell adapter; it cannot reuse `skill-graph-recommend-hook`
  directly but shells out to the same `skill-graph recommend` CLI contract.
- **No hook trust gate** equivalent to Codex's `/hooks` review; plugins are
  declared in `opencode.json` (project or user config).
- **Injection timing**: `prompt` hook mutates the user prompt draft;
  `context` hook affects only the outgoing model call, not persisted history.
  The skeleton records recommendations at prompt time and injects at the next
  context hook — ordering between the two hooks for the same turn needs live
  verification.
- **Plugin distribution**: local path or package specifier in `opencode.json`;
  npm publishing is a later concern (#53 covers PATH-free distribution).

## Deliverable in this PR

- `adapters/opencode/skill-graph/index.ts` — UNVERIFIED skeleton implementing
  the prompt→context injection design.
- `adapters/opencode/skill-graph/README.md` — design, install plan, open
  questions.

## Pending live verification

1. Install/load the plugin in a real OpenCode server; confirm `setup` runs and
   hooks register without errors.
2. Confirm `prompt` hook receives `event.prompt.text` and `sessionID` field
   names; adjust skeleton accordingly.
3. Confirm `context` hook `event.system.push` shape and that injected text
   reaches the model.
4. Test whether `event.prompt.skills` accepts skill-graph recommendations
   directly (preferred over text injection if stable).
5. Confirm runtime (`Bun` vs `node:child_process`) and adjust spawn code.
6. Measure end-to-end latency (CLI spawn ~9ms + plugin overhead).

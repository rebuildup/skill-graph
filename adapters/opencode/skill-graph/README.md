# skill-graph adapter for OpenCode

Opt-in OpenCode plugin that surfaces skill-graph recommendations via the
native plugin API (`@opencode/plugin`). Unlike the Claude Code/Codex shell-hook
adapters, OpenCode's extension point is a TypeScript plugin running inside the
OpenCode server — there is no `hooks.json` equivalent for prompt submission.

## Status: live-verified core path (opencode 2.3.2, 2026-11-25)

Plugin loads under Bun, `session.hook("prompt")`/`("context")` fire with the
field names used here, recommendation→injection executes, fail-open and
dedup verified. Injection *visibility* to the model and `prompt.skills`
direct selection remain open — see
`docs/evidence/2026-11-25-v020-opencode-live-verification.md`.

`index.ts` is written against the documented plugin API but has not been run
against a live OpenCode server. See
`docs/evidence/2026-11-25-v020-opencode-adapter-feasibility.md` for the design
basis and the verification checklist.

## Design

- `ctx.session.hook("prompt", ...)` — intercepts each admitted user prompt
  before attachment/skill resolution. We shell out to the `skill-graph` CLI
  (`recommend --format json`) and record hits per session.
- `ctx.session.hook("context", ...)` — on the next model call, injects the
  recorded names as a system text part (`event.system.push`), mirroring the
  `additionalContext` behavior of the shell adapters.
- Dedup is in-memory per plugin instance (one emission per session+prompt).
  Fail-open: any CLI error yields no injection.

## Install (planned)

1. `cargo build --release` for the `skill-graph` binary.
2. Set `SKILL_GRAPH_BIN` to its absolute path and optionally
   `SKILL_GRAPH_ROOTS` (colon-separated) in the environment that launches
   OpenCode.
3. Add the plugin to `opencode.json`:
   `"plugins": ["file:///absolute/path/to/adapters/opencode/skill-graph"]`
   (exact specifier format pending live verification).

## Open questions for live verification

- Exact event field names (`sessionID` vs `session.id`, prompt draft shape).
- Whether `event.prompt.skills` can be populated directly with skill IDs —
  that would be a stronger integration than system-text advice.
- Runtime: whether plugins execute under Bun (`Bun.spawn`) or Node
  (`node:child_process` fallback needed).
- Whether `context` hook fires once per model call (dedup must be per
  recommendation, not per call — handled via `pending` map consumed once).

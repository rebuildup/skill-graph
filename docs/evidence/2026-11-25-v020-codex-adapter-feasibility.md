# v0.2.0 #51 — Codex adapter 実現可能性

Date: 2026-11-25
Issue: #51
Sources: Codex Hooks docs (learn.chatgpt.com/docs/hooks), Codex skills docs
(build-skills), codex-rs source references.

## Feasibility verdict: feasible, near drop-in

Codex's hook system uses the same `hooks.json` + matcher + command-hook shape
and the same `hookSpecificOutput.additionalContext` output contract as Claude
Code. The existing `skill-graph-recommend-hook` script works unchanged for
`UserPromptSubmit` (`.prompt`, `.session_id`, `.cwd` fields are identical).

## Verified from documentation

| Requirement | Codex support |
|---|---|
| Session-start / per-prompt recommendation | `UserPromptSubmit` hook, `.prompt` input, `additionalContext` output |
| Mid-work trigger | `PostToolUse` with `tool_name` matcher (Bash, apply_patch, MCP tools, local function tools) |
| Skill-load observation | **Unverified** — Codex skills are invoked via `$name`/`/skills`, not confirmed to surface as a `Skill` tool call. The `PostToolUse`/`Skill` matcher is included but may never fire. |
| Dedup / no-output / fail-open | Script-level, runtime-agnostic — unchanged |
| PATH-free execution | Command string accepts absolute paths; no plugin-root variable expansion (unlike `${CLAUDE_PLUGIN_ROOT}`), so install must substitute the absolute hook path |
| Non-pollution | Opt-in: user adds entries to `$CODEX_HOME/hooks.json` or trusted `.codex/hooks.json`; nothing is installed globally by default |

## Codex-specific constraints

- **Hook trust gate**: non-managed hooks must be reviewed and trusted in `/hooks`
  before running; trust is pinned to the hook definition hash, so adapter
  updates require re-trust. `--dangerously-bypass-hook-trust` exists for
  one-off automation but is not a normal install path.
- **Skills are experimental**: behind `[features] skills = true` or
  `--enable skills`; discovery roots are `.agents/skills` (repo chain),
  `~/.agents/skills`, `/etc/codex/skills`, legacy `~/.codex/skills`, plugin
  bundles. Same-name skills are NOT merged (consistent with our identification
  contract — recommend output stays name-grouped, resolve stays ambiguous).
- **`command` is a single string** (no separate `args` field like Claude
  plugin hooks.json); arguments are embedded in the command string.
- Hook `timeout` defaults to 600s; our script finishes in <50ms — set an
  explicit small timeout in production installs.

## Deliverable in this PR

- `adapters/codex/skill-graph/hooks.json` — template with `UserPromptSubmit`
  and `PostToolUse`/`Skill` entries.
- `adapters/codex/skill-graph/bin/skill-graph-recommend-hook` — same script as
  the Claude Code adapter; default roots adjusted for Codex locations.
- `adapters/codex/skill-graph/README.md` — install steps, trust gate, limits.

## Pending live verification (requires Codex CLI + auth)

1. `UserPromptSubmit` fires and `additionalContext` reaches the model.
2. Whether skill invocation produces any hook-observable tool call (determines
   if dependency injection on skill load is possible in Codex).
3. Behavior under `/hooks` trust review for a merged hooks.json.
4. Session dedup state path behavior under Codex's cwd rules.

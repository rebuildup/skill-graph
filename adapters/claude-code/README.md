# skill-graph — Claude Code adapter

Opt-in adapter that recommends installed Agent Skills for each user prompt via a `UserPromptSubmit` hook. It is loaded only when this directory is passed to Claude Code (e.g. `--plugin-dir`) or installed through a plugin mechanism the user explicitly enables. It does not modify settings, skills, or session logs.

## Requirements

- A `skill-graph` binary (release build) on `PATH`, or an absolute path via `SKILL_GRAPH_BIN`.
- `jq` on `PATH` (used to parse hook payloads).
- Agent Skills installed under conventional roots: `~/.claude/skills`, `~/.agents/skills`, `<cwd>/skills`, `<cwd>/.claude/skills`, `<cwd>/.agents/skills`. Override with `SKILL_GRAPH_ROOTS` (colon-separated absolute paths). Duplicate installs across roots are deduplicated by skill name (C04).

## Install / enable (explicit opt-in)

```bash
claude --plugin-dir /path/to/adapters/claude-code/skill-graph
```

For repeated use, install through your usual Claude Code plugin flow pointing at this directory. Nothing is written outside the session; hook state and logs go to `${TMPDIR:-/tmp}` (`skill-graph-hook-state/`, `skill-graph-hook.jsonl`, overridable via `SKILL_GRAPH_HOOK_STATE` / `SKILL_GRAPH_HOOK_LOG`).

## Behavior

Two selective hooks (ADR-0003):

- `UserPromptSubmit` (`prompt` mode): recommends up to 3 skills relevant to the submitted prompt.
- `PostToolUse` matcher `Skill` (`skill-load` mode): when a skill finishes loading, surfaces its declared `requires` dependencies (up to 3) so mid-work dependency gaps are visible without a new prompt.

Both emit `additionalContext` only on hits; silent on no recommendations, dedup repeats (per session + prompt/skill), missing binary, or empty roots — fail-open, exit 0. No hooks on Bash/Edit/Write or other unconditional matchers.

- Latency: ~10–40 ms per hook invocation measured (release binary, real duplicated skill corpus).
- No-save mode: `SKILL_GRAPH_HOOK_NOSTATE=1` disables dedup markers and log writes entirely.

## Disable / remove

Stop passing `--plugin-dir` / uninstall the plugin. Residue is limited to optional files under `${TMPDIR}` which the OS cleans; delete `skill-graph-hook-state/` and `skill-graph-hook.jsonl` to remove immediately.

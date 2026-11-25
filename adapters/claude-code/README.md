# skill-graph — Claude Code adapter

Opt-in adapter that recommends installed Agent Skills for each user prompt via a `UserPromptSubmit` hook. It is loaded only when this directory is passed to Claude Code (e.g. `--plugin-dir`) or installed through a plugin mechanism the user explicitly enables. It does not modify settings, skills, or session logs.

## Requirements

- A `skill-graph` binary (release build) on `PATH`, or an absolute path via `SKILL_GRAPH_BIN`.
- `jq` on `PATH` (used to parse hook payloads).
- Agent Skills installed under conventional roots: `~/.claude/skills`, `~/.agents/skills`, `<cwd>/skills`, `<cwd>/.claude/skills`, `<cwd>/.agents/skills`. Override with `SKILL_GRAPH_ROOTS` (colon-separated absolute paths).

## Install / enable (explicit opt-in)

```bash
claude --plugin-dir /path/to/adapters/claude-code/skill-graph
```

For repeated use, install through your usual Claude Code plugin flow pointing at this directory. Nothing is written outside the session; hook state and logs go to `${TMPDIR:-/tmp}` (`skill-graph-hook-state/`, `skill-graph-hook.jsonl`, overridable via `SKILL_GRAPH_HOOK_STATE` / `SKILL_GRAPH_HOOK_LOG`).

## Behavior

- Emits `additionalContext` with up to 3 recommended skill names only when the recommender returns candidates; otherwise silent.
- Dedups per `session_id` + prompt hash.
- Fails open: missing binary, parse errors, or empty roots → exit 0 with no output.
- Latency budget: hook total < 250 ms (measured ~40 ms end-to-end on a 7-skill fixture, debug build; see evidence).

## Disable / remove

Stop passing `--plugin-dir` / uninstall the plugin. Residue is limited to optional files under `${TMPDIR}` which the OS cleans; delete `skill-graph-hook-state/` and `skill-graph-hook.jsonl` to remove immediately.

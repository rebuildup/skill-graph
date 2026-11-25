# Evidence — P02 Claude Code runtime adapter (2026-11-25)

Issue: #29
Branch: `29` → `release-0-1-0`

## Scope

Productize the X03-validated recommendation path as an opt-in Claude Code adapter. Only Claude Code is included — it is the single runtime with measured hook feasibility (S02/X03); other runtimes stay out per ADR-0002 until measured.

## Package

`adapters/claude-code/skill-graph/`:

```text
.claude-plugin/plugin.json   # name/version/description/author
hooks/hooks.json              # UserPromptSubmit → bin/skill-graph-recommend-hook
bin/skill-graph-recommend-hook
```

`adapters/claude-code/README.md` documents requirements, opt-in enable (`--plugin-dir` or usual plugin flow), behavior, and complete removal (stop loading; optional `${TMPDIR}` residue only).

The adapter deliberately ships **no skill copy** — the portable `plugins/skill-graph` package (P01) owns the canonical `SKILL.md`; duplicating it here would create a second source of truth.

## Hook behavior (`bin/skill-graph-recommend-hook`)

- Reads the `UserPromptSubmit` payload, resolves skill roots (`SKILL_GRAPH_ROOTS` colon-separated override, else `~/.claude/skills`, `~/.agents/skills`, `$PWD/{skills,.claude/skills,.agents/skills}`), runs `skill-graph recommend <prompt> --max 3 --format json` via `SKILL_GRAPH_BIN` (default `skill-graph` on PATH).
- Emits `additionalContext` with up to 3 names + "read the matching SKILL.md" only on hits; silent otherwise.
- Dedup per `session_id` + prompt hash under `${TMPDIR}/skill-graph-hook-state/` (override `SKILL_GRAPH_HOOK_STATE`).
- Fails open: missing binary / parse error / no roots → exit 0, no output. Logs JSONL to `SKILL_GRAPH_HOOK_LOG` (default `${TMPDIR}/skill-graph-hook.jsonl`).

## Verification

- `claude plugin validate adapters/claude-code/skill-graph` → **passed** (only non-blocking icon warning; `${CLAUDE_PLUGIN_ROOT}` quoting warning fixed).
- Direct hook runs (`SKILL_GRAPH_BIN=target/release/skill-graph`, `SKILL_GRAPH_ROOTS=fixtures/skills`):
  - `write tests for the rust cli` → emitted `alpha-skill, beta-skill` (graph `requires` admission visible through the real CLI), `elapsed_ms=28`.
  - Same session+prompt replay → `dedup_skip`, no output.
  - Gibberish prompt → `no_recommendations`, no output.
  - `SKILL_GRAPH_BIN=/nonexistent` → `recommender_error`, exit 0, no output (fail-open).
- Real Claude Code run (`claude -p "release notes changelog" --plugin-dir .../skill-graph --include-hook-events`, auth-blocked model turn): `hook_response` contained `additionalContext: "skill-graph: skills relevant to this task — gamma-skill. ..."`; hook log `emitted=true, recommend_ms=5, elapsed_ms=29`.

## Non-pollution

- No writes to the target project, user settings, skills, or session logs; state/log confined to `${TMPDIR}` and fully removable.
- Loading is explicit opt-in (`--plugin-dir` / plugin install); nothing is registered globally.

## Limits / next

- Live invocation recall still unmeasured — shared Claude Code auth blocker with S02 (#4); rerun after login to observe whether injected names actually get loaded.
- Real installed-skills corpus latency not yet re-measured (fixtures only, 7 skills); C03 synthetic data suggests large headroom.
- Multi-prompt dedup within one live session untested (single `-p` prompt per session); script-level dedup verified.

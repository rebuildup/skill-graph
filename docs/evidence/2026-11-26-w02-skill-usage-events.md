# W02 Skill Usage Event contract + observe extension (#77)

Date: 2026-11-26. Branch: `77`.

## What changed

- `contracts/v1/skill-usage-event.schema.json`: v1 contract for
  `prompt_submit` / `recommendation` / `skill_load`. Prompts are SHA-256
  hashed only; no conversation bodies, command lines, or tool responses are
  persisted. `detection` distinguishes `native_invocation` / `file_read` /
  `path_reference` / `mention` / `inferred` / `applied`.
- `src/events.rs` (new): shared classifier used by `detect` (hook/plugin
  stdin path) and `observe` normalization — command read-verb parsing,
  tool-name gating, frontmatter corroboration, name normalization.
- `src/observe.rs`: normalizes Claude Code JSONL transcripts, Codex rollout
  JSONL (incl. `custom_tool_call`/`exec_command` JS), OpenCode `opencode.db`
  (read-only `rusqlite`), and passes through contract-conformant JSONL event
  logs. Existing logs/DBs are opened read-only.
- `skill-graph detect --runtime <rt> --via <via>`: single-event stdin
  normalizer for hook/plugin emitters.
- Adapters emit the same contract to
  `${TMPDIR}/skill-graph-events.jsonl` (`SKILL_GRAPH_EVENT_LOG`):
  - claude-code / codex shell hook: `prompt` mode emits `prompt_submit` +
    `recommendation`; `skill-load` mode pre-filters with jq (Skill tool or
    SKILL.md-referencing tool_input) then calls `detect`, emitting
    `skill_load` and a dependency `recommendation` only for load-grade
    detections.
  - opencode plugin: `session.hook("prompt")` emits `prompt_submit` /
    `recommendation` / `inferred` (`prompt.skills`);
    `tool.hook("execute.after")` early-excludes non-SKILL.md tools before
    spawning `detect`.
  - Codex `hooks.json` / `install.sh`: `PostToolUse` matcher changed
    `Skill` → `Bash` (Codex has no native skill tool).

## Verification

- `tests/observe.rs`: 6 tests covering Claude transcript skill_load +
  file_read, Codex rollout normalization with injected recommendation,
  contract JSONL passthrough, OpenCode DB normalization.
- Live observe runs: codex 14 events/7 files, opencode.db 47 events,
  claude transcripts 56 events/31 files — all normalized without mutating
  sources.
- Hook script manual test (codex path): prompt mode emits `prompt_submit` +
  `recommendation`; skill-load mode with `cat .../SKILL.md` emits
  `skill_load` (`file_read`) + dependency `recommendation`.

## Boundaries kept

- Detection and recommendation are separate: non-skill tool calls exit
  before the CLI runs; no corpus scan or context injection for them.
- Event log lives only under `${TMPDIR}`; `SKILL_GRAPH_HOOK_NOSTATE=1`
  disables all writes. Old `SKILL_GRAPH_HOOK_LOG` custom format retired.
- Persistent cache / MCP / Embedding remain deferred.

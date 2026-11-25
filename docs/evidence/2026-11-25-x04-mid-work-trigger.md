# Evidence — X04 mid-work recommendation trigger (2026-11-25)

Issue: #34
Branch: `34` → `release-0-1-0`

## Scope

Verify and implement a minimal mid-work recommendation path covering skill-load-time dependency surfacing (group D realized at the right trigger). Live Claude Code verification remains auth-blocked (S02/#4); this ticket delivers design + script-level verification.

## Change

`adapters/claude-code/skill-graph/`:

- `hooks/hooks.json`: added `PostToolUse` matcher `Skill` → same hook binary with `args: ["skill-load"]`. `UserPromptSubmit` unchanged (`args: ["prompt"]`).
- `bin/skill-graph-recommend-hook`: two modes.
  - `prompt`: unchanged behavior — recommend for the submitted prompt, top 3.
  - `skill-load`: extracts the loaded skill name from `tool_input.skill|name|skill_name` (normalized: strips path prefix and `plugin:` prefix), runs `recommend <name>`, drops the loaded skill itself, keeps up to 3 declared dependencies, emits `hookEventName: "PostToolUse"` `additionalContext`.
  - New `SKILL_GRAPH_HOOK_NOSTATE=1`: no-save mode — no dedup markers, no log writes.
- Design recorded in `docs/adr/ADR-0003-mid-work-recommendation-trigger.md`.

## Why `recommend <name>` is sufficient

No new CLI surface was needed: querying the loaded skill's name admits it by name, and its resolved `requires` links admit dependencies with a necessity reason — exactly the declared-dependency set. C04 (#35) made this work on duplicated multi-root installs (previously returned `[]`).

## Script-level verification (synthetic payloads)

Env: `SKILL_GRAPH_BIN=target/release/skill-graph`, `SKILL_GRAPH_ROOTS=.claude/skills:.agents/skills` (the repo's real duplicated skill set).

| Case | Result |
|---|---|
| PostToolUse Skill `system-quality` | emitted `additionalContext`: `'system-quality' declares dependencies — code-naming`, `recommend_ms=11`, `elapsed_ms=40` |
| PostToolUse Skill `code-naming` (no requires) | silent, `reason=no_recommendations` |
| Same skill again, same session | silent, `reason=dedup_skip` |
| `plugin-x:system-quality` (prefixed name) | normalized → `dedup_skip` (same dedup key) |
| UserPromptSubmit prompt mode | unchanged path, silent on no match |
| `SKILL_GRAPH_HOOK_NOSTATE=1` | no state dir, no log file created |

`claude plugin validate adapters/claude-code/skill-graph` → passed (icon warning only).

## Cost / noise bounds

- One process spawn per Skill load (~10–40 ms measured on the real 34-skill duplicated corpus); silent when the loaded skill has no resolvable `requires`.
- Dedup is per session + skill name; repeated loads of the same skill emit once.
- No hooks added for Bash/Edit/Write or any unconditional tool matcher.

## Limits / pending live verification

- Real `PostToolUse` payload field for the loaded skill (`tool_input.skill` vs other) is unverified — the script accepts `skill|name|skill_name` fallbacks and fails open to silence.
- Whether `PostToolUse` fires for the `Skill` tool at all, and whether injected `additionalContext` converts into actual skill loads, requires an authenticated Claude Code session (S02/#4).
- Markdown references of a loaded skill are intentionally not surfaced (weak evidence); revisit only if live data shows `requires` coverage is insufficient.

# ADR-0003: Mid-work recommendation trigger via Skill-load observation

Status: accepted for implementation; live verification pending Claude Code auth  
Date: 2026-11-25

## Context

ADR-0002 fixed `UserPromptSubmit` as the first intervention path. However the motivating failure — `system-quality` loaded while `code-naming` (its declared `requires-skills` dependency) is never reached — happens *inside* a session: the Skill tool loads a skill mid-turn and no new user prompt fires. `UserPromptSubmit` alone cannot cover skill-load-time or intra-turn task drift.

## Decision

Add a second, selective hook: `PostToolUse` with `matcher: "Skill"`. When a skill finishes loading, the hook resolves the loaded skill's name and emits `additionalContext` listing its declared `requires` dependencies (via `recommend <name>` minus the loaded skill, capped at 3). It emits nothing when the loaded skill declares no resolvable dependencies.

Explicitly not adopted:

- Hooks on `Bash`/`Edit`/`Write` or other tools — unconditional per-tool hooks would fire on every operation, adding latency and context cost without a skill-selection signal.
- Score-based mid-work recommendation — the trigger payload carries no task text; declared references are the only high-precision signal available.
- Markdown-link/mention surfacing at load time — weak evidence stays weak; `requires` declarations are the contract-backed dependency signal.

## Rationale

- The stated root problem is *declared* dependencies the agent cannot know it needs. `PostToolUse/Skill` fires exactly when a skill enters context, making it the minimal event that covers the gap.
- C04 (#35) made `requires` resolution and recommendation work on duplicated multi-root installs, which is the normal shape (`.claude/skills` + `.agents/skills`).
- Cost is bounded: one extra process spawn per skill load (~10–40 ms measured), silent output in the common no-dependency case, per-session dedup.

## Consequences

- Adapter now has two hook modes: `prompt` (UserPromptSubmit) and `skill-load` (PostToolUse/Skill). Both share dedup, fail-open, and no-output behavior.
- `SKILL_GRAPH_HOOK_NOSTATE=1` provides a no-save mode (no markers, no log).
- Live verification (real `tool_input` payload shape, whether `PostToolUse` fires for `Skill`, whether injected names convert to loads) is pending Claude Code authentication — tracked under S02 (#4) / X04 (#34).
- If live payloads show `PostToolUse` does not fire for `Skill` or `tool_input.skill` is absent, fall back to `UserPromptExpansion` observation or document the gap; do not add broader tool matchers without evidence.

# Evidence — S01 standards and existing-tool survey (2026-11-25)

Issue: #3
Branch: `3` → `release-0-1-0`

## What changed

`docs/specs/standards.md` now records each candidate standard with status, version/revision, source URL, and compatibility note. Runtime notes keep Agent Skills metadata discovery common while treating Hook events, payloads, context injection, and trust boundaries as runtime-specific. The unstable Agent Skills Discovery proposal is separated from normative specs and its exact draft revision is deferred to I01.

## Survey result

- Agent Skills remains the compatibility baseline: standard `SKILL.md` + YAML frontmatter, `name`/`description` required, no custom required frontmatter.
- Agent Plugins v1.0.0 is the first distribution candidate; client extension namespaces carry runtime-specific Hooks.
- CommonMark/YAML/URI/JSON Schema/MCP/XDG/SemVer are recorded with sources; JSON Schema draft and MCP version are fixed at adapter implementation time.
- Existing tools are mostly external registry/directory search. skill-graph's differentiation stays on local reference graph, work context, observation state, and recommendation rationale.

## Verification

Documentation-only change; no code verification required beyond Markdown review. `git diff` shows only `docs/specs/standards.md`.

# Evidence — P01 CLI / standard plugin distribution (2026-11-25)

Issue: #28
Branch: `28` → `release-0-1-0`

## Scope

Verify the MVP core ships as (a) a working release CLI and (b) an Agent Plugins v1.0.0-conformant package. Client-specific hook adapters remain P02 (#29).

## Package

`plugins/skill-graph/`:

```text
plugins/skill-graph/
├── plugin.json                      # Agent Plugins 1.0.0 manifest
└── skills/
    └── skill-graph/
        └── SKILL.md                 # portable skill: when/how to run the CLI
```

- `plugin.json` targets `https://agent-plugins.org/schemas/1.0.0/plugin.schema.json` with closed top-level fields only (`$schema`, `name`, `version`, `description`, `license`, `keywords`, `repository`).
- No `mcp.json` (MCP deferred per ADR-0002); absent fixed locations are non-errors per spec §6.2.
- No client extension directory yet — hooks are client-specific and land in P02.

## Verification

- Manifest: structural validation against the fetched official schema (`$schema` const, required `name`, name pattern, closed field set, field types) — PASS. A full JSON-Schema validator was not installed; the check covers all constraints present in `plugin.schema.json` for the fields used.
- `SKILL.md`: Agent Skills frontmatter `name` + `description` present; body documents commands, roots, output interpretation, and the untrusted-data boundary.
- Release build: `cargo build --release` clean; smoke run on `fixtures/skills`:
  - `discover` → 7 skills, 0 errors
  - `recommend "release notes changelog"` → `["gamma-skill"]`
  - `resolve alpha-skill` → `kind: name`, `ambiguous: false`, digest present
  - `bench evals/dataset/cases.jsonl` → required_recall 1.0, forbidden_hits 0
- PATH-free execution: already verified for hook-side executables in S02/X03 (`${CLAUDE_PLUGIN_ROOT}/bin/...` ran without PATH install). The portable package itself ships no binaries; executable provisioning is an adapter concern (see below).

## Binary provisioning decision

The Agent Plugins v1 portable core standardizes `skills/` and `mcp.json` only; it does not define executable bundling or hooks. Per-OS binary distribution (signing, notarization, store delivery) is release engineering, not package conformance. The shipped `SKILL.md` therefore instructs the agent to use `skill-graph` from `PATH` or an adapter-provided absolute path. P02 decides how the Claude Code adapter materializes the binary (e.g. `${CLAUDE_PLUGIN_ROOT}`-relative or managed install) and re-verifies PATH-free hook execution in that form.

## Limits / next

- `claude plugin validate` targets Claude Code's own `.claude-plugin/` format, not the portable `plugin.json`; conformance here is to the Agent Plugins schema, with Claude-runtime loading deferred to P02's adapter.
- Real-corpus latency re-measurement on installed skills remains for P02.

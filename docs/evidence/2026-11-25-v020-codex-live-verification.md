# #70 — Codex adapter live verification

Date: 2026-11-25
Runtime: codex-cli 0.171.0, auth: ChatGPT login (`codex login status` → Logged in)
Test project: `/tmp/sg-codex-proj` (`.codex/hooks.json` project layer, `.agents/skills/{alpha,beta}-skill` fixtures)
Flags: `codex exec --trust --dangerously-bypass-hook-trust --skip-git-repo-check`
(`--dangerously-bypass-hook-trust` used because this is one-off automation of
our own vetted hook; the normal path is `/hooks` review+trust — see below.)

## Verified live

| Item | Result |
|---|---|
| `UserPromptSubmit` hook fires | ✅ `hook: UserPromptSubmit` / `Completed` in exec output; script logged `mode=prompt` |
| Recommendation emitted | ✅ prompt "implement a new Rust CLI tool..." → `emitted:true`, recommended `[alpha-skill, secrets-management, beta-skill]` in 33ms |
| `additionalContext` reaches model | ✅ rollout JSONL (`~/.codex/sessions/**/rollout-*.jsonl`) contains `skill-graph: skills relevant to this task — alpha-skill, ...` as developer context |
| No-recommendation silence | ✅ "Reply with exactly: PONG" → `no_recommendations`, no output |
| Multi-turn (resume) | ✅ `codex exec resume --last` fires hook again per prompt; dedup/state works per session |
| Fail-open | ✅ missing roots/binary paths exit 0 silently (script contract unchanged) |
| Trust gate | ⚠️ project `.codex/` layer is **disabled until trusted**: first run printed "Untrusted project-local configuration is disabled. Run `codex exec --trust`". `--trust` persisted project trust; `--dangerously-bypass-hook-trust` skipped per-hook review for the test. **Normal install flow = `/hooks` review in TUI or `--trust`+hook trust** — install.sh output already notes `/hooks`. |

## Verified NEGATIVE result (design boundary)

- **`PostToolUse` matcher `Skill` never fires**: Codex has no `Skill` tool.
  Skills are discovered from `.agents/skills` etc. and the model reads
  `SKILL.md` via ordinary file/Bash tool calls (`exec cat .agents/skills/...`
  observed in rollout). Skill-load observation via tool hooks is therefore
  **not available** in Codex; dependency injection on skill load is
  impossible through this mechanism.
- Possible future alternative: `PostToolUse` matcher `Bash` parsing
  `tool_input.command` for `SKILL.md` reads — fragile, not implemented.

## Runtime-specific notes

- `codex exec` is single-turn; multi-turn verified via `resume --last`
  (requires `--skip-git-repo-check` outside git repos).
- Skills feature: `--enable skills` rejected in 0.171.0 ("Unknown feature
  flag"); skill discovery worked anyway — skills appear to be loaded by
  default in this version (docs called it experimental; behavior may differ
  across versions — recheck on upgrade).
- Model used: `gpt-6.2-sol` via ChatGPT auth (user's existing subscription
  path; no new paid API usage introduced by this verification).
- Hook latency ~31–33ms per invocation (within budget).

## Remaining for #70

- OpenCode plugin live test (separate runtime).
- Codex: namespaced/same-name skill identification live check (engine-level
  contract already tested; live corpus had no collisions).
- Whether to keep or drop the inert `PostToolUse`/`Skill` entry in the Codex
  adapter (harmless; documents intent if Codex later adds a Skill tool).

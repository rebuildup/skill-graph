# Evidence — C06 Claude adapter hook command quoting (2026-11-25)

Issue: #43
Branch: `43` → `release-0-1-0`

## Finding

Claude Code 2.1.329 loaded `adapters/claude-code/skill-graph` but failed to spawn its hook command. `stream-json --include-hook-events` reported `outcome:"error"`, `exit_code:1`:

```text
ENOENT: no such file or directory, posix_spawn '"/home/basic/work/skill-graph/adapters/claude-code/skill-graph/bin/skill-graph-recommend-hook"'
```

`hooks.json` wrapped `${CLAUDE_PLUGIN_ROOT}` in double quotes. Claude Code expanded the variable but passed the quote characters to `posix_spawn`, so the executable path did not exist. The experiment plugin used the unquoted form and worked.

## Change

`adapters/claude-code/skill-graph/hooks/hooks.json` now uses the unquoted command form for both `UserPromptSubmit` and `PostToolUse` hooks:

```json
"command": "${CLAUDE_PLUGIN_ROOT}/bin/skill-graph-recommend-hook"
```

No hook logic changed.

## Verification

`claude plugin validate adapters/claude-code/skill-graph` passed. The remaining warnings are the known missing icon/hosts metadata, unchanged by this fix.

A deterministic local Anthropic-compatible mock endpoint (`/tmp/sg-s02/mock_anthropic.py`, not committed) was used because the Claude OAuth token is expired and the Infisical user session is expired. This verifies Claude Code hook mechanics only; it does not measure model quality or invocation recall.

Command shape:

```bash
cd /tmp/sg-s02/project && env -u ANTHROPIC_API_KEY \
  ANTHROPIC_AUTH_TOKEN=mock-token ANTHROPIC_BASE_URL=http://127.0.0.1:8787 \
  ANTHROPIC_MODEL=mock-model ANTHROPIC_SMALL_FAST_MODEL=mock-model \
  ANTHROPIC_DEFAULT_OPUS_MODEL=mock-model ANTHROPIC_DEFAULT_SONNET_MODEL=mock-model \
  ANTHROPIC_DEFAULT_HAIKU_MODEL=mock-model \
  SKILL_GRAPH_BIN=/home/basic/work/skill-graph/target/release/skill-graph \
  SKILL_GRAPH_ROOTS=/home/basic/work/skill-graph/fixtures/skills \
  SKILL_GRAPH_HOOK_LOG=/tmp/sg-s02/logs/hook.jsonl SKILL_GRAPH_HOOK_STATE=/tmp/sg-s02/state \
  claude -p "implement Rust service" \
  --plugin-dir /tmp/sg-s02/plugin/sg-s02-probe \
  --plugin-dir /home/basic/work/skill-graph/adapters/claude-code/skill-graph \
  --setting-sources project --no-session-persistence \
  --output-format stream-json --include-hook-events --verbose \
  --permission-mode bypassPermissions
```

Observed with the fixed hooks:

- `UserPromptSubmit` fired and returned `additionalContext` listing `alpha-skill, beta-skill`; `hook_response.outcome=success`, `exit_code=0`.
- The mock model emitted a `Skill` tool call for `sg-s02-probe:alpha-skill`. Claude Code resolved it to the plugin skill, returned `tool_use_result.success=true`, and injected the skill body as a synthetic user message.
- `PostToolUse:Skill` fired. The hook received `tool_name:"Skill"` and `tool_input.skill:"sg-s02-probe:alpha-skill"`, normalized it to `alpha-skill`, and returned `additionalContext` naming declared dependency `beta-skill`; `hook_response.outcome=success`.
- The next model request contained the hook context as a system message: `PostToolUse:Skill hook additional context: skill-graph: 'alpha-skill' declares dependencies — beta-skill...`.

Additional live-runtime checks on the same mock path:

- No recommendation: prompt `zzz qqq unrelated` produced `hook_response.output=""`, `outcome=success`, and hook log `reason=no_recommendations`.
- Fail-open: `SKILL_GRAPH_BIN=/nonexistent/skill-graph` produced `hook_response.output=""`, `outcome=success`, and hook log `reason=recommender_error`; the session completed.
- Session dedup: `--input-format stream-json` with the same prompt twice in one session produced two `UserPromptSubmit` hook events; the first emitted context and the second returned empty output with hook log `reason=dedup_skip`.

## Boundary

This evidence covers hook delivery, payload shape, additionalContext injection, no-output, fail-open, and session dedup mechanics. It does not cover real model behavior, recommendation→actual-load conversion, Baseline/E/F invocation recall, mid-work task drift under a real model, or artifact quality. Those remain blocked on an authenticated external-provider route.

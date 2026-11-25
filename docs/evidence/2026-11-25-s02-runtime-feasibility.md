# Evidence — S02 runtime / plugin / hook feasibility (2026-11-25)

Issue: #4
Branch: `4` → `release-0-1-0`

## Sources checked

- Claude Code docs: `hooks`, `plugins/components`, `plugins/manifest-reference`, `skills` (`code.claude.com/docs/en/*`).
- Local CLIs: `claude 2.1.329`, `codex 0.171.0`, `opencode 2.3.2` are installed; this experiment used Claude Code only.
- `claude --help`: `--plugin-dir`, `--setting-sources`, `--no-session-persistence`, `--output-format stream-json`, `--include-hook-events` exist.

## Isolated experiment

Assets are under `experiments/claude-code/plugin-skill-graph/` and were loaded only for one session with `--plugin-dir`; no user/project settings were edited.

```bash
claude plugin validate experiments/claude-code/plugin-skill-graph
cd /tmp/sg-s02/project && env SG_S02_LOG=/tmp/sg-s02/hooks.jsonl SG_S02_STATE=/tmp/sg-s02/state \
  claude -p "/skill-graph-demo:recommender" \
  --plugin-dir /home/basic/work/skill-graph/experiments/claude-code/plugin-skill-graph \
  --setting-sources project --no-session-persistence \
  --output-format stream-json --include-hook-events --verbose \
  --permission-mode bypassPermissions
```

`claude plugin validate` passed with warnings only (missing author/icon; hosts not declared). Logs were kept in `/tmp/sg-s02/` and are not committed.

## Measured results

- `--plugin-dir` loads the plugin for the session only. The `init` stream event lists plugin `skill-graph-demo`, `skills: ["skill-graph-demo:recommender", ...]`, and `slash_commands: ["skill-graph-demo:recommender", ...]`.
- `SessionStart` (`matcher: startup`) fired and returned `hookSpecificOutput.additionalContext`; `stream-json --include-hook-events` exposes `hook_started` / `hook_response` with stdout, stderr, `exit_code`, and `outcome`.
- `UserPromptSubmit` fired with the submitted prompt and returned `additionalContext`.
- `UserPromptExpansion` fired for `/skill-graph-demo:recommender` with `expansion_type=slash_command`, `command_name=skill-graph-demo:recommender`, `command_args=""`, `command_source=plugin`.
- The plugin `bin/sg-recommend` executable ran through `${CLAUDE_PLUGIN_ROOT}` inside hooks and wrote logs/state under `/tmp`; no global PATH install was needed for that path.
- The demo hook implements per-session dedup markers for SessionStart and the first recommend-like prompt; tool hooks are log-only and emit no output.

## Blocker

Model turns fail before tool use: `claude auth status` reports `loggedIn:false`, `authMethod:none`, and the run returns `authentication_failed` / `Not logged in · Please run /login` with `apiKeySource:"none"`. Therefore Skill/Bash `PreToolUse`/`PostToolUse` observation, actual skill-content loading, and recommendation→invocation recall are **not yet measured**.

## Capability matrix

| Capability | Status | Evidence / note |
|---|---|---|
| Session-start recommendation injection | Verified hook path | `SessionStart` fired and `additionalContext` was returned and visible in `hook_response`. |
| Prompt-time recommendation injection | Verified hook path | `UserPromptSubmit` fired with `prompt` and returned `additionalContext`; recommendation quality untested. |
| Slash-command/skill expansion observation | Verified | `UserPromptExpansion` logged `command_name=skill-graph-demo:recommender`, `command_source=plugin`. |
| Skill-load observation via tool hooks | Unverified | Docs/community indicate `Skill` tool exists; `tool_input.skill` is reported but `PostToolUse` firing for `Skill` has conflicting reports. Needs authenticated rerun. |
| Mid-work event recommendation | Partially verified | `UserPromptSubmit` can inject context before Claude processes the prompt; tool-event recommendation not measured. |
| No-output when no recommendation | Implemented, partially observed | Tool hooks are log-only exit 0; prompt hook emits only for recommend-like prompts. |
| PATH-free plugin executable | Verified for hooks | `${CLAUDE_PLUGIN_ROOT}/bin/sg-recommend` ran without PATH install. |
| Session dedup | Implemented | State markers under `/tmp`; multi-prompt behavior untested due auth blocker. |
| Hook-less bootstrap | Partially verified | Plugin skill appears in `skills`/`slash_commands`; automatic description-based loading untested. |
| Existing-config non-pollution | Verified for experiment | Only `--plugin-dir` session load; temp cwd; logs/state under `/tmp`. |
| Codex / OpenCode | Not tested | Installed CLIs noted only; Claude Code was the priority target. |

## Next verification

After Claude Code authentication is available, rerun the same command with a prompt that asks Claude to invoke `skill-graph-demo:recommender` and run `echo sg-s02`, then inspect `/tmp/sg-s02/hooks.jsonl` for `PreToolUse`/`PostToolUse` `tool_name=Skill|Bash` and `tool_input.skill`.

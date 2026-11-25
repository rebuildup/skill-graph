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

## Live-provider blocker

Real model turns still require an authenticated external-provider route. `claude auth status` reports `loggedIn:false`, `authMethod:none`; the Agent Router free routes (`claude.kilo`, `claude.requesty`, `claude.modelscope`, `claude.openrouter-free`) are configured, but the Infisical user session is expired, so `script/with-secrets` cannot inject their credentials. No credential was copied or extracted. Recommendation→invocation recall under a real model therefore remains unmeasured.

## Mock-runtime verification (2026-11-25, post-C06)

A deterministic local Anthropic-compatible mock endpoint verified Claude Code hook mechanics without a model provider. This does not measure recommendation quality or invocation recall.

- `PostToolUse` matcher `Skill` fires. Payload includes `tool_name:"Skill"` and `tool_input.skill:"sg-s02-probe:alpha-skill"`; the hook normalized the namespaced skill to `alpha-skill` and emitted `additionalContext` for declared dependency `beta-skill`.
- `additionalContext` is injected into the next model request as a system message.
- No-recommendation prompts return empty hook output with `outcome=success`.
- A missing recommender binary returns empty hook output with `outcome=success` (`reason=recommender_error`).
- Two identical prompts in one `--input-format stream-json` session produced two `UserPromptSubmit` events; the second was suppressed by session dedup (`reason=dedup_skip`).
- C06 fixed a real adapter bug found here: quoted `${CLAUDE_PLUGIN_ROOT}` broke `posix_spawn` with ENOENT. See `docs/evidence/2026-11-25-c06-hook-command-quoting.md`.

## Capability matrix

| Capability | Status | Evidence / note |
|---|---|---|
| Session-start recommendation injection | Verified hook path | `SessionStart` fired and `additionalContext` was returned and visible in `hook_response`. |
| Prompt-time recommendation injection | Verified hook path | `UserPromptSubmit` fired with `prompt` and returned `additionalContext`; recommendation quality untested. |
| Slash-command/skill expansion observation | Verified | `UserPromptExpansion` logged `command_name=skill-graph-demo:recommender`, `command_source=plugin`. |
| Skill-load observation via tool hooks | Verified (mock runtime) | `PostToolUse:Skill` fired with `tool_input.skill`; namespaced plugin skill resolved and body injected. Real-model choice to load a recommended skill is still unmeasured. |
| Mid-work event recommendation | Verified hook path (mock); real-model drift untested | `PostToolUse` `additionalContext` reached the next model request; whether a real model acts on mid-work drift remains unmeasured. |
| No-output when no recommendation | Verified (mock runtime) | Unrelated prompt returned empty `hook_response.output`, `outcome=success`, `reason=no_recommendations`. |
| PATH-free plugin executable | Verified for hooks | `${CLAUDE_PLUGIN_ROOT}/bin/skill-graph-recommend-hook` ran without PATH install after C06 removed quoting. |
| Session dedup | Verified (mock runtime) | Same prompt twice in one stream-json session: first emitted, second `dedup_skip` with empty output. |
| Hook-less bootstrap | Partially verified | Plugin skill appears in `skills`/`slash_commands`; automatic description-based loading untested. |
| Existing-config non-pollution | Verified for experiment | Only `--plugin-dir` session load; temp cwd; logs/state under `/tmp`. |
| Codex / OpenCode | Not tested | Installed CLIs noted only; Claude Code was the priority target. |

## Next verification

Once an Agent Router external-provider route can obtain its Infisical credential (currently blocked by the expired user session), rerun with a fixed free route such as `agent claude kilo -- -p ...` or `agent claude requesty -- -p ...`, keeping model/provider/conditions fixed across Baseline/E/F. Verify Skill/Bash `PreToolUse`/`PostToolUse` payloads on the real provider, recommendation→actual-load conversion, mid-work task drift, unnecessary-load suppression, re-recommendation/context cost, timeliness, and artifact quality. The mock-runtime results above cover mechanics only.

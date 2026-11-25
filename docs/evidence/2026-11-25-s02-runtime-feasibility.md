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

## Live-provider blocker (resolved 2026-11-25)

~~Real model turns still require an authenticated external-provider route.~~ Resolved via the Kilo anonymous free tier — see "Live-provider verification" below. The Agent Router `claude.*` routes remain Infisical-blocked, but the same Kilo Gateway endpoint accepts anonymous Anthropic-protocol requests with `ANTHROPIC_AUTH_TOKEN=anonymous`.

## Mock-runtime verification (2026-11-25, post-C06)

A deterministic local Anthropic-compatible mock endpoint verified Claude Code hook mechanics without a model provider. This does not measure recommendation quality or invocation recall.

- `PostToolUse` matcher `Skill` fires. Payload includes `tool_name:"Skill"` and `tool_input.skill:"sg-s02-probe:alpha-skill"`; the hook normalized the namespaced skill to `alpha-skill` and emitted `additionalContext` for declared dependency `beta-skill`.
- `additionalContext` is injected into the next model request as a system message.
- No-recommendation prompts return empty hook output with `outcome=success`.
- A missing recommender binary returns empty hook output with `outcome=success` (`reason=recommender_error`).
- Two identical prompts in one `--input-format stream-json` session produced two `UserPromptSubmit` events; the second was suppressed by session dedup (`reason=dedup_skip`).
- C06 fixed a real adapter bug found here: quoted `${CLAUDE_PLUGIN_ROOT}` broke `posix_spawn` with ENOENT. See `docs/evidence/2026-11-25-c06-hook-command-quoting.md`.

## Live-provider verification (2026-11-25, Kilo anonymous free route)

The earlier blocker is resolved without OAuth or Infisical: the Kilo Gateway
anonymous free tier accepts Anthropic-protocol requests with a non-secret
placeholder token. Verified directly and through the real `claude` CLI
(2.1.329):

```bash
ANTHROPIC_BASE_URL=https://api.kilo.ai/api/gateway \
ANTHROPIC_AUTH_TOKEN=anonymous \
claude -p "<task>" \
  --model nvidia/nemotron-3-ultra-550b-a55b:free \
  --plugin-dir /home/basic/work/skill-graph/adapters/claude-code/skill-graph \
  --setting-sources project --no-session-persistence \
  --output-format stream-json --include-hook-events --verbose \
  --permission-mode bypassPermissions
```

- `AUTH_TOKEN=anonymous` (or empty/whitespace) is treated as the anonymous tier;
  any other dummy value is rejected 401. No credential is used or stored.
- Anonymous quota: ~200 requests/hour per IP, free models only. The full
  experiment consumed the window; `baseline-t5` hit HTTP 429 mid-run.
- `agent task run` cannot drive this: task dispatch supports the `omp` adapter
  only (`task.ts` rejects non-omp harnesses), and `claude.*` routes still need
  Infisical for authenticated keys. This env-var path bypasses the Agent
  Router launcher entirely — it is the same Kilo Gateway endpoint as
  `claude.kilo`, anonymous tier instead of account tier.

### Setup

- Corpus: `/tmp/sg-live/project*/.claude/skills` = the repo's 17 project-init
  skills (real `requires-skills` graph: quality-gate/design-refinement/
  engineering-decisions/policy-evaluation/skill-selection/correctness-assurance
  → system-quality → code-naming).
- `SKILL_GRAPH_BIN=target/release/skill-graph`,
  `SKILL_GRAPH_ROOTS=<proj>/.claude/skills` (home roots excluded for control).
- Arms: `plugin` = `--plugin-dir` adapter (group F: text+graph hooks);
  `baseline` = no plugin (group A: native description-based skill selection).
- Tasks: t1 impl (`slugify`), t2 diagnose (`cargo test`), t3 no-match
  (`17*23`), t4 drift (two-turn stream-json: naming question → Japanese README),
  t5 quality-gate design (dep-chain probe). One sample per cell; the model is
  nondeterministic, so counts are indicative, not statistically conclusive.

### Measured results

| Run | Skill loads | Prompt-hook recommendations | Notes |
|---|---|---|---|
| plugin t1 | none | interaction-discipline (noise) | task done, 8 API calls |
| plugin t2 | none | diagnosing-bugs, code-review, natural-japanese | fixed tests, 33 calls |
| plugin t3 | none | code-review (noise) | "391", 2 calls |
| plugin t4 | natural-japanese | turn1: natural-japanese (noise); turn2: natural-japanese, code-review | drift re-recommended on 2nd prompt; 14 calls |
| plugin t5 | quality-gate → system-quality | quality-gate, code-review, interaction-discipline | dep chain: PostToolUse recommended system-quality then code-naming; 22 calls |
| baseline t1 (fresh) | none | — | done, 11 calls |
| baseline t2 (fresh) | none | — | done, 27 calls |
| baseline t3 (fresh) | none | — | "391", 2 calls |
| baseline t4 (fresh) | code-naming, natural-japanese | — | native discovery loaded both relevant skills; 17 calls |
| baseline t5 | quality-gate only (partial) | — | HTTP 429 after 12 calls; no dep loads observed |

Hook latency on the real path: `recommend_ms` 7–14 ms, hook `elapsed_ms`
24–47 ms — inside the 250 ms budget. 9 hook events, 8 emissions; per-prompt
dedup correctly allowed the second drift prompt while suppressing repeats.

### Findings

1. **Dep-chain injection works on a real model.** t5 is the first observed
   graph-attributable load: prompt-mode recommended `quality-gate` (loaded),
   `PostToolUse:Skill` then recommended its declared dep `system-quality`
   (loaded), then `code-naming` (recommended, correctly NOT loaded — the task
   needed no naming work). Baseline loaded `quality-gate` natively but showed
   no dep loads before the 429. n=1, but the mechanism converts end-to-end.
2. **Prompt-mode recommendation quality on real prompts is poor.** 4 of 5
   prompt events produced noise driven by stopwords/punctuation (`->`, `it`,
   `the`, `keep`). The model suppressed every bad recommendation (0 bad loads
   observed), so harm is limited to `additionalContext` tokens and trust.
   Tracked as #46.
3. **Native skill discovery is strong on trivial tasks.** Baseline matched or
   beat the plugin arm on t1–t4 (e.g. t4 loaded code-naming + natural-japanese
   with no hooks). Hooks add measurable value only when declared dependencies
   or mid-work drift exceed description matching — consistent with ADR-0002's
   selective-admission intent, but the current text-match admission is too
   permissive (#46).
4. **Mid-work drift is covered.** The second stream-json prompt received a
   fresh recommendation (dedup is per prompt hash, not per session); t4 turn 2
   produced the relevant `natural-japanese` recommendation and load.
5. **Invocation Recall vs recommendation precision are distinct here.** Of 8
   emissions, ~2 were task-relevant; of recommended skills, loads converted
   only when the model judged them necessary (quality-gate, system-quality,
   natural-japanese loaded; code-naming, code-review, interaction-discipline
   not). This is the desired selectivity, but it means recall gains must be
   measured at the load event, not the emission event.

### Not yet verified

- baseline-t5 completion and artifact-quality comparison (429; resume below).
- Group E (text-search-only hook, no graph) live arm — can be approximated by
  pointing `SKILL_GRAPH_ROOTS` at a corpus copy with `requires-skills` stripped.
- Bash/PreToolUse payload shapes on the real provider (adapter hooks only
  UserPromptSubmit + PostToolUse:Skill; mock runtime already covered mechanics).
- Statistical power: one sample per cell on a nondeterministic model.

### Resume

After the Kilo anonymous window resets (~200 req/hour per IP), rerun only the
missing cells:

```bash
PROJ=/tmp/sg-live/project-t5-base OUT_SUFFIX=-fresh2 /tmp/sg-live/run.sh baseline t5-qualitygate
# (t5 needs the single-prompt variant; see /tmp/sg-live/run.sh or reuse the
#  stream-json input /tmp/sg-live/in-plugin-t5.jsonl without --plugin-dir)
```

Raw logs: `/tmp/sg-live/out/*.jsonl`, `/tmp/sg-live/logs/hook-*.jsonl`
(machine-local, not committed).

## Capability matrix

| Capability | Status | Evidence / note |
|---|---|---|
| Session-start recommendation injection | Verified hook path | `SessionStart` fired and `additionalContext` was returned and visible in `hook_response`. |
| Prompt-time recommendation injection | Verified (live) | `UserPromptSubmit` fired on every prompt; emissions convert to loads only when relevant — but 4/5 prompt recommendations were stopword noise (#46). |
| Slash-command/skill expansion observation | Verified | `UserPromptExpansion` logged `command_name=skill-graph-demo:recommender`, `command_source=plugin`. |
| Skill-load observation via tool hooks | Verified (mock + live) | `PostToolUse:Skill` fired with `tool_input.skill`; live t5 chain: quality-gate→system-quality loaded via dep injection, code-naming recommended but correctly not loaded. |
| Mid-work event recommendation | Verified (live, n=1 chain + drift) | t5 PostToolUse chain and t4 second-prompt re-recommendation both converted on the real model; broader drift coverage still thin. |
| No-output when no recommendation | Verified (mock + live) | t4 PostToolUse emitted `no_recommendations` empty output on the real path; unrelated-prompt case verified on mock. |
| PATH-free plugin executable | Verified for hooks | `${CLAUDE_PLUGIN_ROOT}/bin/skill-graph-recommend-hook` ran without PATH install after C06 removed quoting; same binary ran all live hook events. |
| Session dedup | Verified (mock + live) | Live t4: two distinct prompts each emitted once; identical-prompt suppression verified on mock. |
| Hook-less bootstrap | Verified (live) | Baseline arm with no plugin loaded `code-naming`+`natural-japanese` via native description matching (t4) and `quality-gate` (t5 partial). |
| Existing-config non-pollution | Verified for experiment | Only `--plugin-dir` session load; temp cwd; logs/state under `/tmp`. |
| Codex / OpenCode | Not tested | Installed CLIs noted only; Claude Code was the priority target. |

## Next verification

Live-provider verification ran on the Kilo anonymous free route (see above).
Remaining before S02 can close:

1. Finish `baseline-t5` after the anonymous quota window resets (resume
   commands in the live section) for a complete Baseline-vs-F artifact
   comparison on the dep-chain task.
2. Optionally run the group-E approximation (graph-stripped corpus) to
   isolate the graph's contribution from text search alone.
3. Decide whether #46 (stopword admission) blocks release or lands as a
   fast-follow; re-run t3/t1 prompt cells after any fix.
4. Reflect final results into spec/ADR and the R01 record.

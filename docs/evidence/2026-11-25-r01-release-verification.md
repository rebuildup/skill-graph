# Evidence — R01 release-0-1-0 candidate verification (2026-11-25)

Issue: #33
Branch: `33` → `release-0-1-0`

Auth-independent release verification for the `release-0-1-0` candidate. Updated 2026-11-25 after S02 live-runtime verification on the Kilo anonymous free route — see "Resume procedure" for the remaining cells.

## Checklist

| # | Invariant / requirement | Status | Evidence |
|---|---|---|---|
| 1 | Agent Skills parse without modification | verified | `tests/core.rs`, `tests/security.rs` (symlink/traversal/external/parse-error cases); real `.claude/skills` + `.agents/skills` corpus parsed (34 skills, 0 errors) |
| 2 | Agent Plugins v1.0.0 package conformance | verified | `plugins/skill-graph/plugin.json` structurally validated against official 1.0.0 schema (`docs/evidence/2026-11-25-p01-cli-plugin-distribution.md`) |
| 3 | CLI contract: discover/recommend/resolve/bench/perf | verified | release build smoke on fixtures + real skills (`p01` evidence); `contracts/v1/*.schema.json` + `tests/contract.rs` shape tests |
| 4 | Recommendation output contract (reasons, observation, empty→`[]`) | verified | `tests/contract.rs::recommendation_output_matches_contract_shape_and_no_output` |
| 5 | Resolve ambiguity reported, not uniquified | verified | `tests/core.rs::ambiguous_names_are_reported_not_uniquified`, `resolve_result_contract_covers_name_path_unresolved_and_ambiguity` |
| 6 | PATH-free plugin execution | verified | `${CLAUDE_PLUGIN_ROOT}/bin/...` hooks ran without PATH install (S02/X03/P02 evidence); adapter supports `SKILL_GRAPH_BIN` absolute path |
| 7 | Non-pollution: no writes to project/settings/skills/logs | verified | `src/` contains no filesystem writes outside `perf.rs` temp-corpus generator (which cleans up); hook state/log confined to `${TMPDIR}`; adapter is opt-in `--plugin-dir` only |
| 8 | No-save mode | verified | Core CLI is fully stateless (memory-only). Adapter: `SKILL_GRAPH_HOOK_NOSTATE=1` disables markers and log writes (X04 script-level test: no state dir, no log file) |
| 9 | Silent no-output when no recommendation | verified | `recommend` returns `[]`; hook exits 0 silently on `no_recommendations`/`no_input_or_roots` (P02/X04 logs) |
| 10 | Fail-open on internal error | verified | `SKILL_GRAPH_BIN=/nonexistent` → `recommender_error`, exit 0, no output (P02 evidence) |
| 11 | Duplicate multi-root installs | verified | C04 (#35): name-dedup recommend + name-based requires resolution; regression test + real corpus check |
| 12 | Mid-work dependency surfacing | verified (live, n=1) | X04 (#34) + C06 (#43) + S02 live: `PostToolUse/Skill` emitted declared deps on the real model; t5 chain quality-gate→system-quality loaded via injection, code-naming recommended but correctly not loaded. Drift: second stream-json prompt re-recommended (t4). Broader coverage thin — see S02 evidence |
| 12b | Same-name skill identification contract | verified | C05 (#39): recommend evaluates all copies, groups by name, flags distinct-digest collisions in reasons; identical duplicates dedupe silently; resolve still reports ambiguity (`docs/evidence/2026-11-25-c05-identification-contract.md`) |
| 13 | Recommendation quality (dataset) | verified | 36-case eval: required_recall 1.0, forbidden_hits 0 (`evals/experiments/x02|x03/report.json`, `bench`) |
| 14 | Performance targets (ADR-0002) | verified | recommend warm p95 ~0.7 ms @500 skills, CLI process p95 ~13 ms (release), hook end-to-end ~29–42 ms — all within p95<100 ms / hook<250 ms / max 3 (`c03`, `x03`, `p02` evidence) |
| 15 | Invocation recall / timeliness / artifact quality | partially verified | S02 live (Kilo anonymous, fixed model/route): dep-chain load conversion observed (system-quality via `requires` injection); prompt-mode emissions were mostly stopword noise (#46) but the model suppressed all bad loads; hook latency 24–47 ms. Baseline-vs-F comparison incomplete — baseline-t5 hit the 200 req/hour anonymous quota (429); artifact-quality comparison pending. Recommendation metrics above must not be read as invocation evidence |
| 16 | release→main merge, external publish, permanent install | not attempted | Explicit approval boundary — unchanged |

## Distinctions (per operating rules)

- **Implementation complete / integrated**: items 1–14 above (code + evidence merged to `release-0-1-0`).
- **Recommendation performance**: dataset metrics only; fixture-based, deterministic.
- **Actual invocation performance**: partially measured — S02 live run on Kilo anonymous free route verified dep-chain load conversion and model-level suppression of noisy recommendations; full Baseline/E/F comparison incomplete (anonymous quota exhausted; see Resume procedure).
- **Release verification**: this document; CI green on all merged PRs (#21–#37 range).

## Known residual gaps

- `PostToolUse` payload field names for other tools (`Bash` etc.) — adapter hooks only `Skill`; `tool_input.skill` verified live.
- Re-recommendation frequency across longer real sessions (one drift session observed).
- Real-corpus latency on large skill sets beyond the 34-skill duplicated corpus (C03 synthetic 500-skill data suggests large headroom).
- Codex/OpenCode adapters: not implemented (no measured need; OpenCode has a stored credential but Claude Code is the validated target).

## Resume procedure

The OAuth/Infisical blocker is resolved for verification purposes: the Kilo Gateway anonymous free tier accepts Anthropic-protocol requests with the non-secret placeholder `ANTHROPIC_AUTH_TOKEN=anonymous` (any other dummy value → 401). No credential is used. Quota is ~200 requests/hour per IP; the 2026-11-25 experiment exhausted the window (`baseline-t5` → HTTP 429). The Agent Router `claude.*` authenticated routes remain Infisical-blocked, and `agent task run` supports the `omp` adapter only — the env-var path below is the working route. Full commands and raw-log locations: `docs/evidence/2026-11-25-s02-runtime-feasibility.md` "Live-provider verification".

```bash
cd /tmp/sg-live/project-t5-base   # fresh corpus copy; see S02 evidence for setup
ANTHROPIC_BASE_URL=https://api.kilo.ai/api/gateway ANTHROPIC_AUTH_TOKEN=anonymous \
ANTHROPIC_SMALL_FAST_MODEL=nvidia/nemotron-3-ultra-550b-a55b:free \
CLAUDE_CODE_SUBAGENT_MODEL=nvidia/nemotron-3-ultra-550b-a55b:free \
env -u CLAUDE_CODE_OAUTH_TOKEN claude -p --input-format stream-json \
  --model nvidia/nemotron-3-ultra-550b-a55b:free \
  --setting-sources project --no-session-persistence \
  --output-format stream-json --include-hook-events --verbose \
  --permission-mode bypassPermissions \
  < /tmp/sg-live/in-plugin-t5.jsonl > /tmp/sg-live/out/baseline-t5-fresh2.jsonl
# add --plugin-dir /home/basic/work/skill-graph/adapters/claude-code/skill-graph
# plus SKILL_GRAPH_* env for the plugin arm (see /tmp/sg-live/run.sh)
```

Pending live items: `baseline-t5` completion (Baseline-vs-F artifact comparison on the dep-chain task), optional group-E approximation (graph-stripped corpus), Bash `PreToolUse` payload shape (adapter does not hook Bash; mock covered PostToolUse mechanics), and re-run of prompt cells after #46 lands. X04's unconditional-dependency concern resolved positively in t5: `code-naming` was recommended via the dep chain but the model correctly did not load it — the hook emits names only, never forces loads.

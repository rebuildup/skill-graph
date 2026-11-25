# Evidence — R01 release-0-1-0 candidate verification (2026-11-25)

Issue: #33
Branch: `33` → `release-0-1-0`

Auth-independent release verification for the `release-0-1-0` candidate. Updated at head `feed31d` (post-C05, PR #40) — see "Resume procedure" for the pending live-runtime items.

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
| 12 | Mid-work dependency surfacing | verified (mock runtime); live-model pending | X04 (#34) + C06 (#43): `PostToolUse/Skill` hook emits declared `requires` deps; mock Claude Code run confirmed payload, `additionalContext` injection, no-output, fail-open, and session dedup (`docs/evidence/2026-11-25-c06-hook-command-quoting.md`, S02 mock-runtime section). Real-model drift response remains unmeasured |
| 12b | Same-name skill identification contract | verified | C05 (#39): recommend evaluates all copies, groups by name, flags distinct-digest collisions in reasons; identical duplicates dedupe silently; resolve still reports ambiguity (`docs/evidence/2026-11-25-c05-identification-contract.md`) |
| 13 | Recommendation quality (dataset) | verified | 36-case eval: required_recall 1.0, forbidden_hits 0 (`evals/experiments/x02|x03/report.json`, `bench`) |
| 14 | Performance targets (ADR-0002) | verified | recommend warm p95 ~0.7 ms @500 skills, CLI process p95 ~13 ms (release), hook end-to-end ~29–42 ms — all within p95<100 ms / hook<250 ms / max 3 (`c03`, `x03`, `p02` evidence) |
| 15 | Invocation recall / timeliness / artifact quality | **blocked** | Requires an authenticated external-provider route. Mock runtime verified hook mechanics only; Baseline/E/F live comparison and recommendation→actual-load conversion remain unmeasured. Recommendation metrics above must not be read as invocation evidence |
| 16 | release→main merge, external publish, permanent install | not attempted | Explicit approval boundary — unchanged |

## Distinctions (per operating rules)

- **Implementation complete / integrated**: items 1–14 above (code + evidence merged to `release-0-1-0`).
- **Recommendation performance**: dataset metrics only; fixture-based, deterministic.
- **Actual invocation performance**: unmeasured — blocked on Claude Code auth (Infisical session expired; `claude` not logged in; provider routes need interactive login).
- **Release verification**: this document; CI green on all merged PRs (#21–#37 range).

## Known residual gaps

- Live `PostToolUse` payload field names for `Skill` loads (`tool_input.skill` assumed with fallbacks).
- Multi-prompt dedup inside one live session (script-level only).
- Real-corpus latency on large skill sets beyond the 34-skill duplicated corpus (C03 synthetic 500-skill data suggests large headroom).
- Codex/OpenCode adapters: not implemented (no measured need; OpenCode has a stored credential but Claude Code is the validated target).

## Resume procedure (post-auth)

Blocked on: external-provider credentials. As of 2026-11-25, `claude auth status` → `loggedIn:false` (OAuth expired, no subscription); Agent Router free routes exist (`agent routes`), but the Infisical user session is expired so `script/with-secrets` cannot inject `KILO_API_KEY`/`REQUESTY_API_KEY`/`MODELSCOPE_API_KEY`/`OPENROUTER_API_KEY`. No credential copying/extraction was performed. Hook mechanics were verified with a deterministic local mock endpoint instead (C06 evidence); real-model verification still needs a provider route.

Once either path is available, run the S02 verification (`docs/evidence/2026-11-25-s02-runtime-feasibility.md` "Next verification"):

```bash
# Option A — native Claude Code login, then:
cd /tmp/sg-s02/project && env SKILL_GRAPH_BIN=/home/basic/work/skill-graph/target/release/skill-graph \
  SKILL_GRAPH_ROOTS=$HOME/.claude/skills:$HOME/.agents/skills \
  SKILL_GRAPH_HOOK_LOG=/tmp/sg-s02/hooks.jsonl SKILL_GRAPH_HOOK_STATE=/tmp/sg-s02/state \
  claude -p "<task prompting skill use>" \
  --plugin-dir /home/basic/work/skill-graph/adapters/claude-code/skill-graph \
  --setting-sources project --no-session-persistence \
  --output-format stream-json --include-hook-events --verbose \
  --permission-mode bypassPermissions

# Option B — Agent Router external provider (requires `infisical login` first;
# pick ONE free route and keep it fixed across Baseline/E/F):
agent claude kilo -- -p "<task>" --plugin-dir /home/basic/work/skill-graph/adapters/claude-code/skill-graph \
  --setting-sources project --no-session-persistence \
  --output-format stream-json --include-hook-events --verbose \
  --permission-mode bypassPermissions
# alternatives: agent claude requesty ... / agent claude modelscope ... / agent claude openrouter-free ...
```

Pending live items: `PreToolUse`/`PostToolUse` payload shape for `Bash` on a real provider, recommendation→actual-load conversion (Invocation Recall), Baseline/E/F live comparison, mid-work task-drift response under a real model, re-recommendation/context cost, timeliness, and artifact-quality effect. `PostToolUse` firing for `Skill` and `tool_input.skill` are already verified by the mock runtime (C06 evidence). X04's unconditional-dependency concern (declared `requires` vs actual need) is evaluated there — the hook emits names only, never forces loads.

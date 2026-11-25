# Evidence — R01 release-0-1-0 candidate verification (2026-11-25)

Issue: #33
Branch: `33` → `release-0-1-0`

Auth-independent release verification for the current `release-0-1-0` head (`c7e36b5` + this doc). Items requiring a live authenticated Claude Code session are marked **blocked**, not passed.

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
| 12 | Mid-work dependency surfacing | implemented, live-pending | X04 (#34): `PostToolUse/Skill` hook emits declared `requires` deps; script-level verified on real corpus. Real payload shape & `PostToolUse`-for-`Skill` firing **blocked** on auth |
| 13 | Recommendation quality (dataset) | verified | 36-case eval: required_recall 1.0, forbidden_hits 0 (`evals/experiments/x02|x03/report.json`, `bench`) |
| 14 | Performance targets (ADR-0002) | verified | recommend warm p95 ~0.7 ms @500 skills, CLI process p95 ~13 ms (release), hook end-to-end ~29–42 ms — all within p95<100 ms / hook<250 ms / max 3 (`c03`, `x03`, `p02` evidence) |
| 15 | Invocation recall / timeliness / artifact quality | **blocked** | Requires authenticated Claude Code session; S02 (#4) remains open. Recommendation metrics above must not be read as invocation evidence |
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

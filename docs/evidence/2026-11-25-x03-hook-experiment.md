# Evidence — X03 minimal hook experiment, groups E/F (2026-11-25)

Issue: #12
Branch: `12` → `release-0-1-0`

## Scope

Minimal hook recommendation experiment comparing group E (text search only) and group F (text search + reference graph) for a single runtime (Claude Code). Two layers were measured:

1. Recommendation quality: deterministic E-vs-F comparison over the E02 36-case dataset.
2. Hook feasibility/timing: an isolated `UserPromptSubmit` hook that calls the recommender, dedups per session+prompt, emits `additionalContext` only on hits, and logs latency.

Live model turns remain auth-blocked (see S02 evidence); hook events fire before the auth failure, so hook path, output shape, dedup, no-output, and timing are measured, but recommendation→invocation recall is not.

## Harness

- Eval: `src/x03.rs` + `src/bin/x03.rs` — `cargo run --bin x03 evals/dataset/cases.jsonl`. Report: `evals/experiments/x03/report.json`.
- Hook-facing binary: `src/bin/x03-hook.rs` → `x03::recommend_names(root, query, context, max, text_only)`. Does cold discovery+build+recommend per call, matching a per-event hook process.
- Plugin: `experiments/claude-code/plugin-skill-graph-x03/` — `UserPromptSubmit` hook only, `${CLAUDE_PLUGIN_ROOT}/bin/sg-x03-recommend`, state/log under `$SG_X03_STATE`/`$SG_X03_LOG` (tmp only, no project writes).

## Arms

- `E_text_search`: `recommend` on a graph copy with all links and backlinks stripped — text search over name/description/body only.
- `F_text_graph`: `recommend` on the full resolved graph — adds `requires` admission (one hop), Markdown/mention weak evidence, backlink reasons.
- Both use `max=3`, the candidate hook recommendation cap.

## Eval summary (36 cases)

| arm | required_recall | forbidden_hits | optional_hits | recommended_total | precision_required | precision_acceptable | empty | recommend avg/p95/max (µs) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| E_text_search | 1.0 | 0 | 5/21 | 43 | 0.767 | 0.884 | 3 | 2373 / 4664 / 5240 |
| F_text_graph | 1.0 | 0 | 14/21 | 53 | 0.623 | 0.887 | 3 | 2286 / 4193 / 5810 |

## Hook measurements

Direct script runs (`fixtures/skills` root, debug binary):

- Hit: `release notes changelog` → emitted `additionalContext: "skill-graph recommend (graph): gamma-skill"`, `recommend_ms=10`, `elapsed_ms=38`.
- Dedup: identical session_id+prompt replay → `emitted=false, reason=dedup_skip`, no stdout, `elapsed_ms=14`.
- No-match: gibberish prompt → `emitted=false, reason=no_recommendations`, no stdout.
- Text mode: `rust cli implementation tests` → `alpha-skill` only (graph mode adds `beta-skill` via `requires`), `elapsed_ms=34`.

Real Claude Code run (`claude -p`, `--plugin-dir`, `--include-hook-events`, auth-blocked):

- `hook_started`/`hook_response` for `UserPromptSubmit` observed; `hook_response.output` contained `additionalContext: "skill-graph recommend (graph): gamma-skill"`.
- Hook log: `emitted=true, recommend_ms=11, elapsed_ms=42` — end-to-end hook latency ~42 ms on a 7-skill fixture, far under the 250 ms candidate budget. Process spawn + cold discovery + recommend included.

## Interpretation

- **Graph adds dependency coverage, not forbidden-hit reduction on this dataset.** Both arms keep required_recall 1.0 and forbidden_hits 0. F recommends 10 more skills total; the delta is dominated by `requires`-admitted dependencies (e.g. `beta-skill` via `alpha-skill`), raising optional hits 5→14. `precision_acceptable` is effectively unchanged (0.884 vs 0.887); `precision_required` drops (0.767→0.623) because dependency admissions count as recommended items.
- E (text-only) is a viable minimal hook: fewer recommendations, same recall/forbidden on this dataset. F is justified where declared dependencies should surface (the core "agent doesn't know it needs the dependency" problem), at the cost of ~23% more recommended items.
- Hook path is feasible: correct event, correct `additionalContext` shape, dedup, silent no-output, and ~40 ms latency including process spawn on a small skill set. Latency at 50–500 skills is covered by C03 benchmarks, not this fixture.

## Limits / next

- No live invocation recall or timeliness: model turns fail at auth; rerun after Claude Code login (shared blocker with S02/#4).
- Dedup verified at script level and for a single hook fire per session; multi-prompt dedup inside one live session is untested.
- E-vs-F is an admission-surface comparison (links stripped vs resolved); body-text name mentions remain in both arms.
- E03 should weigh F's dependency recall against its extra recommendations when fixing the MVP recommendation policy.

# Evidence — X02 bootstrap / existing-skill reference experiment (2026-11-25)

Issue: #11
Branch: `11` → `release-0-1-0`

## Scope

Group C/D proxy evaluation over the E02 36-case dataset. This is a deterministic simulation of interventions, not an agent-runtime experiment; Claude Code auth is still unavailable for live invocation recall (see S02 evidence).

Harness: `src/x02.rs` + `src/bin/x02.rs` (`cargo run --bin x02 evals/dataset/cases.jsonl`). Report: `evals/experiments/x02/report.json`.

## Arms

- `baseline`: current `recommend(query, context, max=5)`.
- `compact_start`: session-start compact proxy = `recommend("", context, max=3)` using only known context.
- `requires_from_baseline`: baseline recommendations unioned with unique resolved `requires` targets of recommended skills.
- `reference_from_baseline`: baseline unioned with resolved `requires` + Markdown link targets of recommended skills.
- `compact_plus_reference`: compact_start unioned with resolved `requires` + Markdown targets of compact recommendations.

## Summary

| arm | required_recall | forbidden_hits | optional_hits | recommended_total | precision_required | precision_acceptable | empty_recommendations | reference_added |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| baseline | 1.0 | 0 | 14/21 | 54 | 0.611 | 0.870 | 3 | 0 |
| compact_start | 0.758 | 0 | 10/21 | 41 | 0.610 | 0.854 | 11 | 0 |
| requires_from_baseline | 1.0 | 1 | 14/21 | 61 | 0.541 | 0.770 | 3 | 7 |
| reference_from_baseline | 1.0 | 5 | 15/21 | 66 | 0.500 | 0.727 | 3 | 12 |
| compact_plus_reference | 0.758 | 2 | 10/21 | 49 | 0.510 | 0.714 | 11 | 8 |

## Interpretation

- **Group C compact start is not sufficient alone**: context-only top-3 drops required recall to 0.758 and increases empty recommendations from 3 to 11. A start-time compact list needs the actual task signal or must be treated as a hint, not the only recommendation path.
- **Group D reference resolution must stay selective**: unioning explicit `requires` targets preserves recall but adds forbidden/precision cost in this simulation; unioning Markdown links as well is worse (forbidden 5). This supports keeping explicit dependency admission separate from related-link evidence, matching the C02 admission rule.
- The production recommender already admits `requires` dependencies of admitted skills with a necessity boost rather than unioning all references; this experiment suggests not extending that to Markdown links without stronger evidence.

## Limits / next

- Reference arms are post-hoc unions for measurement; they are not the shipped admission rule.
- No live invocation recall or timeliness; requires authenticated Claude Code rerun (S02/X03).
- `compact_start` uses `context` only; real bootstrap prompts may include repo/task metadata not present in dataset cases.

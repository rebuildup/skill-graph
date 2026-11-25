# Evidence — E03 comparison and MVP decision (2026-11-25)

Issue: #13
Branch: `13` → `release-0-1-0`

## Integrated results

| Experiment | Question | Key result |
|---|---|---|
| E02 (#2) | Dataset + baseline | 36 cases, labels independent of observed invocations; baseline required_recall 1.0, forbidden_hits 1 (pre-C02), now 0 after admission rules. |
| C01 (#7) | Memory core correctness | Discovery/parser/resolver verified in-memory; digest stability and ambiguous-name reporting tested. |
| C02 (#8) | Admission policy | Weak evidence (body/links/mentions/backlinks) cannot admit; `requires` admits one hop with necessity boost. forbidden_hits resolved without score tuning. |
| C03 (#9) | Performance | 500 skills: cold ~7.7 ms, CLI process p95 ~13 ms (release), warm recommend p95 ~0.7 ms. No cache needed for MVP range. |
| S02 (#4) | Runtime feasibility | Claude Code hooks/`--plugin-dir`/PATH-free exec verified; Skill/Bash tool observation and invocation recall auth-blocked (issue stays open). |
| S03 (#5) / S04 (#6) | Contracts / security | v1 JSON schemas + resolve-result contract; symlink/traversal/external-link/parse-error boundaries tested. |
| X01 (#10) | Group B description-only | No measurable metric difference on paired subset; descriptions already scored directly. |
| X02 (#11) | Groups C/D bootstrap+reference | Compact-start alone: recall 0.758, empty 11. Naive reference union: forbidden 1–5, precision loss. Selective admission validated. |
| X03 (#12) | Groups E/F hook recommendation | E vs F: recall 1.0 / forbidden 0 both; F optional hits 5→14, recommended 43→53, precision_acceptable ~equal. Hook end-to-end ~42 ms (debug fixture), dedup/no-output verified. |

## Decision

Recorded in `docs/adr/ADR-0002-mvp-recommendation-approach.md`:

- Adopt group F recommendation policy (current admission rules) for MVP.
- First intervention path: `UserPromptSubmit` hook injecting top-3 via `additionalContext`; silent when empty or duplicate.
- Fix targets: recommend p95 < 100 ms, hook total < 250 ms, max 3 recommendations per injection — all measured within budget.
- Defer O01/O02/I01 (no evidence of need); register P01 (#28) and P02 (#29) as next issues.

## Residual risks

- Live invocation recall, timeliness, and artifact-quality effect are unmeasured (Claude Code auth blocker, S02/#4).
- Dataset is fixture-based (7 skills); real-corpus recall/latency may differ — re-measure during P01/P02.
- X01/X02/X03 arms are deterministic simulations of interventions, not live agent runs; effect on real agent behavior is inferred, not observed.

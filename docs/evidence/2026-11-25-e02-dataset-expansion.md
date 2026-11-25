# Evidence — E02 evaluation dataset and baseline (2026-11-25)

Issue: #2
Branch: `2` → `release-0-1-0`

## What changed

- Added `evals/dataset/schema.md`: JSONL field contract, label rules (`required` / `optional` / `forbidden`), timing and case-kind metadata, privacy rules, and coverage requirements. Skill identity is `(skill_root, name)`; the same name under another root is a different skill.
- Expanded `evals/dataset/cases.jsonl` from 13 to 36 cases. The set now covers fixture skills for controlled ranking regressions and the installed project skills under `skills/` for realistic descriptions and `requires` edges.
- Added optional case metadata (`timing`, `rationale`, `case_kind`) and surfaced it in per-case bench output. The harness still scores only `expected_required` / `expected_optional` / `forbidden`.
- Added cross-lingual cases: Japanese task text with English context keywords, and Japanese context matched to Japanese skill descriptions.
- Updated `zeta-skill` description to use the exact terms `incident recovery`, `rollback`, and `production failure debugging` so the incident-recovery labels are stable under stricter admission rules.

## Coverage

- Domains: Rust implementation, tests/verification, release notes, documentation/prose, UI/frontend, incident recovery, parallel worktrees, agent/session recovery, code naming, PR review, correctness invariants, design refinement, bug diagnosis, GitHub delivery, user-facing interaction, Japanese prose, onboarding, policy evaluation, quality gates, security maintenance, skill selection, system quality, and writing discipline.
- Outcomes: required hits, optional `requires` dependencies, forbidden false-positive candidates, and empty-recommendation negatives.
- Timing labels: `session_start` and `mid_task` are recorded for later Invocation Recall / timeliness analysis; the current harness does not score them.

## Baseline (`cargo run --quiet -- bench evals/dataset/cases.jsonl`)

```json
"summary": {
  "cases": 36,
  "required_recall": 1.0,
  "forbidden_hits": 6,
  "optional_hits": 15,
  "optional_total": 19,
  "empty_recommendations": 3,
  "elapsed_ms": 145
}
```

`required_recall` is 1.0, but `forbidden_hits` is 6. The false positives are concentrated in cases where a broad project-skill description shares generic terms with the task:

- `code-naming-review`: `code-review`, `writing-discipline`
- `pr-review`: `writing-discipline`
- `quality-gate`: `correctness-assurance`
- `system-quality`: `design-refinement`
- `writing-discipline`: `code-review`

This is the baseline C02 must improve without lowering required recall.

## Verification

```bash
mise run check
mise run bench
```

`mise run check` passed: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` (3 passed).

## Remaining work

- C02: add precision / recall@K metrics and tighten admission/ranking so the six baseline forbidden hits are removed structurally rather than by score tuning.
- S02/X03: connect `timing` labels to observed load events for Invocation Recall and timeliness.
- Same-name / multi-origin cases can be added once cases need to distinguish identical names across roots or revisions.

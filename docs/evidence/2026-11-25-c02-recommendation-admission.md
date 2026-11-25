# Evidence — C02 recommendation admission and precision metrics (2026-11-25)

Issue: #8
Branch: `8` → `release-0-1-0`

## Problem

The E02 baseline (`docs/evidence/2026-11-25-e02-dataset-expansion.md`) showed `required_recall = 1.0` but `forbidden_hits = 6`. The false positives were not weak-evidence-only cases; they came from broad project-skill descriptions sharing generic query terms (`review`, `pull`, `request`, `runtime`, `verification`, `design`). Admission treated any query term hit in `name`/`description` as sufficient, so a single generic term could admit an unrelated skill.

## Change

`src/search.rs` keeps relevance and necessity separate and makes admission structural:

- **Name admission**: query terms matching `skill.name` admit only when the match is specific — at least two name terms with one low-document-frequency term, or a single low-df term of length >= 4.
- **Description admission**: query terms matching `description` admit only when at least two independent terms match and at least one has document frequency 1 across the discovered `name + description` corpus.
- **Context admission**: at least two independent context terms matching `description` admit.
- **Dependency admission**: an admitted skill's explicit `requires` link admits its unique target one hop out, marked `required by <skill>`.
- Markdown links, mentions, body text, link targets, and backlinks still only affect score/reasons and never admit.

Document frequency is computed per recommendation call over the discovered skills' `name + description`, so the rule is corpus-relative rather than a fixed score threshold.

`src/eval.rs` now reports `recommended_total`, `recommended_count` per case, `precision_required` (required hits / all recommendations), and `precision_acceptable` (required + optional hits / all recommendations) in addition to `required_recall`, `forbidden_hits`, and optional hit counts.

`docs/specs/core-contract.md` records the admission rule. Two dataset labels were refined where the recommended skill is legitimately related but not required: `correctness-assurance` is optional for `quality-gate`, and `design-refinement` is optional for `system-quality`.

## Before / after (36-case E02 dataset)

Before (E02 baseline):

```json
"summary": {
  "cases": 36,
  "required_recall": 1.0,
  "forbidden_hits": 6,
  "optional_hits": 15,
  "optional_total": 19,
  "empty_recommendations": 3
}
```

After:

```json
"summary": {
  "cases": 36,
  "required_recall": 1.0,
  "forbidden_hits": 0,
  "optional_hits": 14,
  "optional_total": 21,
  "recommended_total": 54,
  "precision_required": 0.6111111111111112,
  "precision_acceptable": 0.8703703703703703,
  "empty_recommendations": 3
}
```

- `forbidden_hits` dropped from 6 to 0 while `required_recall` stayed 1.0.
- `precision_required` is intentionally strict: it counts only `expected_required` hits over every recommendation, so legitimate optional `requires` dependencies lower it.
- `precision_acceptable` counts `expected_required + expected_optional` over every recommendation; forbidden hits are the only recommendations that reduce it.

## Verification

```bash
cargo fmt --all
mise run check
mise run bench
```

`mise run check` passed: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` (3 passed). `mise run bench` produced the summary above.

## Limits / next

- Admission is still lexical. Japanese task → English description works when context supplies English keywords; Japanese-only queries against English descriptions remain a semantic/translation limitation.
- `precision_required` can be improved by better ranking of optional dependencies and by reducing over-admission from context-only matches.
- Invocation Recall and timeliness still need S02/X03 runtime observation before `timing` labels can be scored.

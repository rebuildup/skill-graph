# Evidence — E02 dataset expansion (partial) (2026-11-25)

Issue: #2 (partial; full 30–50 real-task-derived set remains open)
Branch: `2` → `release-0-1-0`

## What changed

- Added fixture skills `delta-skill` (documentation/prose), `epsilon-skill` (frontend UI), `zeta-skill` (incident recovery), `eta-skill` (parallel worktrees) to broaden coverage beyond Rust/tests/release-notes.
- Expanded `evals/dataset/cases.jsonl` from 7 to 13 cases: positive cases per domain, a `requires` necessity case (`rust-dependency`), unused/forbidden tightening, and two no-recommendation cases (`schema-migration-none`, `none`).
- Tightened admission after `ui-component` exposed substring false positives (`change` matching `changes`/`changelogs`): admission now requires query hits in name/description **or** at least two independent context-term hits in description. This is an evidence-count rule, not a score threshold.

## Bench result

```json
"summary": {
  "cases": 13,
  "required_recall": 1.0,
  "forbidden_hits": 0,
  "optional_hits": 3,
  "optional_total": 4,
  "empty_recommendations": 2
}
```

All positive cases recommend the required skill first; unrelated skills are not recommended.

## Verification

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --quiet -- bench evals/dataset/cases.jsonl
```

## Remaining E02 work

- Reach 30–50 cases with real-task-derived contexts (without committing conversation text).
- Add Japanese task → English description cases; current lexical matcher cannot satisfy them and needs semantic/translation support or explicit labels.
- Add same-name / multi-origin cases once the case schema carries skill identity (path/digest) rather than name only.
- Define desired load timing and Invocation Recall once S02 hook observability lands.

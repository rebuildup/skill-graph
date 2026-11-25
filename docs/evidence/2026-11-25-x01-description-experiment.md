# Evidence — X01 description-only improvement experiment (2026-11-25)

Issue: #10
Branch: `10` → `release-0-1-0`

## Design

Group B isolates `description` as the only changed surface. `evals/variants/x01/` contains two arms over the same 15 fixture-root cases from `evals/dataset/cases.jsonl`:

- `baseline/fixtures/skills`: unchanged copy of `fixtures/skills`.
- `improved/fixtures/skills`: same names, bodies, links, and `requires-skills`; only frontmatter `description` was rewritten with broader applicability synonyms.
- `baseline/cases.jsonl` and `improved/cases.jsonl` are identical; each resolves `skill_root:"fixtures/skills"` to its own arm directory.

Reports: `baseline.report.json`, `improved.report.json`; paired diff: `comparison.md`.

## Commands

```bash
cargo run --quiet -- bench evals/variants/x01/baseline/cases.jsonl > evals/variants/x01/baseline.report.json
cargo run --quiet -- bench evals/variants/x01/improved/cases.jsonl > evals/variants/x01/improved.report.json
```

## Result

On the 15-case fixture subset, the description-only variant produced **no metric or per-case recommendation differences**:

| metric | baseline | improved | delta |
|---|---:|---:|---:|
| required_recall | 1.0 | 1.0 | 0.0 |
| forbidden_hits | 0 | 0 | 0 |
| optional_hits | 3 | 3 | 0 |
| recommended_total | 15 | 15 | 0 |
| precision_required | 0.8 | 0.8 | 0.0 |
| precision_acceptable | 1.0 | 1.0 | 0.0 |
| empty_recommendations | 3 | 3 | 0 |

## Interpretation

- The current admission rule is already strict enough that synonym-rich descriptions did not admit new skills on this subset.
- This does **not** show description improvement is useless; it shows this particular synonym expansion was neutral under lexical admission. Larger gains may require description changes that add exact query terms for currently missed required skills, or may need semantic matching beyond lexical overlap.
- The experiment covered fixture skills only. Project-skill descriptions (`skill_root:"skills"`) were not variant-tested here.

## Limits / next

- `bench` reports recommended names/counts, not scores; ranking changes that do not alter the top set are invisible.
- Invocation recall still requires an authenticated runtime experiment (S02/X03).

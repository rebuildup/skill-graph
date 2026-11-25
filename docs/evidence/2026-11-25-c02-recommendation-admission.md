# Evidence — C02 recommendation admission fix (2026-11-25)

Issue: #8 (partial; dataset expansion toward E02 is still open)
Branch: `8` → `release-0-1-0`

## Problem

`bench evals/dataset/cases.jsonl` reported `forbidden_hits = 1`: the `release-notes` case recommended `alpha-skill`. Investigation showed alpha was admitted by weak evidence only — its body mentions "release notes" and gamma's body mention of `alpha-skill` added a graph boost. Relevance (textual relatedness) and necessity (the task actually requires the skill) were not separated, so any positive score produced a recommendation.

## Change

`src/search.rs` now separates evidence channels and gates admission structurally:

- **Direct relevance** (`name`, `description`, `context+description`) admits a skill.
- **Explicit dependency** (`requires-skills` / `requires_skills` resolved to a unique skill) admits its target when the declaring skill is admitted; one hop only, marked as `required by <skill>`.
- **Weak relatedness** (`body_text` matches, link-target matches, mentions, backlinks) only adjusts score/reasons and never admits a skill by itself.
- Markdown links and mentions are relatedness evidence, not necessity, matching the contract that a Markdown link alone does not imply required application.

`docs/specs/core-contract.md` records the admission rule. `tests/core.rs` now asserts the release-notes recommendation contains only `gamma-skill`.

## Dataset regression cases

`evals/dataset/cases.jsonl` grew from 4 to 7 cases, adding positive (`rust-checks`, `release-announcement`, `verification-only`) and tightening forbidden/unused expectations (`gamma-skill` forbidden for Rust/test cases, `alpha-skill`+`beta-skill` forbidden for release announcement). This is a regression set for the ranking fix, not the full E02 30–50 case dataset.

## Before / after

Before (initial verification):

```json
"summary": { "cases": 4, "required_recall": 1.0, "forbidden_hits": 1, "optional_hits": 2, "optional_total": 2, "empty_recommendations": 1 }
```

After:

```json
"summary": { "cases": 7, "required_recall": 1.0, "forbidden_hits": 0, "optional_hits": 2, "optional_total": 3, "empty_recommendations": 1 }
```

- `release-notes` now recommends only `gamma-skill` (was `gamma-skill`, `alpha-skill`).
- `rust-impl` / `rust-checks` no longer recommend unrelated `gamma-skill`.
- `tests` recommends only `beta-skill`; `alpha-skill` remains `expected_optional` but is not force-recommended because it is related, not necessary.

## Verification

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --quiet -- bench evals/dataset/cases.jsonl
```

All passed; bench summary above.

## Limits / next

- Lexical matching is English-substring based; Japanese task → English description cases still need a dataset entry and likely semantic or translation support before they can pass.
- Same-name / multi-origin cases need case schema support for skill identity (path/digest), not just name.
- Full E02 dataset (30–50 cases) and real-task-derived labels remain open under #2.

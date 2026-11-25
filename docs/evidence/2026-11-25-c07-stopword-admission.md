# Evidence — C07 stopword/punctuation admission fix (2026-11-25)

Issue: #46
Branch: `46` → `release-0-1-0`

## Problem

S02 live verification (Kilo anonymous route, real model) showed `recommend`
emitting irrelevant skills on prompts sharing only generic English words or
punctuation with skill descriptions:

- `What is 17 * 23? Answer with just the number.` → `code-review` (score 34)
- `Add a pub fn slugify... Keep it simple.` → `interaction-discipline` (54)
- naming/structure question → `natural-japanese`

Root cause: `terms()` kept every whitespace token with `len() >= 2` —
no stopword list, no punctuation stripping (`->` qualified). Admission
(`description_admitted` = ≥2 description hits with one df==1 term) was
satisfied by rare stopwords (`keep`, `just`).

## Fix

`src/search.rs`:

- `terms()` now trims non-alphanumeric edges from each token (drops `->`,
  `"word,`), requires ≥2 chars, and filters a ~120-entry English stopword
  list. Applied uniformly to name/description/context hits, scoring, and df.
- `field_score` takes the precomputed term list instead of re-splitting the
  raw query, so scoring and admission see the same terms.
- Admission remains structural; no score-threshold changes.

## Verification

- New regression test `stopword_and_punctuation_only_queries_do_not_recommend`
  (tests/core.rs): stopword-only, punctuation-only, and degenerate queries
  return `[]` on the fixture corpus.
- Real-corpus recheck (`/tmp/sg-live` 17-skill corpus): all three observed
  noise queries now return `[]`; the quality-gate task still recommends
  `quality-gate` (78), `design-refinement` (48), `system-quality` (28).
- `cargo test`: 10 core + 4 security tests pass. `cargo clippy -D warnings` clean.
- `bench evals/dataset/cases.jsonl`: required_recall 1.0, forbidden_hits 0 —
  unchanged.

## Known trade-off

`cargo test fails... Diagnose the failure and fix it.` now returns `[]` on the
real corpus: `diagnose` does not substring-match `diagnosis` in
diagnosing-bugs' description (pre-existing stemming limitation, previously
masked by stopword noise). Improving morphological matching is out of scope;
noted for future dataset/eval work.

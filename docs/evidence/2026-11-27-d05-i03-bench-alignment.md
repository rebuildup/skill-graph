# D-05 / I-03 bench alignment and holdout eval (#172)

Date: 2026-11-27. Branch: `172`. Findings from third-party review #168,
tracked under #172 (parent audit #144).

## D-05: quality/profile.json bench command

`quality/profile.json` `commands.bench` pointed at the directory
`evals/dataset`; `src/eval.rs` reads a single JSONL file, so the command
failed with `Is a directory`. Fixed to
`cargo run --quiet -- bench evals/dataset/cases.jsonl`, matching
`mise.toml`'s `bench` task. Verified: the command exits 0 and prints the
eval JSON.

## I-03: bench cutoff vs production top-3, plus holdout cases

Production surfaces top-3 to the model: the Rust hook requests
`recommend(..., 5)` then injects `recs[..min(3)]` (`src/hook.rs:605`),
and the OpenCode plugin calls `recommendNames(text, 3)`
(`src/assets/opencode-plugin.ts:362`). The bench harness previously
scored `required_recall` over `max=5`, crediting ranks 4-5 that never
reach the model.

Changes:

- `bench` gained `--max N` (default 3), matching the injected cutoff.
  `src/eval.rs` fetches `max(max, 5)` recommendations in one pass and
  reports `required_recall` at the primary cutoff plus diagnostic
  `required_recall_at3` / `required_recall_at5` (and per-case
  `required_hits_at3` / `required_hits_at5`).
- `evals/dataset/cases.jsonl` gained 15 `holdout-*` cases: natural
  user-task phrasing with no skill names or description vocabulary
  lifted from the targets (13 against `skills/`, 2 against
  `fixtures/skills`, 1 negative). Convention documented in
  `evals/dataset/schema.md`.

## Results

Before (43 cases, `max=5`, old harness): `required_recall` 1.0
(38/38), `forbidden_hits` 0.

After — existing 43 cases at `max=3`: `required_recall` 1.0 (38/38),
`required_recall_at3` = `required_recall_at5` = 1.0. All previously
required skills already ranked in the top-3.

After — full 58-case dataset at `max=3` (default):

```
required_recall      0.9038  (47/52)
required_recall_at3  0.9038
required_recall_at5  0.9038
forbidden_hits       1
precision_required   0.6104
precision_acceptable 0.8052
empty_recommendations 10
```

The drop is honest signal from the holdout cases, not a regression:
natural phrasing that avoids description keywords exposes real
admission gaps. Misses at top-3:

- `holdout-root-cause-bug` → `diagnosing-bugs`: empty recommendation.
- `holdout-new-teammate` → `onboarding`: empty (English query vs
  Japanese-only description; cross-lingual gap).
- `holdout-coherence` → `system-quality`: empty (same cross-lingual
  gap).
- `holdout-fresh-machine-checks` → `quality-gate`: `code-review`
  recommended instead (recorded as `forbidden_hits: 1`).
- `holdout-gamma-hotfix-note` → `gamma-skill`: empty on fixtures.

Labels were kept as the task-correct skills per `schema.md` label
rules; no case was reworded to preserve the old 1.0 score. Whether to
close these gaps (e.g. glossary coverage, admission thresholds) is
follow-up work outside this issue's scope.

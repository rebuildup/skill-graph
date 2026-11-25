# Evidence — C04 duplicate-install recommendation fix (2026-11-25)

Issue: #35
Branch: `35` → `release-0-1-0`

## Problem

This repository itself keeps identical copies of 17 skills under both `.claude/skills/` and `.agents/skills/` — the normal shape for multi-agent setups. Before the fix, `by_name` held 2 targets per name, so:

- `resolve_requires` only set `resolved_name` for unique names → `requires` edges never resolved in duplicated corpora.
- `document_frequency` counted both copies → name-term df became 2 → `unique_name_hit` admission failed.

Measured: `skill-graph recommend "system-quality" --root .claude/skills --root .agents/skills` returned `[]` (release build, pre-fix).

## Change

- `resolver.rs`: `resolve_requires` resolves to the skill *name* whenever the name exists, independent of copy count. `backlinks` records against the first target copy.
- `search.rs`: `recommend` collapses candidates to one representative per name — the first copy without a parse error, in deterministic discovery order. Document frequency, direct admission, seeds, and `requires` admission all operate on representatives only. `resolve_target` is unchanged: name resolution still reports `ambiguous: true` with all copies, preserving the S03 contract.

Rationale: the agent-facing identity is the skill name (what the Skill tool accepts); duplicate copies are the same logical skill for recommendation purposes. Which physical copy the runtime loads is the runtime's concern, not the recommender's.

## Verification

- New test `duplicate_installs_recommend_once_and_keep_requires` (tests/core.rs): two roots with identical `parent-skill`(requires `child-skill`) + `child-skill` → `parent-skill` recommended once, `child-skill` admitted via `required by` reason.
- `cargo test`: all pass (core 6, contract 4, security 4).
- `cargo run --bin skill-graph -- bench evals/dataset/cases.jsonl`: required_recall 1.0, forbidden_hits 0, recommended_total 54 — unchanged on the single-root fixture dataset.
- Real corpus: `recommend "system-quality" --root .claude/skills --root .agents/skills` now returns `system-quality` + `code-naming` (`required by system-quality`), where it previously returned `[]`.

## Limits

- Representative selection is first-parse-error-free copy in discovery order; same-name skills with *different* content are not diffed — `resolve` remains the tool for ambiguity inspection.
- Backlink reason counts may attribute to a non-representative copy index in duplicated corpora (cosmetic only).
- x02/x03 experiment reports predate this change; their single-root fixture results are unaffected.

# Evidence — C05 recommend identification contract for same-name skills (2026-11-25)

Issue: #39
Branch: `39` → `release-0-1-0`

## Problem

C04 deduplicated recommendations to the first parse-error-free copy per name. That fixed empty results on duplicated roots but introduced a contract gap: same-name skills with *different* content (distinct distribution/revision) had only one copy evaluated, so evidence from other copies was lost and `requires` to an ambiguous name silently resolved to discovery order.

## Change (`src/search.rs` only)

- Evidence evaluation runs over **all** non-parse-error copies again.
- `document_frequency` now counts **distinct skill names**: a term matches a name when any installed copy of that name contains it — duplicate installs no longer inflate df, and terms present only in a non-first copy still count.
- Output groups by `skill.name`: the highest-scoring copy represents the name (its own `path`/`digest` identify which copy won).
- When a recommended name has copies with **distinct digests**, reasons gain `name collision: N distinct versions installed`; identical-digest duplicates are deduplicated silently.
- `requires` admission now admits **all** non-error copies of the target name; output grouping still yields one recommendation per name.
- `resolve_target`, `availability`, `observation` contracts unchanged — ambiguity remains fully reported at resolve time; loaded/applied state remains a runtime-observation concern (`observation: "filesystem_only"`).

## Verification

New regression tests (`tests/core.rs`, now 9):

- `same_name_distinct_versions_are_flagged_not_merged_silently`: query matching only copy B's description → one `collide-skill` rec with `name collision` reason; `resolve` still reports `ambiguous: true` with 2 copies.
- `identical_duplicate_copies_do_not_flag_collision`: identical duplicates → one rec, no collision reason.
- `requires_to_ambiguous_name_admits_once_with_collision_note`: `requires-skills: dep-skill` with two distinct dep versions → one `dep-skill` rec carrying both `required by` and `name collision` reasons.
- Existing `duplicate_installs_recommend_once_and_keep_requires` still passes.

Bench (`evals/dataset/cases.jsonl`, single-root fixtures): required_recall 1.0, forbidden_hits 0, recommended_total 54, precision_acceptable 0.870 — unchanged.

Real corpus (`.claude/skills` + `.agents/skills`, identical duplicates): `recommend "system-quality"` → `system-quality`, `code-naming`, no collision reasons — correct silent dedup of identical copies.

## Limits

- The representative shown is the highest-scoring copy; consumers needing a specific revision must use `resolve` (which reports all copies with digests).
- df is computed per unique name; a term shared by two *different* skills still counts 2 as before.

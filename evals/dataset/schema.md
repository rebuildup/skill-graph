# Evaluation dataset schema

`cases.jsonl` is the canonical evaluation dataset for `skill-graph bench`. Each line is one JSON object. The harness reads the file from top to bottom and evaluates `recommend(graph, query, context, max=5)` against the labels below.

## Fields

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Stable case identifier. |
| `query` | yes | User task text presented to the recommender. May be Japanese or English. |
| `context` | no | Additional work-context text. Used with `query` for admission and ranking. |
| `skill_root` | yes | Repository-relative root passed to discovery. Skill identity is `(skill_root, name)`; the same name under another root is a different skill. |
| `expected_required` | no | Skill names that must be recommended for the case to count as a required hit. |
| `expected_optional` | no | Skill names that may be recommended without penalty. Often used for legitimate `requires` dependencies. |
| `forbidden` | no | Skill names that must not be recommended. |
| `timing` | no | Desired observation point, e.g. `session_start`, `mid_task`, `post_tool`. Documentation-only for the current harness. |
| `rationale` | no | Why the labels were chosen. Documentation-only for the current harness. |
| `case_kind` | no | `positive`, `negative`, `optional`, or `cross-lingual`. Documentation-only for the current harness. |

Unknown fields are ignored by the harness, but new fields should be added here first.

## Label rules

- Labels are defined from the task and the installed skill descriptions, not copied from observed invocation history.
- `expected_required` means the skill is applicable enough that a user would want it loaded at `timing`.
- `expected_optional` means the skill is a legitimate dependency or related skill, but not strictly required.
- `forbidden` means the skill is related enough to be a plausible false positive but not applicable to the task.
- Negative cases use empty `expected_required`; they are useful for measuring empty recommendations and forbidden hits.
- Real invocation logs are not labels. They may inform case selection, but the labels above are the evaluation contract.

## Privacy

- Do not commit conversation transcripts, user prompts from real sessions, secrets, or machine-local paths.
- Cases may be derived from real task shapes, but text is rewritten into minimal task/context strings.
- `skill_root` must stay inside the repository. Absolute paths and `..` segments are not allowed in committed cases.

## Coverage

The dataset should cover:

- implementation, tests, verification, release notes, documentation, UI, incident recovery, and parallel orchestration;
- required, optional, forbidden, and no-skill outcomes;
- session-start and mid-task timing;
- Japanese task text combined with English skill descriptions, and Japanese context matched to Japanese skill descriptions;
- fixture skills for controlled ranking regressions and installed project skills for realistic descriptions and `requires` edges.

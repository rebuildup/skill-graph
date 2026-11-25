# Evidence — E01 session observability (2026-11-25)

Issue: #1
Applied project-init revision: `285b139b01c199d3d4135d90f3cc3f6a932eff3b`

Scope: read-only inspection of local agent state. No agent logs, settings, or session files were modified. No conversation bodies or secrets are copied here; only aggregate counts and schema observations are recorded.

## Data sources inspected

| Source | Location | Observed size | Skill invocation observability |
|---|---|---:|---|
| Devin CLI session DB | `~/.local/share/devin/cli/sessions.db` | `sessions`=9, `message_nodes`=20,498, `tool_call_state`=6,954 rows / 8 sessions | `tool_call_state.tool_call_json` contains tool calls; parsing `inferenceToolName == "skill"` yields 79 invocations across 8 sessions. `source_node_ids` may allow timing joins; `tool_call_state` itself has no timestamp column. |
| Devin skill event spool | `~/.local/share/devin/cli/skill_events_spool.jsonl` | 5 events | Current-session spool only; not a historical source. |
| Devin transcripts / summaries / logs | `~/.local/share/devin/cli/transcripts/*.json` (10), `summaries/history_*.md` (41), `logs/*.log[.gz]` | many files | Transcripts/summaries are conversation artifacts, not invocation ledgers. Logs contain lines such as `skills discovery ... loaded=45 by_provider=[...]`; useful for availability, not per-invocation truth. |
| Claude Code projects | `~/.claude/projects/*/*.jsonl` | 19 files / 19 session ids | Line-term counts are mentions, not invocations. A structured parser for Claude tool-use records is still required before using this as a label source. |
| Codex state | `~/.codex/state_5.sqlite`, `thread_history_1.sqlite`, `logs_2.sqlite`, `sessions/**/*.jsonl` | `threads`=3, `thread_turns`=4, `thread_items`=6, rollout jsonl=3 | Local footprint is small; not the source of the reported 190-session aggregate. Needs a dedicated parser if used later. |

## Reproduced aggregate (Devin `sessions.db`)

Parsing `tool_call_state.tool_call_json` as JSON and filtering `_meta["cognition.ai/inferenceToolName"] == "skill"` produced:

- total skill invocations: **79**
- distinct sessions with skill calls: **8**

Top observed names (count / distinct sessions):

| skill | calls | sessions |
|---|---:|---:|
| github-delivery | 17 | 8 |
| engineering-decisions | 10 | 7 |
| correctness-assurance | 10 | 6 |
| quality-gate | 8 | 4 |
| agent-recovery | 5 | 4 |
| writing-discipline | 4 | 3 |
| parallel-orchestration | 3 | 2 |
| design-refinement | 3 | 3 |
| worktree-workflow | 3 | 1 |
| interaction-discipline | 2 | 2 |

This is a **local subset**, not the reported 190-session / 741-call aggregate. The original aggregate source was not identified in the inspected local stores; treat `190 sessions / 741 calls / 43+1 kinds` as a reported context value, not as a label source.

## Observability classification

- **Observed (usable now):** Devin `tool_call_state` skill invocations for the 8 sessions present in the current DB; skill name per invocation; co-occurrence within those sessions.
- **Partially observed:** load timing — `tool_call_state` lacks timestamps, but `source_node_ids` may join to `message_nodes.created_at`; needs verification before use in E02.
- **Partially observed:** available-skill inventory — Devin logs record `skills discovery ... loaded=N by_provider=[...]`, but mapping that inventory to each stored session is not yet proven.
- **Not observed:** the full 190-session history, per-invocation skill revision/digest for most sources, Claude/Codex invocation ledgers without new parsers, and any claim about *why* a skill was not invoked.
- **Not label-safe:** Claude/Codex raw text mentions and Devin `message_nodes` content matches (e.g. `SKILL.md` substring counts) are mention-level signals only.

## Implications for E02

1. Use parsed Devin `tool_call_state` invocations for a small real-task subset (timing, co-occurrence, transition) after verifying the `source_node_ids → message_nodes.created_at` join.
2. Treat the reported 190/741 aggregate as context only; do not derive labels from it.
3. Build the 30–50 case dataset primarily from task-shaped fixtures plus the verified Devin subset; keep any real session excerpts out of the repo.
4. If Claude/Codex become label sources, add a dedicated read-only parser issue before E02 freezes.

## Commands used (read-only)

```bash
python3 - <<'PY'
import sqlite3, json, collections
con = sqlite3.connect('file:/home/basic/.local/share/devin/cli/sessions.db?mode=ro', uri=True)
counts = collections.Counter(); sessions = collections.defaultdict(set); total = 0
for sid, tcj in con.execute('select session_id, tool_call_json from tool_call_state'):
    try:
        obj = json.loads(tcj or '')
    except Exception:
        continue
    if (obj.get('_meta') or {}).get('cognition.ai/inferenceToolName') != 'skill':
        continue
    skill = (obj.get('rawInput') or {}).get('skill')
    if not skill:
        continue
    counts[skill] += 1
    sessions[skill].add(sid)
    total += 1
print(total, len(set().union(*sessions.values())) if sessions else 0)
for name, c in counts.most_common():
    print(name, c, len(sessions[name]))
PY
```

Schema probes used `PRAGMA table_info` / `sqlite_master` via Python `sqlite3` with `mode=ro`. `sqlite3` CLI is not installed on this machine.

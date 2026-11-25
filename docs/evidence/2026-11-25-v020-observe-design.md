# v0.2.0 #55 — 最小観測（既存ログ優先）の設計と実装

Date: 2026-11-25
Issue: #55

## 方針

新規ログ生成を前提にせず、まず**既存のエージェント transcript を read-only で
解析**する。ランタイムが標準で残す記録を一次情報源とし、hook 側の opt-in
JSONL（`SKILL_GRAPH_HOOK_LOG`）は補助信号に留める。

## 実装: `skill-graph observe <path>`

- `src/observe.rs` — ファイル/ディレクトリ再帰で `*.jsonl` を走査し、
  Claude Code transcript の `assistant` エントリ内 `tool_use`/`Skill`/
  `input.skill` を抽出。namespaced skill (`ns:name`) は name 部に正規化。
- 出力: `files_scanned`, `lines_skipped`, `sessions`, `skill_loads`
  （name→回数）、`loads`（session/timestamp/file 付き明細）。
- `tests/observe.rs` — 集計・namespaced 正規化・非 Skill tool・壊れた行の
  スキップを検証。

## 実コーパスでの初回観測（`~/.claude/projects`, 31 files / 22 sessions）

- `Skill` tool_use は **1 件のみ**（`using-superpowers`）。
- 大半の session では skill 起動が `tool_use`/`Skill` として記録されない
  （skill 本文がメッセージへ直接注入される経路、または記録形式の差異）。

**示唆**: transcript だけでは invocation を網羅できない。v0.2.0 の観測は
(a) transcript の Skill tool_use（下限値として有効）、(b) hook の
`SKILL_GRAPH_HOOK_LOG`（推薦・skill-load イベント双方を記録する opt-in 信号）、
(c) 将来の Codex rollout / OpenCode DB パーサ、の3層で設計する。
transcript の skill-load 欠損は「ロードされなかった」ではなく「記録形式に
現れなかった」可能性があるため、負の証拠として扱わない。

## 他ランタイムの対応状況

| Runtime | 既存ログ | 本実装 |
|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` | ✅ `observe` で解析可能 |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` | 未対応（形式調査が次ステップ） |
| OpenCode | `opencode.db` (SQLite) | 未対応（schema 調査が次ステップ） |

## 境界

- read-only。transcript・会話全文をリポジトリへ混入させない
  （集計値と skill 名のみ出力。`loads` 明細は timestamp/file path を含むが
  プロンプト本文は含まない）。
- `SKILL_GRAPH_HOOK_NOSTATE=1` の無保存モードとは独立: observe は常に
  read-only で副作用なし。

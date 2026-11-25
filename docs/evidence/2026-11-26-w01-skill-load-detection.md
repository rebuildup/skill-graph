# #76 W01 — ランタイム別スキル読み込み検出方式の比較調査

Date: 2026-11-26
Issue: #76 (v0.3.0 W01)
Runtimes verified live: codex-cli 0.171.0 (ChatGPT auth), opencode 2.3.2
(`--standalone`, hungerg-free), claude 2.1.329 (prior S02 evidence)

## 結論サマリ

**3ランタイム全てで実現可能な観測経路が存在する。** Codex の「Skill tool
非存在」は検出不能を意味しない — Codex は skill を必ず shell 経由で読む
ため、`PostToolUse` の `Bash` マッチャが実用上の skill-load シグナルになる。
OpenCode は `ctx.tool.hook("execute.after")` が tool 実行を直接捕捉でき、
DB (`opencode.db`) の read-only 解析も実データで検証済み。

## ランタイム × 方式 比較表

| ランタイム | 方式 | 状態 | コスト | 網羅性 |
|---|---|---|---|---|
| Claude Code | `PostToolUse` matcher `Skill`（tool_input.skill） | ✅ 実測済 (S02) | hook 1回/skill load、~25–45ms | 高：skill tool 使用時は確実。skill を読まずに済む場合は発火しない（=load なし、正しい） |
| Claude Code | transcript JSONL read-only 解析 (`observe`) | ✅ 実装済 (#55) | 低：既存ログの走査のみ | 中：transcript の Skill 記録率が低い（22 sessions→1 load の実測）。hook log が補完信号 |
| Codex | `PostToolUse` matcher `Bash` + `tool_input.command` の SKILL.md パス解析 | ✅ **本日実測** | hook 1回/Bash 呼出 | 高（shell 経由の read は全て捕捉）。apply_patch は読取に使えない、exec JS 内の exec_command も Bash として surface することを確認 |
| Codex | rollout JSONL read-only 解析 | ✅ **本日実測** | 低 | 中〜高：`custom_tool_call`/`custom_tool_call_output` に全 tool 呼出と出力が記録される。`observe` 拡張で対応可能 |
| OpenCode | plugin `ctx.tool.hook("execute.after")` | ✅ **本日実測** | hook 1回/tool 実行 | 高：`event.tool`/`event.input`/`event.result` が構造化。`read` tool の `input.path`、shell の `input.command` を解析 |
| OpenCode | `opencode.db` (`session_message.data.content`) read-only 解析 | ✅ **本日実測**（実 DB 1179 messages） | 低 | 中〜高：全 tool call が `type:"tool"` item として永続化。既存履歴も遡及解析可能 |
| OpenCode | `ctx.event.subscribe` イベントストリーム | API 確認済・本プラグインでは未使用 | — | `tool.execute.*` 等が流れるが `tool.hook` で十分 |

## Codex 実測詳細（本日）

プロジェクト `/tmp/sg-codex-proj`、`.codex/hooks.json` に debug hook を追加し
`codex exec --trust --dangerously-bypass-hook-trust --skip-git-repo-check`
で「`.agents/skills/alpha-skill/SKILL.md` を読め」を実行。

### PostToolUse payload（実測値）

```json
{
  "hook_event_name": "PostToolUse",
  "tool_name": "Bash",
  "tool_input": {"command": "cat .agents/skills/alpha-skill/SKILL.md"},
  "tool_response": "---\nname: alpha-skill\ndescription: ...\n---\nAlpha body.\n",
  "tool_use_id": "exec-...",
  "session_id": "...", "turn_id": "...",
  "transcript_path": "~/.codex/sessions/**/rollout-*.jsonl",
  "cwd": "/tmp/sg-codex-proj"
}
```

- matcher `"Bash"` で発火。matcher `"shell"` では発火しない（tool_name は
  `"Bash"`）。matcher 省略の catch-all でも発火。
- `tool_response` にファイル内容全文 → frontmatter `name:`/`description:`
  から skill 名を確定的に抽出可能。パスだけでなく内容で検証できる。
- Codex の tool surface（rollout `additional_tools` 項目で確認）:
  `exec`（JS orchestration）、`apply_patch`、`view_image`、`write_stdin`、
  MCP tools、`web__run`、clock/collaboration。**ファイル読取は
  `exec_command`（shell）のみ** — `apply_patch` は書込専用で skill 本文を
  「読む」用途には使えない。したがって skill-load は実質全て Bash 経由。

### rollout JSONL 構造（実測値）

- `response_item` type=`custom_tool_call`: `name:"exec"`,
  `input` に JS ソース（`tools.exec_command({cmd:"cat .../SKILL.md"})` を含む）
- type=`custom_tool_call_output`: `call_id` で対応、output 内に実行結果 JSON
  （exit_code + output 本文）
- `event_msg` type=`item_completed`/`task_complete` 等の進行イベント
- read-only 解析では `custom_tool_call.input` 内の `cmd:"..."` から
  SKILL.md パスを抽出可能。hook 非依存の遡及解析が成立

### Codex の FP/FN 評価（`tool_input.command` 解析）

- **TP**: `cat/sed/head/python -c open()/bat` 等、SKILL.md を読む一切の shell
  コマンド。パスは `.agents/skills/<name>/SKILL.md`、`.claude/skills/...`、
  `~/.codex/...` 等あらゆる skill root をカバー（パス正規化が必要）
- **FP 候補**: `find . -name SKILL.md`（列挙のみ）、`grep -r SKILL.md`
  （内容検索）、`ls skills/` — パス文字列は含むが load ではない。
  **対策**: (a) コマンドが read 系動詞で始まること、(b) `tool_response` が
  YAML frontmatter（`---\nname:`）を含むこと、の AND 条件で精度を上げられる
- **FN 候補**: `spawn_agent` の sub-agent が親 session の hook を経由せず
  skill を読む可能性（未検証のエッジ）、MCP resource 経由の skill 公開
  （実用上稀）
- 結論: hook 経路は実用十分。observe 側は frontmatter 確認で TP を強化可能

## OpenCode 実測詳細（本日）

プロジェクト `/tmp/sg-oc-proj`（fixture skill `.opencode/skill/probe-skill/`）、
probe plugin `/tmp/sg-oc-plugin` で `opencode run --standalone` を実行。

### `ctx.tool.hook` payload（実測値）

```
BEFORE tool=read input={"path":"/tmp/sg-oc-proj/.opencode/skill/probe-skill/SKILL.md"}
AFTER  tool=read status=completed input={...同上...}
       result={"output":{"type":"text","content":"---\nname: probe-skill\n..."}}
```

- `execute.before`/`execute.after` 両方が発火。`event.tool` はツール名
  （`read`, `shell`, `grep`, `edit` 等）、`event.input` はツール固有 input
  （read→`{path}`、shell→`{command}`）、`event.status`、`event.result` に
  出力内容 → Codex と同じく frontmatter による確定的検証が可能
- skill-load の判定: `tool=="read" && input.path` が `*/SKILL.md`、または
  `tool=="shell" && input.command` が read 系 + SKILL.md パス
- `ctx.skill.list()` も利用可能（plugin から skill registry を列挙できる
  ことを確認。`event.prompt.skills` 直接注入の ID 解決にも使える）

### opencode.db read-only 解析（実測値）

`~/.local/share/opencode/opencode.db` (SQLite) の `session_message` テーブル:

- `type='assistant'` 行の `data.content[]` に `type:"tool"` item として
  全 tool call が永続化（`name`, `state.input`, `state.content`）
- 既存 DB（1179 messages）から SKILL.md 関連 tool event を抽出する解析を
  実施: 厳密な load 判定で 6 件、曖昧な言及で 5 件を分離できた
  - TP 例: `sed -n '95,140p' .agents/skills/worktree-workflow/SKILL.md`、
    `read .opencode/skill/probe-skill/SKILL.md`
  - FP 例: `grep pattern skills/*/SKILL.md`（内容検査で load ではない）、
    `skills-lock.json` 操作
- `observe` 拡張の入力として十分な構造。session/turn 対応付けに
  `session_message.session_id`/`seq` が使える

## Claude Code（既存証跡の整理）

- `PostToolUse` matcher `Skill`: `tool_input.skill` に skill 名が直接入る
  （S02 で実測、adapter 実装済み）。namespaced name の正規化も engine 層で
  検証済み
- transcript read-only (`observe`): Skill tool 記録率が低い実測あり
  （22 sessions → 1 load）。hook log (`skill-graph` の emit 記録) が
  補完信号として機能する設計済み
- skill を file read で読む運用（adapter が SKILL.md パスを直接教える
  場合を含む）は PostToolUse Skill では捕捉されない — Codex と同じ
  「Bash/file-read 経由」の盲点は Claude Code にも存在するが、Claude Code
  では Skill tool が canonical 経路のため優先度は低い

## W02（Skill Usage Event 契約）への入力

正規化イベントに必要なフィールド（全ランタイムで取得可能と実測確認）:

| field | Claude Code | Codex | OpenCode |
|---|---|---|---|
| runtime | `claude-code` | `codex` | `opencode` |
| session_id | hook payload / transcript path | hook `session_id` / rollout `session_meta` | plugin `sessionID` / DB `session_id` |
| tool | `Skill` | `Bash` (+command解析) | `read`/`shell` (+input解析) |
| skill_name | `tool_input.skill` | path 親 dir or frontmatter `name:` | path or frontmatter `name:` |
| skill_path | — (namespaced) | `tool_input.command` 内パス | `input.path` |
| content_digest | 不要（name で識別) | `tool_response` frontmatter で検証可 | `event.result` で検証可 |
| observed_via | `hook` | `hook` / `transcript` | `hook` / `db` |
| confidence | high | high (frontmatter 確認時) / medium (path のみ) | high (frontmatter 確認時) / medium |

設計示唆:

- **hook 経路を第一級、transcript/DB を補完・遡及用**にする。hook は
  real-time で依存注入（W05 連携）に使え、transcript は hook 未導入期間や
  別マシンの履歴をカバーする
- skill_name の確定は **path 解決 + frontmatter `name:` 照合**を標準手順に
  すると、同名 skill の識別（C04/C05 契約）と整合する
- Codex/OpenCode とも「skill-load = SKILL.md への read 系アクセス」と
  定義すれば検出可能。`grep`/`find`/`ls` 等の非 load アクセスを除く
  ヒューリスティック（read 動詞 + frontmatter 確認）を W02 で契約化する
- Codex の `PostToolUse` は現行 adapter で matcher `Skill`（不発）のみ。
  W02 で matcher `Bash` エントリを追加し、command 解析を hook script に
  実装するのが自然な拡張

## 未検証・残件

- Codex `spawn_agent` sub-agent 内の skill 読取が親 session の hook/rollout
  に現れるか（エッジケース、実害は限定的）
- OpenCode `event.prompt.skills` 直接注入（推薦側の改善であり W01 範囲外）
- Claude Code で Skill tool を介さない SKILL.md file read がどの程度あるか
  （transcript 解析で今後計測可能）

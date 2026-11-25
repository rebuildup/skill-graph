# Standards Spec

調査日: 2026-11-25。適用 revision / version は実装時に再確認する。

| 標準 | 状態 | 初期適用 | 備考 |
|---|---|---|---|
| Agent Skills | 公開仕様 | `SKILL.md` + YAML frontmatter。`name` / `description` 必須、`license` / `compatibility` / `metadata` / `allowed-tools` 任意 | agentskills.io/specification。`metadata` は string map。独自必須化はしない |
| Agent Plugins | 公開仕様 v1.0.0 | `plugin.json`、`skills/`、`mcp.json`、client extension namespace | 配布形式の第一候補。Hooks は client extension |
| CommonMark | 規範仕様 0.31.2 | Markdown 本文のリンク抽出 | リンク関係の意味論は標準化されていない。参照は明示リンクとして扱う |
| YAML | 仕様 1.2 | frontmatter 解析 | `SKILL.md` の frontmatter は YAML。実装は厳密な YAML 1.2 subset を前提にしない |
| URI Generic Syntax | RFC 3986 | link / file URI 解決 | 相対参照解決と正規化を利用 |
| file URI | RFC 8089 | file URI 入力 | local file URI のみ対象。UNC 等の非標準は拒否または明示診断 |
| JSON Schema | 公開仕様 | eval dataset / plugin manifest / output contract の検証 | draft は実装時に固定 |
| MCP | 公開仕様 | 将来の adapter 境界 | 初期実装では必須にしない |
| XDG Base Directory | Version 0.8 | user-level state / config の将来配置 | project 内への書き込みは禁止のまま |
| SemVer | 2.0.0 | release / version 表記 | public API 変更で major を上げる |
| Agent Skills Discovery RFC | Draft v0.2.0 | 外部 discovery の参考 | IETF 標準ではない。`/.well-known/agent-skills/index.json` は提案 |
| RFC 8288 | 規範仕様 | typed link relation の設計参考 | CommonMark link に意味が標準化されているとは扱わない |

## ランタイム調査メモ

- Claude Code: `SKILL.md` を Agent Skills 標準で扱い、project `.claude/skills`、user `~/.claude/skills`、plugin 等から discovery する。Hooks は settings / plugin / skill frontmatter で `PreToolUse` 等を定義できる。
- Codex: `SKILL.md` を Agent Skills 標準で扱う。Hooks は `hooks.json` / config で `SessionStart`、`UserPromptSubmit`、`PreToolUse` 等を定義できる。
- OpenCode: `.opencode/skills`、`.claude/skills`、`.agents/skills`、HTTP catalog を discovery する。Hooks 相当は plugin JS/TS の event hook。
- 共通化できるのは「スキル metadata の一覧化」まで。Hook event、payload、context 注入、信頼境界は runtime 固有として adapter に閉じ込める。

## 既存ツール

- skillagent.dev / SerpentStack / skill-finder / OpenAgentSkill 等は外部 registry・directory 検索が中心。
- skill-graph の差分は、導入済みローカルスキルの参照グラフ、作業コンテキスト、観測状態、推薦根拠を扱う点に置く。

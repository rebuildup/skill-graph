# Standards Spec

調査日: 2026-11-25。適用 revision / version は実装時に再確認する。外部仕様は証拠であり、project-local policy を暗黙に上書きしない。

## Normative / public specifications

| 標準 | Status | Version / revision | Source | Compatibility note |
|---|---|---|---|---|
| Agent Skills | 公開仕様 | `SKILL.md` + YAML frontmatter | https://agentskills.io/specification | `name` / `description` 必須、`license` / `compatibility` / `metadata` / `allowed-tools` 任意。`metadata` は string map。独自必須化はしない |
| Agent Plugins | 公開仕様 | v1.0.0 | https://agent-plugins.org/ | `plugin.json`、`skills/`、`mcp.json`、client extension namespace。配布形式の第一候補。Hooks は client extension |
| CommonMark | 規範仕様 | 0.31.2 | https://spec.commonmark.org/ | Markdown 本文のリンク抽出に使う。リンク関係の意味論は標準化されていない |
| YAML | 規範仕様 | 1.2 | https://yaml.org/spec/ | `SKILL.md` frontmatter は YAML。実装は厳密な YAML 1.2 subset を前提にしない |
| URI Generic Syntax | 規範仕様 | RFC 3986 | https://www.rfc-editor.org/rfc/rfc3986 | link / file URI 解決。相対参照解決と正規化を利用 |
| file URI | 規範仕様 | RFC 8089 | https://www.rfc-editor.org/rfc/rfc8089 | local file URI のみ対象。UNC 等の非標準は拒否または明示診断 |
| JSON Schema | 公開仕様 | draft は実装時に固定 | https://json-schema.org/ | eval dataset / plugin manifest / output contract の検証に使う |
| MCP | 公開仕様 | version は adapter 実装時に固定 | https://modelcontextprotocol.io/ | 将来の adapter 境界。初期実装では必須にしない |
| XDG Base Directory | 公開仕様 | 0.8 | https://specifications.freedesktop.org/basedir-spec/latest/ | user-level state / config の将来配置。project 内への書き込みは禁止のまま |
| SemVer | 公開仕様 | 2.0.0 | https://semver.org/ | release / public API version 表記 |
| RFC 8288 | 規範仕様 | — | https://www.rfc-editor.org/rfc/rfc8288 | typed link relation の設計参考。CommonMark link に意味が標準化されているとは扱わない |

## Proposals / unstable references

| 項目 | Status | Source | 扱い |
|---|---|---|---|
| Agent Skills Discovery | 提案 / draft | agentskills.io ecosystem notes; exact draft revision は I01 で固定 | `/.well-known/agent-skills/index.json` は提案。IETF 標準ではない。外部 discovery は core contract に必須化しない |

## ランタイム調査メモ

- Claude Code: `SKILL.md` を Agent Skills 標準で扱い、project `.claude/skills`、user `~/.claude/skills`、plugin 等から discovery する。Hooks は settings / plugin / skill frontmatter で `PreToolUse` 等を定義できる。Hook event・payload・context 注入・信頼境界は runtime 固有。
- Codex: `SKILL.md` を Agent Skills 標準で扱う。Hooks は `hooks.json` / config で `SessionStart`、`UserPromptSubmit`、`PreToolUse` 等を定義できる。event/payload は Claude Code と共通と仮定しない。
- OpenCode: `.opencode/skills`、`.claude/skills`、`.agents/skills`、HTTP catalog を discovery する。Hooks 相当は plugin JS/TS の event hook。
- 共通化できるのは「スキル metadata の一覧化」まで。Hook event、payload、context 注入、信頼境界は runtime 固有として adapter に閉じ込める。

## 既存ツール

- skillagent.dev / SerpentStack / skill-finder / OpenAgentSkill 等は外部 registry・directory 検索が中心。
- skill-graph の差分は、導入済みローカルスキルの参照グラフ、作業コンテキスト、観測状態、推薦根拠を扱う点に置く。

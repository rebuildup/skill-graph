# Architecture

## Goal

特定エージェントに依存しない、Agent Skills の発見・参照解決・推薦エンジンを提供する。コアエンジンはオンメモリで完結させ、各利用インターフェースはその薄い入口とする。

## Layers

```text
Entry points（いずれも薄い入口）
  CLI / Runtime Hook / Adapter / OpenCode Plugin / MCP server / observe
        |
   Application API（discovery / resolver / search / events / observe）
        |
 Search / Recommendation
        |
Discovery / Parser / Resolver
        |
     Filesystem
```

- `Discovery`: 許可された root から `SKILL.md` を列挙する。
- `Parser`: YAML frontmatter と Markdown 本文を解析する。
- `Resolver`: 相対リンク、名前言及、requires-skills、配布元、digest を解決する。
- `Search / Recommendation`: クエリと作業コンテキストに対して候補と根拠を返す。
- `CLI`: 上記を人間とスクリプトから利用するための薄い入口。
- `Hook`（`skill-graph hook`）: ランタイムの hook payload を読み、Skill Usage Event を発行し `additionalContext` を返す。常に fail-open。
- `Adapter`（`skill-graph adapter`）: ランタイムのネイティブ設定へ hook/plugin エントリを冪等 merge/remove するライフサイクル管理。
- `OpenCode Plugin`: adapter が materialize する TS plugin。同梱エンジンを `skill-graph` CLI 経由で呼び、event log へ JSONL を追記する。
- `MCP server`（`skill-graph mcp`）: stdio transport の read-only MCP server（ADR-0005 / v1.0.0 spec §1）。`recommend` / `discover` / `resolve` の 3 tool を公開し、event log・dedup state・設定には触れない。graph は process 内で roots signature 単位に memoize する（永続キャッシュではない）。
- `observe`: transcript / `opencode.db` / event log を read-only で走査し、Skill Usage Event contract に正規化して集計する（`--summary` で funnel 指標）。

## Initial storage modes

| Mode | Behavior |
|---|---|
| Stateless | メモリのみ。単発解析・推薦。 |
| Session-scoped | hook の dedup marker / `.recommended` 等、runtime 管理または OS 一時領域の最小状態。 |
| Cached | 性能不足が実測された場合のみ導入。 |
| Instrumented | 明示的に有効化した評価・統計記録。 |

## Boundaries

- 解析対象は信頼できないデータとして扱う。
- ファイルシステム読み取りは許可 root 内に限定する。
- 外部ネットワーク取得は既定で行わない。
- 永続キャッシュ、グラフDB、Embedding、常駐サービスは v1.0.0 spec §6 の判定により不採用。
- MCP Server は ADR-0005 により **採用済み**（stdio・read-only 3 tools・観測イベントを発行しない）。v1.0.0 spec §1 が正本。
- event log は `${TMPDIR}/skill-graph-events.jsonl`（`SKILL_GRAPH_EVENT_LOG` で変更可）への JSONL 追記のみ。prompt 本文・コマンド・tool response は保存しない。

## Identity

同名スキルを誤って同一視しないため、少なくとも次を区別する。

- skill name
- source root / distribution origin
- canonical path
- content digest
- frontmatter revision-like metadata（存在する場合）

曖昧な場合は曖昧な結果を返し、暗黙に一つへ解決しない。

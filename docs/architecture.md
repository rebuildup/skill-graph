# Architecture

## Goal

特定エージェントに依存しない、Agent Skills の発見・参照解決・推薦エンジンを提供する。初期実装はオンメモリで完結させる。

## Layers

```text
CLI / Bootstrap / Runtime Hook
        |
   Application API
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

## Initial storage modes

| Mode | Behavior |
|---|---|
| Stateless | メモリのみ。単発解析・推薦。 |
| Session-scoped | 必要時のみ、セッション内の最小状態を保持。 |
| Cached | 性能不足が実測された場合のみ導入。 |
| Instrumented | 明示的に有効化した評価・統計記録。 |

## Boundaries

- 解析対象は信頼できないデータとして扱う。
- ファイルシステム読み取りは許可 root 内に限定する。
- 外部ネットワーク取得は既定で行わない。
- 永続キャッシュ、DB、常駐プロセス、MCP Server は初期構成に含めない。

## Identity

同名スキルを誤って同一視しないため、少なくとも次を区別する。

- skill name
- source root / distribution origin
- canonical path
- content digest
- frontmatter revision-like metadata（存在する場合）

曖昧な場合は曖昧な結果を返し、暗黙に一つへ解決しない。

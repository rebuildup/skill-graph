# skill-graph

Agent Skills の発見・参照解決・推薦を支援する、エージェント非依存のツール。

標準の Agent Skills を変更せず、導入済みスキルの `SKILL.md`、Markdown リンク、frontmatter、作業コンテキストから、必要なスキルへ到達できるようにすることを目的とする。

## Status

初期化中。評価基盤・実現可能性検証・最小プロトタイプを並行して整備している。

## Documents

- 開発契約: `AGENTS.md`
- 開発環境: `docs/development.md`
- アーキテクチャ: `docs/architecture.md`
- 仕様: `docs/specs/`
- 設計判断: `docs/adr/`
- 評価計画: `docs/specs/evaluation.md`

## Quickstart

前提: Rust toolchain と mise。

```bash
mise install
mise run check
cargo run -- search "release notes"
```

## Principles

- 標準 Agent Skills を原則として変更しない。
- 対象プロジェクトへキャッシュ、ログ、管理ファイルを生成しない。
- 推薦は根拠と観測状態を返し、候補がなければ無出力とする。
- CLI、Hook、Plugin、将来の MCP は共通エンジンの薄い入口とする。

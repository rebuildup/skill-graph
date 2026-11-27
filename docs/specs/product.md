# Product Spec — skill-graph

> 旧称注記: 本文中の `project-init` は `rebuildup/project-init` の旧称であり、
> 現行の canonical repository は `rebuildup/aude-skills`（改名追跡:
> rebuildup/aude-skills#212）。

## 目的

Agent Skills の存在をエージェントが自発的に認識できない問題を、標準 Markdown / frontmatter / 参照関係 / 作業コンテキストを使って改善する。単なる検索 CLI ではなく、必要なスキルが必要なタイミングで読み込まれることを目指す。

## 最終要求

- 導入済み Agent Skills の発見
- 標準 Markdown と frontmatter の解析
- スキル間参照の解決
- 関連スキル、バックリンク、依存関係の取得
- 作業内容に基づく根拠付き推薦
- 既存エージェントログからの利用実績分析
- 推薦性能と実際の呼び出し性能の評価
- CLI、各エージェント向け Plugin、必要時の Hook / MCP 連携

## 非目標（初期）

- project-init 専用ツールにすること
- スキル管理・導入ツールの置き換え
- 標準 Agent Skills の変更
- 永続キャッシュ、グラフDB、Embedding、常駐サービス、MCP Server の先行実装
- 呼び出し回数増加自体を成功条件にすること

## MVP

評価ハーネス、オンメモリ Discovery / Parser / Resolver、テキスト検索と参照関係による推薦、最小 CLI、既存開始経路を使う実験、単一ランタイム向け最小 Hook 実験、比較評価を含む。

E03（ADR-0002）で採用方式を確定済み: group F の推薦ポリシー（直接適合 admission + 1-hop `requires` necessity）を `UserPromptSubmit` hook から top-3 で注入する。P01（#28）・P02（#29）で製品化し、v0.1.0（`release-0-1-0` @ `80f8424`）を実験的初期リリース候補とした（R03 #50）。v0.2.0 は実開発環境への適用拡大と実運用での呼び出し性能評価を主目的とし、Issue #51–#57 で管理する。

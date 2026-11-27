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

初期スコープ（v0.1.0〜v0.3.x）における非目標。v0.4.0 / v1.0.0 spec chain
でスコープが拡張された項目は各 spec を正本とする。

- project-init 専用ツールにすること
- スキル管理・導入ツールの置き換え
- 標準 Agent Skills の変更
- 永続キャッシュ、グラフDB、Embedding、常駐サービスの先行実装
  （v1.0.0 spec §6 で不採用を確定）
- 呼び出し回数増加自体を成功条件にすること

注: 「MCP Server の先行実装」は初期の非目標だったが、v1.0.0 spec §1 /
ADR-0005 により **正式な完成対象として採用**され、`skill-graph mcp`
（stdio・read-only）として v0.4.0 で実装済み。現行の非目標は
`docs/specs/v1.0.0.md`「非目標（v1.0.0 でも維持）」を参照。

## MVP

評価ハーネス、オンメモリ Discovery / Parser / Resolver、テキスト検索と参照関係による推薦、最小 CLI、既存開始経路を使う実験、単一ランタイム向け最小 Hook 実験、比較評価を含む。

E03（ADR-0002）で採用方式を確定済み: group F の推薦ポリシー（直接適合 admission + 1-hop `requires` necessity）を `UserPromptSubmit` hook から top-3 で注入する。P01（#28）・P02（#29）で製品化し、v0.1.0（`release-0-1-0` @ `80f8424`）を実験的初期リリース候補とした（R03 #50）。v0.2.0 は実開発環境への適用拡大と実運用での呼び出し性能評価を主目的とし、Issue #51–#57 で管理する。

その後の spec chain で完成形へ拡張した: v0.3.0 で Skill Usage Event 契約と `observe` 観測、v0.4.0 で hook/adapter のバイナリ内包・パッケージマネージャ配布・MCP server・`observe --summary`・session-context 推薦を実装（`docs/specs/v0.4.0-package-manager-distribution.md`、ADR-0004/0005）。v1.0.0 はこれらを含む正式完成版として `docs/specs/v1.0.0.md` が定義する。

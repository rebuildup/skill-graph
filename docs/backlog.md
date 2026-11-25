# Issue Drafts

GitHub repository が未作成のため、以下は登録予定の draft である。repository 作成後、GitHub Issue へ転記し、GitHub Issue を正本とする。

| Draft | Title | Depends | Acceptance summary |
|---|---|---|---|
| E01 | 既存セッションの観測可能性・発火傾向分析 | — | 元ログ所在、取得可否、観測/非観測の区別、集計制約を記録 |
| E02 | 評価データセットとBaseline | E01 | 30〜50 case、ラベル規則、baseline metric を定義 |
| S01 | 標準仕様・既存ツール調査 | — | standards.md の version 固定と runtime 差分を記録 |
| S02 | ランタイム・Plugin・Hook 実現可能性検証 | S01 | Claude/Codex/OpenCode の discovery/hook/context 注入を実測 |
| S03 | 最小の参照・推薦・イベント契約 | S01 | 入出力 schema と互換性条件を Spec 化 |
| S04 | セキュリティ・非汚染・保存境界 | S01 | threat model、拒否条件、no-save mode を契約化 |
| C01 | オンメモリ Discovery・Parser・Resolver | S03, S04 | fixture で parse/resolve/link/backlink/dependency を検証 |
| C02 | テキスト検索と参照関係による推薦 | C01, E02 | eval dataset で precision/recall/reason を検証 |
| C03 | 実データによる性能ベンチマーク | C02 | 50〜500 skills で cold/warm/Hook 起動時間を計測 |
| X01 | description 改善実験 | E02 | group B の再現可能な比較 |
| X02 | Bootstrap・既存 skill 経由の推薦実験 | C02 | group C/D の隔離環境で検証 |
| X03 | 単一ランタイム向け最小 Hook 実験 | S02, C02 | group E/F の最小実装と計測 |
| E03 | 比較評価とMVP方式の確定 | X01–X03 | 採用方式、残課題、製品化 Issue を決定 |
| P01 | CLI・標準 Plugin 配布 | E03 | Agent Plugins v1.0.0 準拠 package を検証 |
| P02 | 必要なランタイム Adapter の製品化 | E03 | 実測で必要性が確認された adapter のみ実装 |
| O01 | 永続キャッシュ | 性能不足が確認された場合 | cache key、無効化、意味同値性を検証 |
| O02 | MCP 連携 | 利用価値が確認された場合 | read-only tool surface と権限境界を設計 |
| I01 | 相互運用仕様と上流標準化 | 実装・運用実績確立後 | 実装実績を根拠に提案 |

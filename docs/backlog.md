# Issue Drafts

GitHub repository `rebuildup/skill-graph` にて E01–E03 / S01–S04 / C01–C07 / X01–X04 / P01 / P02 / R01–R03 を登録・完了済み（R03 = Issue #50、v0.1.0 候補確定）。v0.1.0 は `release-0-1-0` @ `80f8424` を実験的初期リリース候補とする。O01 / O02 / I01 は発動条件未達のため deferred を維持する。v0.2.0 の要求は GitHub Issue を正本とし、下表は対応関係である。

## v0.1.0 (release-0-1-0) — 完了

| Issue | Title | Status |
|---|---|---|
| #1–#13 | E01–E03 / S01–S04 / C01–C03 / X01–X03 | closed |
| #28 / #29 | P01 CLI・Plugin 配布 / P02 Claude Code adapter | closed |
| #33 / #34 | R01 リリース候補検証 / X04 mid-work trigger | closed |
| #35 / #39 / #41 / #43 / #46 | C04–C07 識別・重複・hook quoting・stopword 修正、R02 | closed |
| #4 / #50 | S02 ランタイム実現可能性 / R03 v0.1.0 確定 | closed |

## v0.2.0 drafts（Issue 登録済み）

| Issue | Title | Depends | Acceptance summary |
|---|---|---|---|
| #51 | V01 Codex adapter 実現可能性と実装 | — | Codex hooks/payload 実測、adapter、live 検証 |
| #52 | V02 OpenCode adapter 実現可能性と実装 | — | OpenCode hooks/payload 実測、adapter、live 検証 |
| #53 | V03 PATH 不要の導入・配布経路 | #51, #52 | runtime 別の opt-in 導入手順と検証 |
| #54 | V04 実コーパスでの検索性能 | — | 実規模 corpus で p95 目標を再計測 |
| #55 | V05 利用実績の観測（opt-in） | — | 推薦→ロード転換の実測手段と privacy 境界 |
| #56 | V06 project-init との統合 | #55 | skills-lock/requires 連携の設計と検証 |
| #57 | V07 190-session 統計との比較可能な評価設計 | #55 | 対照条件・指標・ラベル規則の Spec |

## v0.3.0 drafts（Spec: docs/specs/v0.3.0.md、Issue #76–#81）

| Workstream | 内容 | Depends |
|---|---|---|
| #76 / W01 | ランタイム別スキル読み込み検出方式の比較調査 | — |
| #77 / W02 | 共通 Skill Usage Event 契約 + observe 拡張 | #76 |
| #78 / W03 | 標準スキル向け多言語推薦改善（keywords 非依存） | — |
| #79 / W04 | 実環境向け導入・更新・削除経路の整備と検証 | — |
| #80 / W05 | project-init / dotfiles / pc-setup 疎結合統合設計 | #77 |
| #81 / W06 | 実運用観測パイロットと継続改善サイクル | #77, #79 |

O01 / O02 / I01 は v0.2.0/v0.3.0 でも deferred を維持（永続キャッシュ・MCP・高度なグラフ処理・上流標準化は効果実証まで着手しない）。

## v0.1.0 draft→issue 対応（履歴）

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
| P01 | CLI・標準 Plugin 配布 | E03 (#13) | Issue #28: Agent Plugins v1.0.0 準拠 package を検証 |
| P02 | 必要なランタイム Adapter の製品化 | E03 (#13), P01 (#28) | Issue #29: 実測で必要性が確認された adapter のみ実装 |
| O01 | 永続キャッシュ | 性能不足が確認された場合 | cache key、無効化、意味同値性を検証 |
| O02 | MCP 連携 | 利用価値が確認された場合 | read-only tool surface と権限境界を設計 |
| I01 | 相互運用仕様と上流標準化 | 実装・運用実績確立後 | 実装実績を根拠に提案 |

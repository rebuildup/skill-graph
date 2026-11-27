# Specs index — 正本と履歴の区別

`docs/specs/` には2種類の文書がある。要求・受入条件の最終正本は常に
GitHub Issue / PR であり、Spec はその設計記録である。

## 継続正本（evergreen）

製品全体に効き続ける契約。変更時は実装と同時に更新する。

| File | 範囲 |
|---|---|
| `product.md` | 製品目的・最終要求・非目標（MVP 記述は v0.1.0〜v0.2.x 時点の履歴的文脈を含む。現行スコープは `v1.0.0.md` を参照） |
| `core-contract.md` | コアエンジン（Discovery / Parser / Resolver / Search / Recommendation）の互換契約 |
| `contracts.md` | `contracts/v1/` JSON Schema への入口 |
| `evaluation.md` | 評価方法の恒常仕様 |
| `standards.md` | 依拠する外部標準の調査記録（証拠であり policy を上書きしない） |

## リリース scoped spec（版ごとの履歴）

`vX.Y.Z*.md` はそのリリースサイクルの要求・境界・判定を記録したもので、
リリース完了後は**履歴として保持し、内容を遡及変更しない**。後続版で
覆された記述が残ることがあるため、現行の要求を判断するときは次の順で
正本を確認する。

1. GitHub Issue / PR（要求・受入条件）
2. `docs/adr/`（確定した設計判断）
3. 最新の版 spec（現行チェーン: 先行完成版 `v0.5.0.md` → 正式完成版 `v1.0.0.md`）
4. evergreen spec（上表）

| File | 対象 |
|---|---|
| `v0.2.0.md` | v0.2.0 リリース仕様 |
| `v0.2.0-evaluation.md` | v0.2.0 評価設計（#57） |
| `v0.2.0-project-init-integration.md` | v0.2.0 project-init 統合境界（#56） |
| `v0.3.0.md` | v0.3.0 リリース仕様 |
| `v0.3.0-integration-boundaries.md` | v0.3.0 4者責務境界（#80） |
| `v0.3.0-observation-pilot.md` | v0.3.0 観測パイロット指標 |
| `v0.4.0-package-manager-distribution.md` | v0.4.0 パッケージ配布仕様 |
| `v0.5.0.md` | v0.5.0 先行完成版仕様（全配布チャネル正式公開 + 実利用検証。現行リリースターゲット） |
| `v1.0.0.md` | v1.0.0 正式完成版仕様（Status: draft。v0.5.0 実利用結果を踏まえ完成を確定） |

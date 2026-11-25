# Evaluation Spec

## 目的

推薦性能と実際の呼び出し性能を分けて評価し、参照グラフが追加価値を持つかを検証する。

## Data sources

1. 既存 session log（190 sessions、2026-11-25 時点の集計値）の元データ所在を調査する。
2. 元ログへアクセスできる場合は read-only で観測事実を抽出する。
3. 集計値しかない場合は制約を記録し、fixture / eval dataset で補う。
4. 実セッションから 30〜50 case を初期候補として抽出する。

## Dataset case

各 case は少なくとも次を持つ。

- id / source / runtime / observed_at
- task context（prompt、working directory、changed files 等の最小表現）
- available skills（name、path、origin、revision/digest）
- expected skills: required / optional / forbidden
- desired load timing
- observed invocations（ある場合）
- labels source（human / rule / inferred）と confidence

実際の呼び出し履歴をそのまま正解ラベルにしない。

## Coverage

- 新規実装、機能追加、命名・構造変更、設計判断、UI 変更、復旧、並列開発、短い局所修正
- 日本語 task と英語 skill description の組み合わせ
- 推薦を拒否すべき case
- 同名・複数配布元・revision 違いの case
- 依存宣言あり / なし、関連リンクのみの case

## Metrics

- Discovery Recall
- Resolution Accuracy
- Recommendation Precision / Recall / Recall@K
- Invocation Recall
- Timeliness
- 再推薦・再読み込み頻度
- 追加 context 消費
- cold / warm 処理時間
- 成果物品質
- project / 既存環境への書き込み
- 未許可 network 通信

性能目標は baseline と初期 benchmark に基づいて確定する。候補値は推薦 p95 100ms 以下、Hook 全体 250ms 以下、推薦最大 3 件だが、実測なしに固定しない。

## Experiment groups

| Group | Intervention |
|---|---|
| A | 現行の skill 選択 |
| B | description のみ改善 |
| C | 開始時の compact な関連 skill 情報 |
| D | 既存 skill 経由の参照解決 |
| E | text search を使う Hook 推薦 |
| F | text search + 参照 graph を使う Hook 推薦 |

同じ model、task、初期状態、利用可能 skill、skill revision を可能な限り固定する。実際の読み込みと成果物品質への効果を区別する。

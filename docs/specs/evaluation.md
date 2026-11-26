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

性能目標は E03（ADR-0002）で確定した: 推薦 warm p95 100ms 以下、Hook 全体 250ms 以下、推薦最大 3 件。C03（500 skills、release で process p95 約 13ms）と X03（hook end-to-end 約 42ms）の実測で予算内を確認済み。実 corpus での再計測は P01/P02 で行う。

## Skill metadata convention（#184 / ADR-0006）

推薦 recall は query と skill metadata の語彙共通部分に強く依存する。
holdout のような description 語彙を避けた自然言語 query では、
cross-lingual（英語 query ↔ 和文 description 等）と言い換えの双方で
語彙ギャップが admission 不成立を起こす。

規約:

- `description` は用途が分かる最小文とし、利用者が実際に使う言語・
  言い換え・略称は frontmatter `keywords` に列挙する。和文 description
  の skill は英語 alias を、英語 description の skill は和文 alias を
  `keywords` に持つことを推奨する。
- `keywords` は具体語を選ぶ。admission は description 2 語以上一致 +
  低 df 語を要求するため、汎用語だけの `keywords` は到達性を改善しない。
- skill 固有の語彙追加の正本はその skill の upstream repo とし、
  skill-graph 側は eval corpus として観測・記録する
  （`docs/specs/v0.3.0-integration-boundaries.md`）。
- admission 閾値の緩和や embedding 導入で recall を改善しない。
  v1.0.0 spec §6 の embedding 不採用判定を維持する。

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

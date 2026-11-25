# #78 W03 — keywords 非依存の多言語推薦改善

Date: 2026-11-26
Issue: #78 (v0.3.0 W03)
Branch: `78-multilingual-recall`

## 目的

`keywords` frontmatter を持たない標準 Agent Skills に対し、日本語クエリ→英語
description / 英語クエリ→日本語 description の双方で推薦が届くようにする。
keywords は任意の精度向上手段であり続け、対応範囲を縮小しない。

## 採用方式: ビルトイン日英グロッサリ

比較した選択肢と判断:

| 方式 | 判断 |
|---|---|
| 形態素解析 (MeCab/Janome等) | 外部依存・辞書保守が重い。オフライン必須の CLI に不適 |
| 文字 n-gram のみ (#61) | CJK↔CJK は届くが CJK↔ASCII の語彙ギャップは埋まらない |
| Embedding / ベクトル検索 | deferred 判断（実測根拠なし）に反する |
| **手入れした日英対訳グロッサリ** | **採用。依存ゼロ・決定的・監査可能・小さい** |

`src/search.rs` の `GLOSS` テーブル（約300エントリ、開発ドメインの
カタカナ外来語と常用漢字熟語）を双方向に使う:

- **query 側**: CJK run から glossary の英語語を term として追加発行
- **skill 側**: description+keywords / body の lowercase 済みテキストに
  glossary 展開を**追記**（原文は verbatim 保持）
- `"pull request"` 句 → `"pr"` の略語展開も追加

## Admission への影響と対策

グロス展開は term を増やすため admission（構造的判定）を歪めうる。実 eval で
2件の false positive を検出し、以下で修正:

1. **同一 CJK 部分文字列の bigram+gloss クラスタが独立証拠に見える問題**
   - `日本語の文章を自然にしたい` → `code-naming`（desc に
     `文章の推敲には使用しない` と否定文で登場）が forbidden hit 化
   - 対策: term に provenance source を付与（gloss 語は由来 CJK key を共有）。
     `description_admitted`（name anchor なし経路）は **distinct source ≥2**
     を要求。1部分文字列由来の証拠は1票に畳まれる
   - `name_plus_description` / `multiple_name_hits` / `unique_name_hit` は
     name が identity anchor なので raw count のまま（`命名` 単独概念でも
     `code-naming` の name hit + desc hit で正しく admit されることを維持）

2. **context_admitted の水増し**
   - `skill selection agent install` + context で `onboarding` が
     `初期化`→`init` gloss 経由で context hit 2件を獲得し forbidden hit 化
   - 対策: context-level admission の hit 計数は**未展開** description のみ
     を対象にする（context は最弱シグナルのため gloss で増幅しない）

## 検証

- 単体テスト5件追加（glossary 発行・provenance source・expand_gloss・
  pull-request 略語）
- eval dataset に4件追加（`skills/` root は keywords 未宣言のみの実コーパス）:
  - `gloss-en-to-jp-desc`: "naming conventions for new functions" → code-naming
  - `gloss-jp-to-en-desc`: "PRをレビューして" → code-review
  - `gloss-en-prose-natural`: "make japanese prose sound natural" → natural-japanese
    （否定文 distractor の code-naming は出ない）
  - `gloss-weather-none`: 無関係な日本語クエリ → empty
- **43 cases: required_recall 1.0 / forbidden_hits 0**（recommended_total は
  67→62 に減少。gloss によるノイズ増ではなく admission 精緻化で減少）
- `mise run check` green

## 実コーパス probe（`skills/` root）

| query | 結果 |
|---|---|
| 新しい関数の命名を考えて | code-naming |
| バグをdebugして | diagnosing-bugs |
| docsの日本語を自然にして | natural-japanese |
| 日本語の文章を自然にしたい | natural-japanese のみ（code-naming FP 解消） |
| review this pull request | code-review |
| pull requestをレビュー | code-review |
| skill selection agent install (context付) | skill-selection のみ（onboarding FP 解消） |
| 今日の天気は / 美味しいラーメン屋 / 猫の写真 | EMPTY |

## 既知の限界（成功扱いしない）

- **語彙ギャップの残存**: `リリースノートを書いて` は empty。対象候補
  （writing-discipline / github-delivery）の name・description いずれにも
  release-note 系語彙がなく、glossary でも埋まらない。stem・同義語辞書・
  embedding が必要な領域であり、軽量方式の境界として記録
- `refactor this function` / `関数をリファクタしたい` は empty — 専用
  refactor skill がコーパスに存在しないため空が正しい挙動
- `PRのレビューをして` / `pull requestをレビュー` で `system-quality` が
  `code-review` と同時推薦されることがある（score 差は小さい）。
  recall は達成、ranking 精度は今後の課題
- GLOSS は手入れ・日英ペア中心。韓国語・中国語等他言語や、ドメイン外
  語彙は対象外
- glossary はマッチテキストを増やすため df を押し下げる方向に働く。
  現行 admission（df==1 経路）では gloss 由来 term 自体が df==1 を
  満たしうるが、distinct-source 要件が単独 substring の自己 admit を防ぐ

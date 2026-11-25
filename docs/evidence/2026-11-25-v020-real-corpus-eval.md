# v0.2.0 #54 — 実コーパス検索性能・品質評価

Date: 2026-11-25
Issue: #54
Binary: `target/release/skill-graph` (release-0-2-0 @ post-v0.1.0-sync, engine identical to v0.1.0 `80f8424`)

## Corpus

実運用に近い構成として、以下3 root を同時 discovery（計56 copy / 37 unique skill names）:

- `~/.dotfiles/.agents/skills` — project-init 系 20 skills
- `~/.dotfiles/.claude/skills` — 同上のほぼ複製（同名・別 root の multi-copy ケース）
- `skill-graph/.claude/skills` — この repo の project-local 17 skills

## Latency（オンメモリ、cache なし）

| 操作 | 実測 |
|---|---|
| `recommend` cold（プロセス起動含む） | ~9 ms |
| `recommend` warm ×30 平均 | ~9 ms/回 |
| `discover` | ~4 ms |

v0.1.0 性能目標（recommend p95 < 100 ms、hook < 250 ms）に対し、実コーパス規模でも 1 桁 ms で余裕。**永続キャッシュ不要**の裏付けが強まった。

## Probe 結果（抜粋）

正方向:

- `diagnose the failing test` → `diagnosing-bugs` ✓
- `commit message を書いて` → `writing-discipline` ✓（日英混在でも英単語が効く）
- 無関係入力 `ありがとう` / `ok` / `?` / `17 * 23` → すべて空出力 ✓（C07 修正の実コーパス確認）

問題（後述 F1–F4）:

- 日本語のみのクエリ 8 件中 7 件が空（`PRのレビューをして`, `テストが落ちてる、原因を診断して`, `新しい関数の命名を考えて`, `Herdrでworkerを並列に動かしたい`, `バグをdebugして`, `releaseのブランチを切って`, `docsの日本語を自然にして`, `worktreeを作って並行開発したい`, `secretsを安全に扱いたい`, `リリースノートを書いて`, `依存関係の脆弱性を確認`, `テストを書く`）
- `code review` / `PR review` / `review this pull request` → **空**（`code-review` skill が存在するのに不検出）
- `refactor this function` → 空
- `diagnose the failing test` は一致したが、より小さい corpus では不一致だった（df 依存の脆弱性、F4）

## Findings

### F1: 日本語セグメンテーション欠如（critical）

`terms()` は空白分割のみ。日本語は空白を含まないためクエリ全体が 1 トークンになり、description への部分文字列一致が成立しない。skill description 自体が日本語（project-init 系 20 skills）であっても、クエリ側が分かち書きされない限り recall ≈ 0。

実運用のプロンプトは日本語混在が普通であり、**現状の日本語対応は実質英語クエリ依存**。v0.2.0 で tokenizer（文字種境界 or n-gram）を導入する根拠が確定した。

### F2: 高 df 語による admission 失敗（high）

`code review` / `PR review` が空。原因: `code`, `review`, `pull`, `request` 等の語が多くの skill の name/description に出現し df > 1 となり、`unique_name_hit`（df==1 要件）と description hit 閾値を両方満たせない。fixture corpus（36 case、少数 skill）では顕在化しなかった、実規模でのみ出る false negative。

### F3: 英語パラフレーズ gap（medium）

`review this pull request` は `code-review` の description が "a PR" 表現で "pull request" を含まないため hit せず。F2 と複合。stemming/同義語なしの限界として記録。

### F4: df 依存の非決定性（medium）

`diagnose the failing test` は 56-copy corpus で一致、project 17-skill corpus では不一致。corpus 構成で結果が変わる = admission が絶対的でなく相対的。eval dataset の corpus 固定性に注意が必要。

### F5: 同名 multi-copy の扱い（確認済み・問題なし）

`.agents`/`.claude` の同名複製は name で group 化され、recommend 出力は unique name 単位。`requires` 解決・collision 表示は v0.1.0 C04/C05 契約通り。

## 結論と次アクション

- 性能: 目標を大幅にクリア。cache/MCP は引き続き deferred で妥当。
- 品質: **日本語 recall と高 df 語の admission が実運用ブロッカー級**。v0.2.0 のスコープとして tokenizer 導入（F1）と admission/df 見直し（F2–F4）を Issue 化する。
- fixture eval は日本語ケースと高 df ケースを追加しないと回帰を検出できない。

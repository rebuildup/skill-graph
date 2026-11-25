# v0.2.0 #61/#62 — tokenizer・keywords・admission 再設計の検証

Date: 2026-11-25
Issues: #61 (日本語), #62 (admission/df)

## 変更内容

1. **Tokenizer（#61）**: `terms()` を script-run 分割に変更。ASCII 英数字は
   単語のまま、CJK(仮名・漢字・ハングル)ランは文字 bigram に展開。空白なし
   日本語文・日英混在トークン（`Herdrでworkerを並列に` → `herdr`,`worker`,
   CJK bigrams）の両方を扱う。形態素解析・外部依存なし。
2. **`keywords` frontmatter（#61 言語横断）**: Skill が `keywords: [...]` を
   宣言すると description 相当の match text に加わる。契約
   （SkillSummary.description）には含めず、opt-in の発見手がかりに限定。
   上流管理 skill を編集せずに日英両対応する道を提供。
3. **Admission（#62）**: `directly_admitted` を再設計。
   - name hits ≥2 → df 不問で admit（name は identity、複数 term 一致は特異的）
   - name hit ≥1 かつ description hits ≥2 → admit（`PR review` → `code-review`）
   - 従来の df==1 規則（単一 name hit / description の低 df hit / context≥2）は維持
   - ranking（score順）と admission を分離した構造は不変

## 評価結果

### fixture eval（39 cases、kappa 追加3件を含む）

- required_recall 1.0 (35/35)、forbidden_hits 0、elapsed ~40ms
- kappa-japanese（日本語クエリ→英語 desc+keywords）PASS、kappa-english PASS、
  kappa-negative（無関係クエリで keywords が誤 admit しない）PASS

### 実コーパス probe（56 copy / 37 unique names）

改善: `code review`/`PR review`→code-review、`Herdrでworkerを並列に`→
herdr-runtime、`worktreeを作って`→worktree-workflow、`secretsを安全に`→
secrets-management、`releaseのブランチを切って`→linear-release-control、
`依存関係の脆弱性を確認`→security-maintenance（+2件の周辺候補）。

残る miss（既知の限界として記録）:
- **日本語クエリ→キーワード未宣言の英語 desc skill**: `命名`→code-naming、
  `docsの日本語を自然に`→natural-japanese 等。言語横断は keywords 宣言が
  前提。upstream 管理 skill への適用は upstream 側の対応が必要。
- **パラフレーズ**: `review this pull request`（desc が "a PR" 表現）。
  軽量方式の限界として記録。stemming/同義語は引き続き deferred。
- `バグをdebugして`→診断系は英語 desc のため miss（`debug` は desc にあるが
  単独 desc hit では admit しない保守的設計の結果）。

### corpus 安定性

- 無関係 skill 3件追加で `release notes changelog`→gamma-skill 不変
- 高頻度語混入の noise skill でも真の一致が上位を維持（code-review > 追加skill）
- 同名 multi-copy の name-grouping/collision 契約は不変（C04/C05 テスト通過）

### 性能

recommend ~9ms/プロセス（実コーパス）、warm recommend p95 <0.1ms —
目標 p95<100ms を維持。bigram 展開による term 数増加の影響は実測で無視可能。

## 残課題

- 実コーパス skill への `keywords` 付与は upstream（project-init/dotfiles）
  側の対応事項。skill-graph は仕組みのみ提供。
- CJK bigram は文字 2-gram のみ。複合語の分割精度は限定的（例: 「命名」→
  「名」「命」ではなく「命名」bigram 1個で desc 中の「命名」を拾う設計で
  実用上十分と判断）。

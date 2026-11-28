# v0.5.0 skill 発火漏れ — 介入方式の再検討と実験設計

Date: 2026-11-28
関連: #308, #318, #319, `2026-11-28-v050-usage-effect-ab.md`

## 事実（検証済み）

A/B 実験で実利用効果が確認できなかった原因を、コードと
ランタイム仕様から切り分けた。

### OpenCode の skill surface は包括的（ソース確認）

`opencode/src/skill/index.ts`（anomalyco/opencode）で確認:

- discovery は `~/.config/opencode/skills`、`~/.claude/skills`、
  `~/.agents/skills`（global）+ project 側 `.opencode/`・`.claude/`・
  `.agents/` を cwd から worktree まで遡って走査 + config の
  `skills.paths`（任意ディレクトリ）と `skills.urls`（remote pull）
- **truncate なし**: `available()` は全 skill を name+description 付きで
  常時 context へ列挙する。corpus 規模による native 側の打ち切りはない
- 標準的な配置では「skill-graph が推薦しなくても model は全候補を
  見ている」= 発火漏れの構造的余地が小さい。A/B の null result は
  plugin の欠陥ではなくこの構造の反映

### 残る仮説（効果が出得る条件）

| # | 仮説 | 検証方法 | 状態 |
|---|---|---|---|
| H1 | 大規模 corpus（百件級）で flat 列挙による選択精度が落ち、top-3 注入が相対優位になる | 150+ skills で A/B 再実験 | **未検証（quota 待ち）** |
| H2 | runtime が skill を surface しない環境（hooks のみ存在・native skill 機構なし）でのみ効果が出る | Codex exec / Claude Code での A/B | **未検証（両者 quota 切れ）** |
| H3 | 標準外 root（`SKILL_GRAPH_ROOTS` の custom path）にある skill は runtime に不可視で、skill-graph の集約だけが届けられる | custom root のみの corpus で A/B | 未検証（H2 と同じ block） |
| H4 | prompt 時点以外（作業中盤で relevance が出る）への介入が必要 | mid-work trigger 実験 | 未検証 |

### 介入方式の比較（現時点の評価）

| 方式 | 利点 | 欠点 | 判定 |
|---|---|---|---|
| A. 現行 push 注入（prompt→top3→additionalContext） | 確実に届く・実装済み | native 列挙と重複、context を消費 | H1/H2 で効果が出なければ価値は delivery 保証+観測に限定 |
| B. 生成 index（AGENTS.md/CLAUDE.md へ curated 一覧を書く） | hook 不要・全 runtime に効く | staleness、双方向編集の衝突、生成物の維持 | 代替案として保持。skill-graph の推薦を静的化するだけで新規性は薄い |
| C. pull（MCP/CLI を agent が呼ぶ） | 非侵襲 | model の自発性依存 = push より弱い | 既存 `mcp` で提供済み。主経路にはしない |
| D. native surface の絞り込み（config で skill 源を限定し skill-graph 注入に一本化） | context 節約と選択精度の両立 | runtime 標準機能を迂回する構成で保守性が悪い | 非推奨（利用側の設定を侵襲しすぎる） |

## ブロッカー

- OpenCode 無料 model（`opencode-go/*-free`）: 全 model で
  `Model unavailable`（quota 枯渇）
- Codex（ChatGPT account）: usage limit、~Nov 30 まで回復見込みなし
- Claude Code: 未 login

H1–H3 の実験はいずれも model 実行が前提のため、quota 回復まで
live 検証は不能。検証ハーネスは準備済み:

- `/tmp/sg-exp/gen-corpus.py` — 150+ distractor skill 生成
  （`/tmp/sg-exp/corpus-large` に生成済み）
- `/tmp/sg-exp/run-exp.sh` + `tasks.txt` — 既存 A/B runner を
  corpus 差し替えで再利用可能
- 集計 script（`count2.py` 系）— `tool=="skill"` / SKILL.md read
  を transcript JSONL から機械集計

## 暫定結論（実験未了のため確定ではない）

OpenCode のような native skill surface が包括的な runtime では、
skill-graph の push 注入が invocation を増やす証拠は現時点でない。
製品価値の成立には H1（大規模 corpus）または H2（surface の弱い
runtime）での正の効果が必要。どちらも否なら、価値の中核は
「発火させる」ではなく「届ける保証 + 跨 runtime の観測性」へ
再定義すべきであり、その場合 README の訴求も変わる。
公開判断は H1–H3 の実測が出るまで保留が妥当。

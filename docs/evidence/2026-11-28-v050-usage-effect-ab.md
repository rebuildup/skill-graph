# v0.5.0 実利用効果 — OpenCode A/B 実測記録

Date: 2026-11-28
Issue: #308（#225 G05 — オフライン推薦精度と実利用効果の区別）
関連: #318（README の実利用効果の提示）、`docs/specs/v0.3.0-observation-pilot.md`

## 目的

「導入すると実際に必要なスキルが使われるようになるか」を、オフライン
評価（recall@3 52/52）とは別の実ランタイム A/B 実験で測定し、
観測定義・条件・限界を記録する。公開前に実施可能な初期比較実験で
あり、pilot spec の長期観測（群あたり50 sessions/2週間）を前提に
しない。

## 観測定義

| 指標 | 定義（本実験での operationalization） |
|---|---|
| skill invocation | OpenCode transcript（`opencode run --format json`）中の `tool_use` で `tool=="skill"` の呼出。`SKILL.md` の直接 read は本ランタイムでは発生しなかった（skill tool 経由のみ） |
| correct-task load | そのタスクの ground-truth skill が少なくとも1回 invoke された session の割合 |
| invocation/session | skill tool 呼出総数 / session 数 |
| unnecessary load | invoke されたが推薦集合に含まれなかった skill（treat 群のみ集計可能） |
| delivery | plugin event log の `kind=recommendation, emitted=true`（recommend が model に届いた） |
| conversion | emitted recommendation に含まれた skill が同 session で invoke された割合 |

推薦・注入・mention を load と混同しない。ground truth はタスク設計者
（本記録の著者）が各タスクに対応付けた skill 名。

## 実験条件

- Runtime: OpenCode（`opencode run`、非対話）、model `opencode-go/hy4-flash-free`（無料枠、両群同一）
- Corpus A（round 1）: 実務 skill 8件 + test-skill 1件 = 9件
- Corpus B（round 2/3）: Corpus A に confusable な distractor skill 24件を追加 = 33件
- Tasks: 8件（API エラー統一・SQL migration・shell 堅牢化・CLI 引数・
  pytest・構造化ログ・設定検証・React hooks）。Corpus B では
  `argument-parsing`/`cli-design`/`unit-test-patterns` 等、表層
  キーワードが競合する distractor を配置
- 群: treat = `opencode.json` に skill-graph plugin（開発版
  `src/assets/opencode-plugin.ts`、`SKILL_GRAPH_BIN`=release build）を
  opt-in。ctrl = plugin なし。同一 HOME 分離（`/tmp/sg-exp/home-{treat,ctrl}`）、
  同一 project `/tmp/sg-exp/proj`（各 run 前に `git checkout .` で初期化）
- 観測: transcript JSONL を `tool=="skill"` で集計。treat 群の
  delivery/conversion は `~/.local/state/skill-graph/events.jsonl` で確認
- 再現物: `/tmp/sg-exp/`（run-exp.sh, tasks.txt, corpus, 生 transcript）

## 結果

### Round 1（9 skills、n=8 sessions/群）

| 群 | skill invocations | correct-task load | 備考 |
|---|---|---|---|
| treat | 8 | 8/8 | 全件正解 skill を invoke |
| ctrl | 8 | 8/8 | OpenCode が skill を native に
surface するため小 corpus では差が出ない |

### Round 2（33 skills、n=8 sessions/群）

| 群 | skill invocations | correct-task load | zero-load | wrong-only |
|---|---|---|---|---|
| treat | 13 (1.63/session) | 7/8 | 0 | 1（task4: `argument-parsing` のみ） |
| ctrl | 10 (1.25/session) | 6/8 | 1（task4: skill 未使用） | 1（task5: `unit-test-patterns`） |

- treat の delivery: 8 session 中 7 session で `emitted=true` の推薦を
  確認。task4 は `emitted=false, reason=no_recommendations`
- conversion: emitted recommendation に正解 skill が含まれた 7 session
  全てで正解 skill が invoke された（7/7）
- treat の不要 load: task1 `rest-api-design`、task6 `structured-logging`/
  `json-logging`（いずれも推薦集合外または推薦の副次候補）

### Round 3（33 skills、replication、n=8 sessions/群）

| 群 | skill invocations | correct-task load | 備考 |
|---|---|---|---|
| treat | 10 (1.25/session) | 6/8 | task4 `argument-parsing` のみ、task5 `unit-test-patterns`（誤） |
| ctrl | 12 (1.50/session) | 6/8 | task4 `argument-parsing` のみ、task5 `unit-test-patterns`（誤） |

round 2 の差は replication で再現しなかった。

## Pooled 結果（round 1–3、n=24 sessions/群）

| 指標 | treat | ctrl |
|---|---|---|
| skill invocations 合計 | 31 (1.29/session) | 30 (1.25/session) |
| correct-task load | 21/24 (87.5%) | 20/24 (83.3%) |

**結論: 本予備実験では skill-graph による実利用効果の増加を
統計的・実質的に確認できなかった。** invocation 数差 +3.3%、
correct-task load 差 +4.2pt は n=24/群・単一 model・単一
runtime ではノイズの範囲。round 2 で観測した +30% は round 3 で
反転し、単発の差を効果と見なすことはできない。

## 正直な解釈と限界

- **統計的有意性はない**（どころか点推定もほぼゼロ）。
  n=24/群・単一 model・単一 runtime・単一 corpus 設計の初期実験で
  あり、pilot spec の長期 A/B（交絡対策・層別・信頼区間）が
  引き続き必要。公開後の実利用観測（#130/#207）で再検証する。
- OpenCode は skill を native に surface するため、小さい corpus
  では baseline が既に高い（round 1 で両群 8/8）。skill-graph の
  限界効用が出る条件（より大きい corpus・description が弱い
  skill・native surface のない runtime）は本実験の範囲外。
- 観測されたのは「推薦→invoke の conversion は機能している」
  こと（round 2: emitted 推薦の正解 skill が 7/7 session で
  invoke された）と「engine の推薦 recall が corpus 規模で
  感度を持つ」ことであり、対照群に対する呼び出し数の優位ではない。
- **発見した engine 側の問題**: Corpus B で task4
  （`tools/report.py` + `--verbose`/`--output` flags）に対し
  `recommend` が `[]` を返した（`no_recommendations`、round 2・3
  の両方で再現）。同一 prompt が Corpus A では `cli-arg-parsing`
  を推薦していた。corpus 拡大で admission が推薦を棄権する方向に
  倒れる recall 感度がある。
  検証: `cd /tmp/sg-exp/proj && HOME=/tmp/sg-exp/home-treat
  skill-graph recommend "<task4 prompt>"` は `[]`、
  `search "command line flags arguments"` は
  `argument-parsing`/`cli-arg-parsing` を返す。plugin は engine
  出力を忠実に転送しており plugin 側の欠陥ではない。別 Issue 化を
  検討。
- `mention`・推薦・注入は load に計上していない。`emitted=true` は
  delivery であり invocation ではない。
- **発見した観測欠陥（修正済み）**: OpenCode の native `skill` tool は
  input に `{"id": "<name>"}` を取るが、`classify_tool_access_all` は
  Claude Code 形式の `skill`/`name`/`skill_name` key のみを読んでいた
  ため、treat 群の 31 回の skill invoke に対し `skill_load` event が
  ほぼ発行されていなかった（event log 中 4 件のみ、かつ別要因）。
  `id` を fallback chain に追加して修正（`src/events.rs`、
  `native_skill_tool_is_high_confidence` テストで `id` 形を回帰
  カバー）。修正後 `detect` が `{"id":"api-error-handling"}` から
  `native_invocation`/`high` の skill_load を返すことを実測確認。
  これにより OpenCode 経路の conversion 計測が可能になる
  （過去分の event log は遡及修正されない）。

## #318 への供給値

README の hero 指標は **実測された正直な比較**を提示する:

- pooled skill invocations/session: treat 1.29 vs ctrl 1.25（+3.3%、有意差なし）
- pooled correct-task load: 87.5% vs 83.3%（+4.2pt、n=24 で非有意）
- delivery: treat 群で emitted recommendation は全件 model の
  文脈へ到達（`emitted=true` のみが delivery に計上される契約）

改善が統計的に支持されないため、README は増加率を効果として
主張せず、「推薦が必ず文脈へ届く」delivery の保証と継続観測の
枠組みを価値として提示する構成とした。

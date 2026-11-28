# v0.5.0 実利用効果 — Devin A/B 実測記録（H1/H3 検証）

Date: 2026-11-29
Issue: #308, #319, #318
関連: `2026-11-28-v050-usage-effect-ab.md`（OpenCode null result）、
`2026-11-28-v050-skill-surface-analysis.md`（surface 分析と仮説 H1–H4）

## 目的

surface 分析で立てた仮説のうち、quota 非依存で検証可能なものを
Devin CLI を実験ランタイムとして実測する。

- **H1**: 大規模 corpus（157 skills）で runtime の flat 列挙による
  選択精度が落ち、top-k 注入が相対優位になるか
- **H3**: runtime が surface しない root に置かれた skill を
  skill-graph の推薦注入が届かせられるか

## 実験条件

- Runtime: Devin CLI `devin -p`（非対話）、model は両群とも
  SWE-2 Flash Medium で同一。`--export` の JSON から
  `skill` tool 呼出と `*/SKILL.md` ファイル read の両方を
  load として計測
- 実験者と被験エージェントは別プロセス。treat 群のみ prompt 先頭に
  `skill-graph: skills relevant to this task — <name (path)>` を付与
  （H3 では path 付き。skillbank 配置のため file read が唯一の
  到達経路）
- Tasks: 8件（OpenCode 実験と同一の ground truth 対応）
- Corpus 構成:
  - Phase A: `.agents/skills` に実務 skill 8件のみ
  - Phase B (H1): 149件の confusable distractor を追加し 157件
  - Phase C (H3): 実務 skill 8件を `/tmp/sg-exp/skillbank/` へ移動
    （Devin の native scan 対象外）。`.agents/skills` には
    distractor 149件のみ残存。`SKILL_GRAPH_ROOTS` で
    `skillbank + .agents/skills` を推薦対象に指定

## 結果

### Phase A（8 skills、native surface あり）

| 群 | skill load 数 | correct-task load |
|---|---|---|
| treat | 8 | 8/8 |
| ctrl  | 8 | 8/8 |

null。小 corpus では native surface が飽和しており注入の余地なし。

### Phase B / H1（157 skills、native surface あり）

| 群 | skill load 数 | correct-task load |
|---|---|---|
| treat | 11 | 8/8 |
| ctrl  | 11 | 8/8 |

null。157件への拡大でも SWE-2 Flash Medium は flat 列挙から
正しい skill を全タスクで選択した。両群とも distractor
（`experiment-report-reference` 等）の混入 load が各1件あり、
推薦側の distractor 混入（task2/3/6/7 の top-3 に非該当 skill）
と同程度で、相対優位は観測されなかった。

### Phase C / H3（実務 skill は runtime 不可視 root のみ、round 1）

| 群 | 正解 skill load | 備考 |
|---|---|---|
| treat | **7/8** | 唯一の失敗は task4: `recommend` が
  `cli-arg-parsing` を top-3 に返さず `experiment-report-reference`
  のみ返却（#319 の recall 欠陥がそのまま製品指標を直撃） |
| ctrl  | **0/8** | distractor のみ load。実務 skill には
  一切到達不能 |

**正の効果を確認**: runtime が surface しない root に skill が
ある条件では、注入が唯一の到達経路であり 7/8 vs 0/8 と明確。
失敗1件は介入方式ではなく推薦 recall（#319）の問題。

## 追補: round 2/3（n=24/群）と #319 修正の end-to-end 検証

Phase C を round 2・3 で反復し、途中で #319（recommend recall）を
修正した release build を treat 側に適用した。

| round | recommend binary | treat correct | ctrl correct |
|---|---|---|---|
| r1 | 修正前 | 7/8（task4 失敗 = #319） | 0/8 |
| r2 | 修正後 | 8/8 | 0/8 |
| r3 | 修正後 | 8/8 | 0/8 |
| **pooled** | — | **23/24 (95.8%)** | **0/24 (0%)** |

- treat の skill load 総数 45、ctrl は 12（全て distractor で
  正解 skill への到達は 0）
- r1 task4 の失敗は #319 修正後の r2/r3 で両方正解に転じ、
  recall 欠陥が製品指標を直撃していた因果が end-to-end で確認
  できた
- 23/24 vs 0/24 は二項検定で p < 1e-6 オーダー（Fisher exact
  相当）。ただし条件付き効果である点は変わらない

## 解釈

- 「スキルが入っているのに使われない」問題に対する skill-graph の
  実利用効果は **条件付きで実在する**: 効果が出るのは
  runtime が native に surface しない skill 集合に対してであり、
  runtime が既に全候補を列挙する環境（OpenCode・Devin、
  8〜157件の範囲で検証）では統計的に検出可能な増加はなかった。
- したがって製品価値の中核的主張は「runtime 非依存の統一
  discovery + 推薦 delivery」であり、native surface が完全な
  runtime では increment は観測されない。これは製品の欠陥では
  なく、介入が効く適用領域の境界である。
- H2（native surface の弱い runtime: Codex exec 等）は
  Agent Router の credentialed provider が Infisical の対話
  login を要求し自律実行不可、ovhcloud anonymous は 2 req/min で
  agentic task に不適のため未検証のまま残る。

## 限界

- 単一 model（SWE-2 Flash Medium）、単一 runtime（Devin）、
  各 phase n=8 task × 1 round。統計的検出力は限定的
- treat 注入は prompt prefix による模擬（実 hook/plugin 経路
  ではない）。delivery 機構自体は OpenCode 実験で別途確認済み
- H3 の ctrl は「skill が存在するが runtime から不可視」という
  条件設定であり、実務上その配置がどの程度一般的かは本実験の
  範囲外
- task4 の失敗は #319（corpus 拡大時の recommend recall 低下）
  に起因し、介入方式の評価と推薦品質の評価が交差している

## 成果物

- Runner: `/tmp/sg-exp/run-devin-exp.sh`, `run-devin-large.sh`,
  `run-devin-h3.sh`（ephemeral）
- Export: `/tmp/sg-exp/devin-out{,-large,-h3}/*.md`
- 集計: `count-devin2.py` 系（skill tool + SKILL.md read の
  union を load として計数）

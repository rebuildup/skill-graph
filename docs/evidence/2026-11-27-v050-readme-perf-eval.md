# v0.5.0 README/SVG 用パフォーマンス・評価エビデンス

Issue: #213。README の性能・精度・効率セクションと `assets/readme/*.svg` の
データソース。全て release-0-5-0 最終候補 @ `074e6ab` (#225 第7回レビュー対応後、
PR merge 前の ticket head) の release build で実測した。

## 測定環境

- OS: Linux (WSL2/Ubuntu)、kernel x86_64
- CPU: このマシンのシングルスレッド性能 (WSL2、他負荷なし)
- Rust: rustc 1.100.0、`cargo build --release`
- Binary: `target/release/skill-graph` (version 0.5.0)
- 測定日: 2026-11-27（初回）、2026-11-28（最終候補で再測定）

## 再現コマンド

```bash
cargo build --release
./target/release/skill-graph perf                 # 合成コーパス perf (JSON)
./target/release/skill-graph bench evals/dataset/cases.jsonl   # 推薦評価 (JSON)
./target/release/skill-graph discover --root .agents/skills   # 実コーパス
/usr/bin/time -v ./target/release/skill-graph discover --root <corpus>
./target/release/skill-graph recommend --root <corpus> "<query>"
echo '{"hook_event_name":"UserPromptSubmit","prompt":"test"}' \
  | ./target/release/skill-graph hook claude-code prompt
```

## 1. 合成コーパス・スケーラビリティ (`skill-graph perf`)

`perf` は N 個の合成 SKILL.md を生成し、cold discovery+build、warm recommend
(p50/p95/max)、process 全体 (p50/p95/max、**30 samples**) を計測する。
warm iterations=30 (既定)。process samples は #225 r5 で 5→30 に増量:
n=5 では p95 の順位統計量が max に退化していたため。単位 ms。

| skills | cold_discover | cold_build | cold_total | warm p50 | warm p95 | warm max | process p50 | process p95 | process max |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 50  | 0.77 | 0.17 | 0.94  | 0.084 | 0.119 | 0.137 | 1.81 | 2.13  | 2.15  |
| 100 | 1.40 | 0.34 | 1.74  | 0.169 | 0.307 | 0.400 | 3.02 | 3.70  | 4.52  |
| 200 | 3.25 | 0.69 | 3.94  | 0.342 | 0.435 | 0.545 | 5.25 | 6.20  | 6.99  |
| 500 | 7.28 | 1.77 | 9.06  | 0.828 | 1.041 | 1.402 | 12.82| 14.56 | 14.82 |

filesystem cache 状態で cold_total は ±20% 程度変動する
(500 skills で ~8–11ms)。表は最終候補での実測値 (1 run)。

読み方:

- **cold_total はほぼ線形** (50→500 で約 9 倍)。500 skills でも ~9ms。
- **warm recommend はサブミリ秒〜1ms 未満** — グラフ構築は cold のみ、
  推薦は常駐不要の都度プロセスで p95 ~1.0ms (500 skills)。
- **process p95 (CLI spawn 込み) は 500 skills で ~15ms** (30 samples、
  max 14.8ms) — hook として実用上無視できるレイテンシ。

## 2. 推薦精度 (`skill-graph bench evals/dataset/cases.jsonl`)

合成評価データセット 58 cases (required 52 / optional 31 / forbidden は
expected set に含まれない)。`elapsed_ms: 28` で全件評価。
最終候補での再測定: recall/forbidden は不変、recommended_total は
resolver mention-cap 修正 (#225) による推薦集合の変化を反映 (87→88)。

| 指標 | 値 |
|---|---|
| required_recall | **1.000** (52/52) |
| required_recall@3 | **1.000** (52/52) |
| required_recall@5 | 1.000 |
| forbidden_hits | **0** |
| precision_required | 0.591 |
| precision_acceptable | 0.773 |
| recommended_total | 88 (max=3/case) |
| empty_recommendations | 6 |

注意: これは**合成コーパスに対するオフライン評価**であり、実セッションでの
skill_load→conversion とは別物。実利用効果は `observe --summary` の funnel
指標 (delivery/conversion/timeliness) で別途測定する (下記 §5)。

## 3. 実コーパスでの計測

RSS は `/usr/bin/time -v` の "Maximum resident set size" を**各点 20 回
計測し最大値**を採用（`assets/readme/data/memory.json` が単一データ
ソース — 図・README・本表は全てそこから導出される）。単位 MB
(1024^2 bytes)。

### 3a. このリポジトリの開発スキル群 (`.agents/skills`, 24 skills)

- `discover`: **Maximum RSS 6.6 MB** (20 runs、観測範囲 6.4–6.6)
- 実際の第三者製 Agent Skills (aude-skills / writing-skills /
  mattpocock/skills / design-skills 由来、keywords 非依存が大半) を含む。

### 3b. design-skills リポジトリ (`skills/`, 88 skills)

- `discover`: **Maximum RSS 7.4 MB** (20 runs、観測範囲 7.0–7.4)
- 公開されている実在の大規模スキルコーパスでの動作確認。

### 3c. 合成 500 skills コーパス (`/tmp/corpus500`、perf と同規模)

- `discover`: **Maximum RSS 7.0 MB** (20 runs、観測範囲 6.6–7.0)
- `recommend --root /tmp/corpus500 "performance testing"`: **Maximum
  RSS 7.2 MB** (20 runs)

メモリはコーパス 24→88→500 skills で 6.6→7.4→7.0 MB と**ほぼ一定**
(パース済み構造が小さいため)。常駐プロセスがないので、コストは呼び出し
ごとのプロセス起動 + 都度 discovery に限定される。

## 4. Hook end-to-end

```
echo '{"hook_event_name":"UserPromptSubmit","prompt":"test performance"}' \
  | ./target/release/skill-graph hook claude-code prompt
```

- wall <10ms、**Maximum RSS 6.9 MB** (20 runs、観測範囲 6.5–6.9)
- fail-open 設計: 常に exit 0、hook 失敗がエージェントを阻害しない
- イベントは `~/.local/state/skill-graph/events.jsonl` に append
  (prompt 本文は SHA-256 hash のみ — contracts.md の privacy 契約)

## 5. 実セッション観測 (`observe --summary`)

このマシンの実 event log (adapter 有効化済み claude-code/codex/opencode
経由の hook/plugin イベント) から:

- sessions: 7、recommendation delivery rate: 75%
- delivery→conversion funnel は実利用で蓄積中 (サンプル数は少ない)

## 6. アーキテクチャ上の帰結 (README 記載の根拠)

- **永続キャッシュ・常駐サービス不要**: グラフは呼び出しごとにオンメモリ構築。
  cold でも 500 skills で ~9ms、warm recommend は ~1ms。
  デーモン・DB・index ファイルの運用コストがゼロ。
- **プロセスモデル**: 各呼び出しは独立プロセス。state は event log
  (append-only JSONL) のみで、クラッシュしても推薦経路に影響しない。
- **スケーラビリティ**: discovery が支配的コストだが線形かつ小さい。
  500 skills ≈ 実用上の上限想定を大きく超える規模でも ~9ms。

## 7. SVG データソース

`assets/readme/` の SVG は上記 §1–§4 の実測値から生成する。
データファイルは `assets/readme/data/` に一元化:
`perf-500.json`（`skill-graph perf` 出力）、`bench-eval.json`
（`skill-graph bench` 出力）、`memory.json`（RSS 計測条件・
サンプル数・各点の値）。生成スクリプト:
`scripts/gen-readme-assets.py`。手編集ではなくスクリプト再実行で
再生成可能。図・README・本書の数値は全てこれら JSON が正本。

# v0.5.0 README/SVG 用パフォーマンス・評価エビデンス

Issue: #213。README の性能・精度・効率セクションと `assets/readme/*.svg` の
データソース。

## 測定対象 SHA との対応（#225 G01）

- 初回測定: `e9e1fc7`（第8回レビュー対応後）。
- **現行測定: release build of `d69468a` + 本ラウンド未コミット差分**
  （B07 spec 適合・F クラスタの script/workflow 変更を含む working tree。
  `src/`・`Cargo.*` の binary 影響入力は freeze 対象 commit と同一内容で
  あることを commit 時に確認する）。
- session ID identity domain・boundJsonValue・MCP envelope・funnel 再構成・
  adapter/discovery 強化など第9–11回対応で入った変更は本測定に**含まれる**
  （e9e1fc7 の値を継承しない）。
- freeze 時の最終候補では `check-eval-gate.sh`（recall/forbidden 閾値）を
  artifact-verify.yml が機械検査する（#225 G04）。perf/RSS の数値は
  README/SVG の表示用であり release gate ではない — 変動は本書と
  `assets/readme/data/` の再生成で追跡する。

## 測定環境（#225 G08）

- OS: Linux (WSL2/Ubuntu)、kernel x86_64
- CPU: 13th Gen Intel Core i7-13700H（8 cores）、他負荷なしの quiet 環境
- Rust: rustc 1.100.0、`cargo build --release --locked`
- Binary: `target/release/skill-graph` (version 0.5.0)
- 測定日: 2026-11-28（`d69468a`+差分で再測定）
- コーパス revision:
  - `.agents/skills`（24 skills）: 本 repo `d69468a` 時点の tree
  - design-skills `skills/`（88 skills）: `/home/basic/work/design-skills`
    @ `7519d3246c6edb6201ad8f6f28195ec20fe614d1`
  - 合成コーパス: `/tmp/corpus500`（uniform ~240B SKILL.md テンプレート、
    skill 数のみ可変の管理系列 `/tmp/sg-rss-{50,100,200,500}` を派生）
- イベントログ既定パス: `$XDG_STATE_HOME/skill-graph/events.jsonl`、
  無ければ `~/.local/state/skill-graph/events.jsonl`（HOME も不明な場合
  のみ `${TMPDIR}/skill-graph/events.jsonl`）— hook.rs `per_user_state_root`
  と一致（#225 D06/G08）

## 再現コマンド

```bash
cargo build --release --locked
./target/release/skill-graph perf                 # 合成コーパス perf (JSON)
./target/release/skill-graph bench evals/dataset/cases.jsonl   # 推薦評価 (JSON)
./target/release/skill-graph discover --root .agents/skills   # 実コーパス
/usr/bin/time -v ./target/release/skill-graph discover --root <corpus>
./target/release/skill-graph recommend --root <corpus> "<query>"
SKILL_GRAPH_ROOTS=$PWD/.agents/skills \
  sh -c 'echo '"'"'{"hook_event_name":"UserPromptSubmit","session_id":"s1","prompt":"please review this pull request carefully"}'"'"' \
    | ./target/release/skill-graph hook claude-code prompt'
scripts/check-eval-gate.sh ./target/release/skill-graph   # freeze 閾値 gate
```

## 1. 合成コーパス・スケーラビリティ (`skill-graph perf`)

`perf` は N 個の合成 SKILL.md を生成し、cold discovery+build、warm recommend
(p50/p95/max)、process 全体 (p50/p95/max、**30 samples**) を計測する。
warm iterations=30 (既定)。単位 ms。現行測定値:

| skills | cold_discover | cold_build | cold_total | warm p50 | warm p95 | warm max | process p50 | process p95 | process max |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 50  | 1.82 | 0.26 | 2.08  | 0.131 | 0.191 | 0.278 | 3.76 | 6.53  | 7.00  |
| 100 | 2.36 | 0.35 | 2.71  | 0.173 | 0.332 | 0.473 | 5.00 | 6.09  | 21.18 |
| 200 | 4.06 | 0.71 | 4.77  | 0.404 | 0.497 | 0.556 | 7.63 | 9.29  | 10.03 |
| 500 | 10.55| 1.95 | 12.50 | 1.004 | 1.115 | 1.146 | 13.75| 16.60 | 16.83 |

filesystem cache 状態で cold_total は変動する。表は 1 run の実測値。

読み方:

- **cold_total はほぼ線形** (50→500 で約 6 倍)。500 skills で ~12.5ms。
- **warm recommend はサブミリ秒〜1.1ms** — グラフ構築は cold のみ、
  推薦は常駐不要の都度プロセスで p95 ~1.1ms (500 skills)。
- **process p95 (CLI spawn 込み) は 500 skills で ~16.6ms** (30 samples)
  — hook として実用上無視できるレイテンシ。

## 2. 推薦精度 (`skill-graph bench evals/dataset/cases.jsonl`)

合成評価データセット 58 cases (required 52 / optional 31 / forbidden は
expected set に含まれない)。`elapsed_ms: 29` で全件評価。

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

**freeze gate 接続（#225 G04）**: `scripts/check-eval-gate.sh` が
cases=dataset 全件・required_recall=1.0・required_recall_at3=1.0・
forbidden_hits=0 を機械検査し、CI（ci.yml）と freeze 受入
（artifact-verify.yml `freeze-manifest` job）の双方で失敗を拒否する。
閾値の根拠は `docs/specs/evaluation.md`「Freeze eval gate」。

注意: これは**合成コーパスに対するオフライン評価**であり、実セッションでの
skill_load→conversion・成果物品質の改善とは別物（#225 G05 — 下記 §5）。

## 3. RSS 計測（#225 G02 — 測定範囲と主張の一致）

RSS は `/usr/bin/time -v` の "Maximum resident set size" を**各点 20 回
計測し最大値**を採用（`assets/readme/data/memory.json` が単一データ
ソース — 図・README・本表は全てそこから導出される）。単位 MB
(1024^2 bytes)。

### 3a. 実コーパス（種類の異なる 2 系列）

- `.agents/skills`（24 skills、この repo の開発スキル群）:
  **Maximum RSS 6.6 MB** (20 runs、観測範囲 6.2–6.6)
- design-skills `skills/`（88 skills、公開の大規模実コーパス）:
  **Maximum RSS 7.4 MB** (20 runs、観測範囲 7.0–7.4)

### 3b. 管理された合成系列（本文量を制御）

同一 ~240-byte SKILL.md テンプレートで **skill 数のみを変えた系列**
（`/tmp/sg-rss-{50,100,200,500}`、`/tmp/corpus500` から派生）:

- 50 skills: **6.4 MB** / 100: **6.3 MB** / 200: **6.6 MB** /
  500: **7.4 MB**（各 20 runs 最大値）

読み方: 制御系列でも 500 skills で 7.4MB — パース済み構造が小さいため
増加は緩やかだが、**「コーパス規模に依存しない」とは主張しない**。
README/SVG の表現は「6.3〜7.4MB・緩やかな増加」に揃えた（#225 G02）。
本文量・参照密度をさらに変えた系列は未測定であり、任意コーパスでの
上限保証ではない。

## 4. Hook end-to-end（#225 G03 — 実際の推薦生成を検証）

固定 root（`.agents/skills`）・固定 prompt
（`please review this pull request carefully`）で、**20/20 run が
`additionalContext` に実推薦（`code-review, github-delivery,
writing-discipline`）を含む出力を生成**することを確認した上で計測:

- wall ≤10ms（`/usr/bin/time` 分解能 0.01s で 0.00–0.01s）
- **Maximum RSS 7.0 MB** (20 runs、観測範囲 6.6–7.0)
- fail-open 設計: 常に exit 0、hook 失敗がエージェントを阻害しない
- イベントは per-user state root の `events.jsonl` に append
  （prompt 本文は SHA-256 hash のみ — contracts.md の privacy 契約）

棄却・早期終了経路（推薦 0 件・dedup 抑制）と実推薦経路は区別する:
上記は `SKILL_GRAPH_HOOK_NOSTATE=1` で dedup を無効化し、推薦が
実際に注入されることを各 run で grep 検証してから時間・RSS を採った。

## 5. 実セッション観測と実利用効果の区別（#225 G05）

このマシンの実 event log からの `observe --summary`:

- sessions: 7、recommendation delivery rate: 75%
- delivery→conversion funnel は実利用で蓄積中（サンプル数は少ない）

**正直な範囲記述**: §2 の recall/forbidden はオフライン合成評価であり、
実セッションでの invocation recall・conversion・成果物品質への効果を
実証したものではない。実利用効果の検証は observation pilot
（`docs/specs/v0.3.0-observation-pilot.md`）と公開後の #130/#207 の
継続検証に委ね、本リリースの公開前条件にはしない（pilot 仕様にない
期間完遂を新設しない — #225 G05 受入条件）。

## 6. アーキテクチャ上の帰結（README 記載の根拠）

- **永続キャッシュ・常駐サービス不要**: グラフは呼び出しごとにオンメモリ構築
  （`bench`/`perf` 内部の LRU は計測専用 — 製品 runtime は call 毎に
  再構築、contracts.md 保証表と一致）。
- **プロセスモデル**: 各呼び出しは独立プロセス。state は event log
  (append-only JSONL) のみで、クラッシュしても推薦経路に影響しない。
- **スケーラビリティ**: discovery が支配的コストだが線形かつ小さい。
  500 skills ≈ 実用上の上限想定を大きく超える規模でも ~12.5ms。

## 7. SVG データソース

`assets/readme/` の SVG は上記 §1–§4 の実測値から生成する。
データファイルは `assets/readme/data/` に一元化:
`perf-500.json`（`skill-graph perf` 出力）、`bench-eval.json`
（`skill-graph bench` 出力）、`memory.json`（RSS 計測条件・
サンプル数・各点の値）。生成スクリプト:
`scripts/gen-readme-assets.py`。手編集ではなくスクリプト再実行で
再生成可能。図・README・本書の数値は全てこれら JSON が正本。

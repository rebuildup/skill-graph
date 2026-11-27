# v0.5.0 README/SVG 用パフォーマンス・評価エビデンス

Issue: #213。README の性能・精度・効率セクションと `assets/readme/*.svg` の
データソース。全て release-0-5-0 @ `7c9d6bf` (v0.5.0 release candidate、
PR #214 merge 後) の release build で実測した。

## 測定環境

- OS: Linux (WSL2/Ubuntu)、kernel x86_64
- CPU: このマシンのシングルスレッド性能 (WSL2、他負荷なし)
- Rust: rustc 1.100.0、`cargo build --release`
- Binary: `target/release/skill-graph` (version 0.5.0)
- 測定日: 2026-11-27

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
(p50/p95/max)、process 全体 (p50/p95、5 samples) を計測する。iterations=30
(既定)。単位 ms。

| skills | cold_discover | cold_build | cold_total | warm p50 | warm p95 | warm max | process p50 | process p95 |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 50  | 1.04 | 0.17 | 1.21  | 0.078 | 0.092 | 0.123 | 2.03 | 2.30  |
| 100 | 1.41 | 0.37 | 1.78  | 0.150 | 0.186 | 0.225 | 3.01 | 3.16  |
| 200 | 2.74 | 0.76 | 3.50* | 0.337 | 0.438 | 0.486 | 5.06 | 5.44  |
| 500 | 6.88 | 1.56 | 8.44* | 0.730 | 0.847 | 0.981 | 10.66| 13.16 |

*2回目の実行では cold_total 3.58 / 9.07ms (filesystem cache 状態の差)。
表は初回実行値、両者を含めてもオーダーは同じ。

読み方:

- **cold_total はほぼ線形** (50→500 で約 7 倍)。500 skills でも 10ms 未満。
- **warm recommend はサブミリ秒** — グラフ構築は cold のみ、推薦は常駐不要の
  都度プロセスで p95 < 1ms のコア処理。
- **process p95 (CLI spawn 込み) は 500 skills で ~13ms** — hook として
  実用上無視できるレイテンシ。

## 2. 推薦精度 (`skill-graph bench evals/dataset/cases.jsonl`)

合成評価データセット 58 cases (required 52 / optional 31 / forbidden は
expected set に含まれない)。`elapsed_ms: 23` で全件評価。

| 指標 | 値 |
|---|---|
| required_recall | **1.000** (52/52) |
| required_recall@3 | **1.000** (52/52) |
| required_recall@5 | 1.000 |
| forbidden_hits | **0** |
| precision_required | 0.598 |
| precision_acceptable | 0.805 |
| recommended_total | 87 (max=3/case) |
| empty_recommendations | 6 |

注意: これは**合成コーパスに対するオフライン評価**であり、実セッションでの
skill_load→conversion とは別物。実利用効果は `observe --summary` の funnel
指標 (delivery/conversion/timeliness) で別途測定する (下記 §5)。

## 3. 実コーパスでの計測

### 3a. このリポジトリの開発スキル群 (`.agents/skills`, 17 skills)

- `discover`: wall <10ms、**Maximum RSS 6.3 MB**
- 実際の第三者製 Agent Skills (aude-skills / writing-skills /
  mattpocock/skills / design-skills 由来、keywords 非依存が大半) を含む。

### 3b. design-skills リポジトリ (`skills/`, 87 skills)

- `discover`: wall ~10ms、**Maximum RSS 7.5 MB**
- 公開されている実在の大規模スキルコーパスでの動作確認。

### 3c. 合成 500 skills コーパス (手動生成、perf と同規模)

- `discover`: wall <10ms、**Maximum RSS 7.1 MB**
- `recommend --root /tmp/corpus500 "performance testing"`: wall ~10–16ms
  (5 回計測 0.01s 表示 = /usr/bin/time 分解能)、RSS ~7.1 MB

メモリはコーパス 17→87→500 skills で 6.3→7.5→7.1 MB と**ほぼ一定**
(パース済み構造が小さいため)。常駐プロセスがないので、コストは呼び出し
ごとのプロセス起動 + 都度 discovery に限定される。

## 4. Hook end-to-end

```
echo '{"hook_event_name":"UserPromptSubmit","prompt":"test performance"}' \
  | ./target/release/skill-graph hook claude-code prompt
```

- wall <10ms、**Maximum RSS 6.4 MB**
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
  cold でも 500 skills で ~9ms、warm recommend はサブミリ秒。
  デーモン・DB・index ファイルの運用コストがゼロ。
- **プロセスモデル**: 各呼び出しは独立プロセス。state は event log
  (append-only JSONL) のみで、クラッシュしても推薦経路に影響しない。
- **スケーラビリティ**: discovery が支配的コストだが線形かつ小さい。
  500 skills ≈ 実用上の上限想定を大きく超える規模でも ~10ms。

## 7. SVG データソース

`assets/readme/` の SVG は上記 §1–§4 の実測値から生成する。
生成スクリプト: `scripts/gen-readme-assets.sh` (本 PR で追加)。
手編集ではなくスクリプト再実行で再生成可能。

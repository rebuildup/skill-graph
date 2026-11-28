# skill-graph

導入済みの Agent Skills の中から、**いまのタスクに本当に必要な
スキルを見つけてエージェントに届ける**ランタイム非依存 CLI。
Claude Code / Codex / OpenCode で同じ仕組みが使え
（opt-in adapter）、runtime の native discovery が届かない
スキルも推薦経由でエージェントへ届けられる。
標準の Agent Skills 仕様は変更しない。

## 実測: 効果が出る条件を実ランタイムで確認した

実ランタイム A/B 実験（OpenCode・Devin、同一 model・同一タスク・
複数 skill corpus）を実施した。結果は条件で分かれる。

- **runtime が既に全スキルを surface する環境では差が出ない**:
  OpenCode（plugin on/off、n=24 session/群）で呼び出し数
  1.29 vs 1.25/session、正解ロード率 87.5% vs 83.3%（いずれも
  非有意）。Devin でも corpus 8件・157件の両方で 8/8 vs 8/8。
- **runtime が surface しない root のスキルには明確な効果**:
  実務 skill 8件を runtime 非検出の場所へ移した条件で、
  推薦注入群は **7/8 が正解スキルをロード**、対照群は **0/8**
  （そのスキルに到達する経路が存在しない）。唯一の失敗は
  推薦 recall の既知欠陥（Issue #319）に起因。

つまり「入っているのに使われない」の解消は、runtime の
native discovery が届かないスキル集合に対して実測で確認
できた。delivery（推薦が文脈へ届く）と conversion
（推薦→SKILL.md 読込）は全条件で機能している。
条件・分母・生データ・限界は
[OpenCode 記録](docs/evidence/2026-11-28-v050-usage-effect-ab.md)・
[Devin 記録](docs/evidence/2026-11-29-v050-usage-effect-devin-ab.md)
・[surface 分析](docs/evidence/2026-11-28-v050-skill-surface-analysis.md)
を参照。

## 課題: スキルは入れたのに使われない

Agent Skills を導入しても、エージェントが「どのスキルが今回の
タスクに効くか」を自力で選べなければ、SKILL.md は読まれず眠る。
corpus が増えるほど選択は難しくなり、関係ないスキルを読んだり、
必要なスキルを見落としたりする。

skill-graph はこの選択を肩代わりする。プロンプトを受け取った時点で
関連スキルを推薦し、hook / plugin 経由でエージェントの文脈へ注入
する。エージェントは注入された候補から SKILL.md を実際に読み、
宣言された依存スキルも辿れる。

## 仕組み

```text
prompt → skill-graph recommend → additionalContext / system context
        → エージェントが SKILL.md を読む → skill_load を記録
        → 宣言依存を追加推薦
```

- **fail-open**: バイナリ不在・内部エラーでもセッションをブロック
  しない。壊れた hook が残っても runtime の通常動作は妨げない。
- **プライバシー**: prompt は SHA-256 hash のみ保存。会話本文・
  コマンド・tool response は永続化しない。
- **常駐しない**: デーモン・DB・インデックス・キャッシュを持たず、
  呼び出しごとのオンメモリ処理。レイテンシ p95 ≈ 17ms
  （500 skills、プロセス起動込み。詳細は
  [evidence](docs/evidence/2026-11-27-v050-readme-perf-eval.md)）。
- **推薦品質**: オフライン評価セットで Recall@3 = 52/52・
  不要推薦 0・ネガティブケース全棄権（合成コーパス。
  実利用効果とは別指標として区別する）。

## Quick start

```bash
mise use -g github:rebuildup/skill-graph   # または brew / npm / cargo（詳細は docs/install.md）
skill-graph adapter enable claude-code      # codex / opencode も同様
skill-graph adapter status claude-code      # enabled を確認
```

`adapter enable` はランタイムの設定ファイル（Claude Code
`settings.json`、Codex `hooks.json`、OpenCode `opencode.json`）へ
hook/plugin を**絶対パスで冪等に追記**する。`adapter disable` で
追加分のみ除去できる。対応 OS・最低バージョン:
Linux x86_64/ARM64 (glibc 2.39+)・macOS 15+・WSL2。
Windows native は対象外。インストール手順・CLI リファレンス・
MCP 設定・adapter 詳細・プライバシー境界・既知の制限は
[docs/install.md](docs/install.md) にまとめた。

## Documents

- [docs/install.md](docs/install.md) — 導入・CLI・adapter・privacy・制限
- [docs/evidence/](docs/evidence/) — 性能・推薦品質・実利用効果の実測記録
- [docs/specs/](docs/specs/) — 仕様・評価枠組み・観測 pilot 計画
- [docs/adr/](docs/adr/) — 設計判断

開発者向け: `AGENTS.md`（開発契約）、品質ゲート `mise run check`。

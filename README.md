# skill-graph

導入済みの Agent Skills の中から、**いまのタスクに本当に必要な
スキルを見つけてエージェントに届ける**ランタイム非依存 CLI。
スキルが「入っているのに使われない」状態を解消し、
Claude Code / Codex / OpenCode で同じ仕組みが使える
（opt-in adapter）。標準の Agent Skills 仕様は変更しない。

## 実測: 推薦は必ず文脈へ届く

実ランタイム A/B 実験（OpenCode・同一 model・同一タスク・
2 種の skill corpus、計 n=24 session/群）を実施した。
正直な結果として、**skill 呼び出し数の有意な増加は現時点で
確認できていない**（plugin 有効 1.29 vs 無効 1.25
invocations/session、正解スキルのロード率 87.5% vs 83.3%
— いずれも非有意）。一方で、推薦が実際に model の文脈へ
届く delivery と、推薦→スキル読込の conversion は実測で
確認できている。実利用効果の継続計測は `observe --summary`
の funnel 指標と観測 pilot で行う。条件・分母・生データ・
限界は
[実測記録](docs/evidence/2026-11-28-v050-usage-effect-ab.md)
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

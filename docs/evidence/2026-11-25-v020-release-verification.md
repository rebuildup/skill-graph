# v0.2.0 リリース候補検証記録

Date: 2026-11-25
Candidate: `release-0-2-0` @ `714e640`（検証実施時点の head）
Status: release candidate — main 統合・外部公開・恒久インストールは承認境界

## 検証チェックリスト

| 項目 | 結果 | 証跡 |
|---|---|---|
| 品質ゲート | ✅ `mise run check`（fmt/clippy/test 全緑、26 tests） | CI + local |
| fixture eval | ✅ 39 cases: required_recall 1.0, forbidden_hits 0 | `evals/dataset/cases.jsonl` |
| 実コーパス性能 | ✅ recommend ~9ms/process, warm p95 <0.1ms（目標 p95<100ms） | #54 evidence |
| 検索品質 | ✅ CJK tokenizer + keywords + admission 再設計。日本語・日英混在・高 df 語に対応 | #61/#62 evidence |
| Codex adapter | ✅ live 検証済み（UserPromptSubmit→additionalContext を rollout で確認）。PostToolUse/Skill は Codex に Skill tool がなく不可 — 設計境界として記録 | #70 evidence |
| OpenCode adapter | ✅ live 検証済み（plugin load、prompt/context hook、注入文を model が逐語引用）。`prompt.skills` 直接選択は未検証 | #70 evidence |
| PATH-free install | ✅ `adapters/install.sh`（絶対パス・冪等・外科的 uninstall・dry-run）。temp HOME roundtrip 検証 | #53 |
| 観測 | ✅ `skill-graph observe`（既存 transcript read-only）。transcript の Skill 記録率が低いことを実測で確認し、hook log を補完信号とする設計を記録 | #55 |
| project-init 統合 | ✅ `.agents/skills` 共有配置契約のみの疎結合設計 | #56 |
| 評価設計 | ✅ 比較可能性・交絡対策・パイロット計画を spec 化。741/190 統計は参考値扱い | #57 |

## 対応ランタイム（v0.2.0）

| Runtime | 推薦経路 | skill-load 観測 | 状態 |
|---|---|---|---|
| Claude Code 2.1.x | UserPromptSubmit + PostToolUse/Skill hooks | ✅ 両方 live 検証済み（v0.1.0） | verified |
| Codex 0.171.x | UserPromptSubmit hook（hooks.json） | ❌ Skill tool 非存在のため不可 | verified（推薦のみ） |
| OpenCode 2.3.x | TS plugin（prompt→context 注入） | 未検証（tool.execute events 要追加調査） | verified（推薦のみ） |

## 配布成果物

- `skill-graph` CLI（Rust、単一バイナリ）
- `adapters/` — claude-code / codex / opencode 各 adapter + `install.sh`
- `contracts/v1/`、`evals/dataset/`（39 cases）
- GitHub Release asset 化は v0.1.0 と同じ手順（tag→release→assets）を踏襲予定

## 既知の制約

- 日本語→英語 description の言語横断は `keywords` frontmatter 宣言が前提
  （upstream skill への付与はコンテンツ側の対応事項）
- パラフレーズ・stemming/同義語なし（軽量方式の境界、deferred）
- transcript の skill-load 記録率が低い（observe は下限値、hook log が補完）
- OpenCode `prompt.skills` 直接注入・skill-load 観測は未検証
- n=1/cell の方向性評価は統計的改善として扱わない（v0.1.0 から継続）

## 未実施（承認境界）

- `release-0-2-0` → `main` 統合
- GitHub Release / tag 作成
- 既存環境への恒久インストール
- 190-session 観測パイロット（運用開始後の計画、#57 spec）

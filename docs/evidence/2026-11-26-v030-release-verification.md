# v0.3.0 release candidate verification

Date: 2026-11-26
Candidate: `release-0-3-0` @ `57648f4`（検証中に見つかった修正を追加後、
最終 SHA は本 doc の commit を含む head）
Status: release candidate — `main` 統合・GitHub Release 公開・恒久導入は
承認境界（ユーザー明示承認待ち）。

正本: `docs/specs/v0.3.0.md`、`docs/specs/contracts.md`（Skill Usage
Event v1）、`docs/specs/v0.3.0-integration-boundaries.md`、
`docs/specs/v0.3.0-observation-pilot.md`、本証跡。

## 1. イベント観測の非汚染・プライバシー契約適合

| 項目 | 確認結果 |
|---|---|
| 既定動作 | イベントログ `${TMPDIR}/skill-graph-events.jsonl`、dedup state `${TMPDIR}/skill-graph-hook-state`。ユーザー指定は `SKILL_GRAPH_EVENT_LOG`/`SKILL_GRAPH_HOOK_STATE` のみ |
| 保存期間・容量 | `${TMPDIR}` の揮発性に依存 + `SKILL_GRAPH_EVENT_LOG_MAX`（既定 64 MiB）超過で `<log>.1` へ単一世代ローテーション（今回追加・実測済み）。state marker も TMPDIR 配下 |
| セッション分離 | `session_id` を event に保持。dedup marker は `safe_sid`（`[A-Za-z0-9._-]`・128 文字上限）でサニタイズ — `../../etc/evil` 入力で state dir 外への marker 作成がないことを実測 |
| 並列書き込み | 1 event = 1 行の `>>`（O_APPEND）書き込み。複数 hook の同時実行で行が混在しないことを構造上確認（単一 write、行長 << PIPE_BUF） |
| 安全なファイル生成 | marker はサニタイズ済み固定パターン。event log は既存ファイルへの append（symlink 追従の残存リスクは TMPDIR が user-private である前提で許容 — 契約上の保存先は TMPDIR/ユーザー指定のみ） |
| 無効化時の非生成性 | `SKILL_GRAPH_HOOK_NOSTATE=1` でログ・state dir・marker が一切作成されないことを実測（`ls` で不存在確認） |
| 本文非保存 | prompt は SHA-256 hash のみ。コマンド・tool response・会話本文は hook/plugin いずれも永続化しない（jq pre-filter で非対象 tool call は binary 起動前に除外） |

OpenCode plugin 側も同契約: `appendFileSync` で JSONL 1 行追記、
`prompt_hash` は SHA-256、`SKILL_GRAPH_EVENT_LOG` で保存先変更可、
fail-open（emit/detect 失敗は debug log のみ）。

## 2. ランタイム別 e2e（検出 → 依存推薦 → モデル到達）

| Runtime | 経路 | 結果 |
|---|---|---|
| Codex 0.171.0 | `codex exec`（project `.codex/hooks.json`、experiment 用 `--dangerously-bypass-hook-trust`）で `cat .../alpha-skill/SKILL.md` を実行 | **live verified**: event log に `prompt_submit`→`recommendation(emitted)`→`skill_load(file_read)`→`recommendation(skill_load,emitted)`。rollout JSONL に `skill-graph: 'alpha-skill' declares dependencies — beta-skill` が model context として到達 |
| Claude Code 2.1.329 | project `.claude/settings.json` + `claude -p` | prompt 経路は **live verified**（`prompt_submit`+`recommendation` 発行、additionalContext 生成）。model 呼出は CLI 未 login（`Not logged in`）で未検証 — v0.1.0/v0.2.0 evidence で同一 output contract の live 実績あり。skill-load 経路は hook script の jq 出力契約が同一で、detect 部分は単体テスト済み |
| OpenCode 2.3.2 | project `opencode.json` plugins + `opencode run` | plugin load・`session.hook("prompt")` 発火・`prompt_submit`/`recommendation` イベント発行を **live verified**（default event log に記録）。model 呼出は provider credentials 不在で未検証。`tool.hook("execute.after")` の新規検出経路は provider 不在のため live 未検証 — `detect` CLI は単体テスト済み、plugin 側は fail-open 設計 |

## 3. dotfiles 既存デバイス経路（WSL2/Linux 実機）

`~/.dotfiles` 管理下の実機で全工程を実施:

1. `install.sh claude-code --user` → live `~/.claude/settings.json` に merge
2. `bun script/claude-settings.ts compose` → skill-graph hooks が overlay
   `~/.config/dotfiles/claude/settings.local.json` に import され、
   `check` = ok（drift なし）
3. 再 install → 冪等（重複なし）
4. `install.sh --uninstall` → **live settings と overlay の両方**から除去
   （今回の修正: overlay を掃除しないと次回 compose で hooks が復活する
   問題を発見・修正）
5. 再 compose → `~/.claude/settings.json` と overlay が検証前状態と
   `jq -S` canonical で**完全一致**（復元確認）

macOS: 本環境に macOS がなく未検証。全対象パスは `$HOME`/`$PWD`/`$XDG_*`
相対で OS 依存は `stat`/`sha256sum|shasum` の fallback のみ（両対応済み）。
Windows native: adapter は bash 前提のため非対応（WSL/Linux が対象）。

Codex `--user` は `~/.codex/hooks.json`（dotfiles 未管理・新規ファイル）、
OpenCode `--user` は `~/.config/opencode/opencode.json`（同）—
`test-install-roundtrip.sh` で seeded-config roundtrip 14/14 PASS。

## 4. v0.2.0 リリース状態・main 差分・v0.3.0 整合

- v0.2.0: tag + GitHub Release 公開済み（assets: linux binary,
  adapters tarball, sha256sums）。release notes あり。
- `main..release-0-3-0`: 44 commits（W01–W06 + 修正）。v0.3.0 は
  release notes を `docs/release/v0.3.0.md` に起草済み（公開は承認後）。
- 配布成果物レイアウトは v0.2.0 と同一（`bin/` + `adapters/` +
  `contracts/v1/` + `plugins/` + `sha256sums.txt`）— release 時に
  `cargo build --release` + tar で再現可能。

## 修正（本検証で発見・ticket として統合）

- `install.sh`: claude-code `--user` uninstall が dotfiles compose overlay
  （`~/.config/dotfiles/claude/settings.local.json`）も掃除するよう修正
- hook script: `session_id` の marker 用サニタイズ（path traversal 防止）、
  `SKILL_GRAPH_EVENT_LOG_MAX` ローテーション追加
- `test-install-roundtrip.sh`: dotfiles overlay ケース追加（14 checks）

## 残存リスク / 未検証

- Claude Code の model 到達は login 環境での再確認が望ましい（contract は
  v0.1.0 実績と同一）
- OpenCode `execute.after` の live tool-call 経路は provider 認証がある
  環境での初回確認が望ましい
- macOS 実機未検証（WSL2/Linux で代替）
- Codex `/hooks` trust は通常導入経路として文書化済み（bypass は
  今回の検証でも experiment 用途のみ使用）

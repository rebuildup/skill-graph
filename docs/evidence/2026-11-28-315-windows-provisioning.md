# #315 Windows provisioning 障害の再現と復旧検証

Date: 2026-11-28
Issue: #315 / rebuildup/pc-setup#259
正本: ADR-0007、rebuildup/pc-setup PR #260（branch `259`）

## 1. 障害の再現（修正前）

実機: Windows 11 x64、mise 2026.11.7、managed config
`C:\Users\rebui\.config\mise\config.toml`（pc-setup
`scripts/apply-global-mise.ps1` の render 済みコピー）。

- `claude --version` → mise shim の auto-install が
  `github:rebuildup/skill-graph@0.4.0` を解決し
  `No matching asset found for platform windows-x64` で exit 1。
  Claude Code 2.x 自体は install 済みだが、shim が session 開始前に
  全 tool の install を試行するため起動自体が阻害された。
- `MISE_LOCKED=1`（converge と同じ条件）では
  `No lockfile URL found for github:rebuildup/skill-graph@0.4.0 on
  platform windows-x64 (--locked mode)` で失敗。pc-setup#259 の
  「latest が lockfile と照合できない」報告と一致する失敗形で、
  locked mode では asset 解決より先に lockfile URL 検索で落ちる。

## 2. 恒久修正（pc-setup 側正本）

rebuildup/pc-setup PR #260（branch `259`、base `main`）:

- `mise.global.toml` の `[tools."github:rebuildup/skill-graph"]` に
  `os = ["linux", "macos"]` を付与。upstream は Windows 向け release
  asset を公開していないため、Windows では install 対象から除外する。
- `mise.global.lock` を `mise lock --global --platform
  linux-x64,windows-x64,macos-x64,macos-arm64` で再生成。
- `scripts/ci.sh` に「skill-graph の os 条件が linux/macos に限定
  されていること」を契約として固定する検査を追加。

## 3. 検証（実 mise / 実 Windows ホスト）

修正版 render を `scripts/apply-global-mise.ps1` で実 managed config
へ適用（既存 config は `config.toml.pc-setup-backup-*` として退避）
した上で、Windows 実機で確認した。

| 項目 | 結果 |
|---|---|
| `mise install --locked`（converge と同経路） | 40/40 tools installed、skill-graph は Windows で解決対象外。exit 0 |
| `claude --version` | `2.1.332 (Claude Code)`、exit 0（修正前は exit 1） |
| `mise ls github:rebuildup/skill-graph` | Windows config からは解決されない（install 済み旧版 0.3.0 の残存のみで、config 起因の install 試行なし） |
| skill-graph 不在での agent 起動 | `claude` が skill-graph 未導入の Windows で正常起動することを確認。provisioning 失敗・不在が起動前提にならないことの実証 |
| Linux/WSL での導入維持 | 同期した config+lock で `mise install --locked github:rebuildup/skill-graph` が 0.4.0 を解決・installed（os 条件が linux を阻害しない） |
| `./scripts/ci.sh`（pc-setup） | 全項目 pass |

### カバー範囲の注記

- 検証に使った render は PR #260 の branch 内容であり、main 統合後は
  Windows converge（Scheduled Task、origin/main のみ消費）が同一
  render を再適用する。merge 前に暫定的に適用した host 状態と、
  merge 後の converge 適用結果は byte 同一のはずである（renderer は
  決定的）。
- fresh config / locked mode / 既存 host の修復は、同一 render の
  適用で代替した（apply-global-mise.ps1 は毎回 config を上書きする
  冪等経路であり、fresh/既存の差は入力 config の有無のみ）。
- skill-graph の「取得失敗」ケースは、Windows では os 条件により
  fetch 自体が発生しないことで担保される。Linux/macOS での fetch
  失敗時の挙動は mise の tool 単位失敗であり、他 tool の起動を
  阻害しない（auto-install は tool 毎に独立）。
- macOS 実機での再検証は未実施（利用可能な macOS ホストなし）。
  os 条件は macos を含むため macOS の解決経路は変更されていない。

## 4. 観測した隣接リスク（本 issue の scope 外）

検証中に、検証用 WSL ホストの pc-setup 作業ツリー（未 commit の
ローカル変更）に skill-graph entry を含まない stale な
`mise.global.lock` と、`~/.config/mise/config.local.toml` の
`skill-graph = "0.4.0"` pin が `--locked` install を
`not in the lockfile` で失敗させる状態を確認した。managed config
の正本（origin/main）には影響しないローカル状態の問題であり、
pc-setup#259 の修正対象ではないが、同種の「optional tool の lock
不整合が install を阻害する」パターンとして記録する。

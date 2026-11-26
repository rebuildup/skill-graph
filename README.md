# skill-graph

Agent Skills の発見・参照解決・推薦を支援する、エージェント非依存の CLI
ツール。標準の Agent Skills を変更せず、導入済みスキルの `SKILL.md`、
Markdown リンク、frontmatter、作業コンテキストから必要なスキルへ到達
できるようにする。

対応ランタイム: **Claude Code / Codex / OpenCode**（opt-in adapter）。

## Install

### Prebuilt binary（推奨）

[GitHub Releases](https://github.com/rebuildup/skill-graph/releases) から
OS/arch に対応するバイナリを取得し、checksum を検証する:

```bash
# Linux x86_64 の例
curl -fLO https://github.com/rebuildup/skill-graph/releases/latest/download/skill-graph-x86_64-unknown-linux-gnu
curl -fLO https://github.com/rebuildup/skill-graph/releases/latest/download/sha256sums.txt
sha256sum -c sha256sums.txt --ignore-missing
install -m 755 skill-graph-x86_64-unknown-linux-gnu ~/.local/bin/skill-graph
```

| Asset | Target |
|---|---|
| `skill-graph-x86_64-unknown-linux-gnu` | Linux x86_64 (glibc), WSL2 含む |
| `skill-graph-aarch64-unknown-linux-gnu` | Linux ARM64 (glibc) |
| `skill-graph-aarch64-apple-darwin` | macOS Apple Silicon |
| `skill-graph-x86_64-apple-darwin` | macOS Intel |

`skill-graph-adapters.tar.gz` に adapter・`install.sh`・`contracts/`・
`plugins/` が同梱される。PATH 登録は不要 — `install.sh` は絶対パスで
配線する（`SKILL_GRAPH_BIN` にバイナリのパスを指定）。

### Source

Rust toolchain（`rust-toolchain.toml` 参照）があれば:

```bash
git clone https://github.com/rebuildup/skill-graph && cd skill-graph
cargo build --release   # -> target/release/skill-graph
```

## CLI

```bash
skill-graph discover [--root <dir>]...     # スキル列挙
skill-graph search <query> [--root ...]     # キーワード検索
skill-graph recommend <query> [--root ...] # 推薦（JSON: --format json）
skill-graph resolve <target> [--root ...]  # 名前/リンク解決
skill-graph observe <path>                 # transcript/DB/イベントログの観測集計
skill-graph detect --runtime <rt> --via <via>  # hook/plugin 用イベント正規化（stdin）
skill-graph bench <dataset> / perf         # 評価・性能ハーネス
```

既定の探索 root: `~/.claude/skills`、`~/.agents/skills`、`./skills`、
`./.claude/skills`、`./.agents/skills` 等。`--root` / `SKILL_GRAPH_ROOTS`
で上書き可能。永続キャッシュ・daemon・PATH 変更は行わない。

## Runtime adapters（opt-in）

`adapters/install.sh` がランタイムのネイティブ設定へ hook/plugin を
**絶対パス・冪等 merge** で追加する。PATH・shell rc・無関係な設定は
変更しない。前提: `jq` と `skill-graph` バイナリ（`SKILL_GRAPH_BIN`）。

```bash
tar -xzf skill-graph-adapters.tar.gz
export SKILL_GRAPH_BIN=$PWD/path/to/skill-graph   # or PATH 上の名前
./adapters/install.sh claude-code --user|--project|--dry-run
./adapters/install.sh codex       --user|--project
./adapters/install.sh opencode    --user|--project
./adapters/install.sh <rt> <scope> --uninstall    # 完全復元（冪等）
```

| Runtime | Mechanism | 備考 |
|---|---|---|
| claude-code | `settings.json` hooks（UserPromptSubmit / PostToolUse `Skill`） | dotfiles compose 環境でも overlay ごと正しく uninstall される |
| codex | `hooks.json`（UserPromptSubmit / PostToolUse `Bash`） | 初回は Codex TUI の `/hooks` で trust が必要（通常導入経路） |
| opencode | `opencode.json` `plugins` → TS plugin | `file://` specifier を merge |

### 挙動

- prompt 受付時に関連スキルを推薦し `additionalContext`（または
  OpenCode の system context）として注入。
- SKILL.md の実読み込みを検出すると `skill_load` イベントを発行し、
  宣言された依存スキルを追加推薦。
- 対象外 tool call は jq pre-filter で binary 起動前に除外。
- イベントは `${TMPDIR}/skill-graph-events.jsonl`（JSONL）へ。
  prompt は SHA-256 hash のみで本文・コマンド・tool response は
  **一切保存しない**。`SKILL_GRAPH_EVENT_LOG` で保存先変更、
  `SKILL_GRAPH_HOOK_NOSTATE=1` で全書き込み無効化、
  `SKILL_GRAPH_EVENT_LOG_MAX`（既定 64MiB）でローテーション。
- すべて fail-open: 内部エラーでもセッションをブロックしない。

## Update / Uninstall

- **更新**: 新バージョンのバイナリを同じパスに置き換えるだけ。
  adapter 設定は絶対パス参照のため再 install 不要（hook script 更新時は
  Codex の `/hooks` trust が hash 変更で再要求される）。
- **無効化（一時）**: `SKILL_GRAPH_HOOK_NOSTATE=1` でイベント書き込み停止、
  または hook/plugin を設定から外す。
- **削除**: `adapters/install.sh <rt> <scope> --uninstall`。
  追加したエントリのみ除去し、他の hook/plugin・設定は保持。
  `{}` まで減った設定ファイルは削除される。

## Privacy

- 保存するのは `contracts/v1/skill-usage-event.schema.json` のイベント
  のみ。会話本文・コマンド・tool response は永続化しない。
- transcript / `opencode.db` の解析は read-only。`observe` はソースを
  変更しない。
- 既定の保存先は `${TMPDIR}`（揮発領域）のみ。永続的な観測が必要な場合は
  `SKILL_GRAPH_EVENT_LOG` にユーザー指定パスを明示する（opt-in）。

## Known limitations

- macOS は x86_64 / arm64 バイナリを提供するが実機検証は WSL2/Linux のみ。
- OpenCode の `tool.execute.after` 検出経路は provider 認証環境での
  live 確認が今後の観測対象（`detect` CLI 自体はテスト済み）。
- Codex は skill-load を `PostToolUse` matcher `Bash` のコマンド分類で
  検出する（Codex に Skill tool は存在しない）。
- 永続キャッシュ・MCP・Embedding は意図的に未実装（効果実証まで保留）。

## Development

`AGENTS.md`（開発契約）、`docs/specs/`（仕様・評価・pilot 計画）、
`docs/evidence/`（実測記録）、`docs/adr/`（設計判断）が正本。
品質ゲート: `mise run check`（fmt + clippy + test）。

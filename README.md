# skill-graph

Agent Skills の発見・参照解決・推薦を支援する、エージェント非依存の CLI
ツール。標準の Agent Skills を変更せず、導入済みスキルの `SKILL.md`、
Markdown リンク、frontmatter、作業コンテキストから必要なスキルへ到達
できるようにする。

対応ランタイム: **Claude Code / Codex / OpenCode**（opt-in adapter）。

## Install

パッケージマネージャから導入する。hook の有効化は install では行われず、
後述の `skill-graph adapter enable` で明示的に opt-in する。

### mise

```bash
mise use -g github:rebuildup/skill-graph          # 最新版
mise use -g github:rebuildup/skill-graph@0.4.0   # バージョン固定
```

### npm / Bun / pnpm

```bash
npm install -g @rebuildup/skill-graph
bun add -g @rebuildup/skill-graph
pnpm add -g @rebuildup/skill-graph
```

プラットフォーム別バイナリを optional dependency として同梱する
（postinstall のダウンロード・ビルドなし）。pin は
`npm install -g @rebuildup/skill-graph@0.4.0`。

### Homebrew

```bash
brew install rebuildup/tap/skill-graph
```

### crates.io

```bash
cargo install skill-graph
```

ソースからビルドする場合（Rust toolchain は `rust-toolchain.toml` 参照）:

```bash
git clone https://github.com/rebuildup/skill-graph && cd skill-graph
cargo build --release   # -> target/release/skill-graph
```

### 対応プラットフォーム

Linux x86_64/ARM64 (glibc, WSL2 含む)、macOS x86_64/ARM64。
Windows は WSL2 経由。musl・Windows native は対象外。

## CLI

```bash
skill-graph discover [--root <dir>]...                  # スキル列挙
skill-graph search <query> [--root ...]                  # キーワード検索
skill-graph recommend <query> [--root ...]               # 推薦（--format json）
skill-graph resolve <target> [--root ...]                # 名前/リンク解決
skill-graph observe <path> [--summary]                     # transcript/DB/イベントログの観測集計（--summary で funnel 指標）
skill-graph detect --runtime <rt> --via <via>            # hook/plugin 用イベント正規化（stdin）
skill-graph hook <claude-code|codex> <prompt|skill-load> # ランタイム hook 入口（stdin、fail-open）
skill-graph mcp                                          # stdio MCP server（read-only tools）
skill-graph adapter <enable|disable|status> <rt>         # adapter ライフサイクル管理
skill-graph bench <dataset> / perf                       # 評価・性能ハーネス
```

既定の探索 root: `~/.claude/skills`、`~/.agents/skills`、`./skills`、
`./.claude/skills`、`./.agents/skills` 等。`--root` / `SKILL_GRAPH_ROOTS`
で上書き可能。永続キャッシュ・daemon・PATH 変更は行わない。

## MCP server

`skill-graph mcp` は stdio トランスポートの MCP server。CLI と同一の
Discovery → Resolver → Recommendation エンジンを read-only tool として
公開する（ADR-0005）。tool は `recommend` / `discover` / `resolve` の
3 つで、状態変更・イベント書き込みは行わない。

```bash
skill-graph mcp   # stdio。JSON-RPC を stdin/stdout で話す
```

各クライアントの MCP 設定例（`command` はインストール済み
`skill-graph` のパス）:

```jsonc
// Claude Code: ~/.claude.json またはプロジェクト .mcp.json
{ "mcpServers": { "skill-graph": { "command": "skill-graph", "args": ["mcp"] } } }

// OpenCode: opencode.json
{ "mcp": { "skill-graph": { "type": "local", "command": ["skill-graph", "mcp"] } } }

// Codex: ~/.codex/config.toml
[mcp_servers.skill-graph]
command = "skill-graph"
args = ["mcp"]
```

`observe --summary` は observation-pilot の funnel 指標
（delivery / conversion / unnecessary-load / timeliness / context
consumption、runtime 別内訳つき）を算出する。

## Runtime adapters（opt-in）

`skill-graph adapter` がランタイムのネイティブ設定へ hook/plugin を
**絶対パス・冪等 merge** で追加する。PATH・shell rc・無関係な設定は
変更しない。`--user`（既定）/ `--project`（カレントプロジェクト）を選択可。

```bash
skill-graph adapter enable  claude-code
skill-graph adapter enable  codex
skill-graph adapter enable  opencode
skill-graph adapter status  claude-code   # enabled/disabled/legacy/broken
skill-graph adapter disable claude-code   # 追加エントリのみ除去・完全復元
```

| Runtime | Mechanism | 備考 |
|---|---|---|
| claude-code | `settings.json` hooks（UserPromptSubmit / PostToolUse `Skill`） | hook コマンドは `<bin> hook claude-code <mode>` |
| codex | `hooks.json`（UserPromptSubmit / PostToolUse `Bash`） | 初回は Codex TUI の `/hooks` で trust が必要（通常導入経路） |
| opencode | `opencode.json` `plugins` → 同梱 TS plugin | `file://` specifier、plugin は `~/.local/share/skill-graph/opencode-plugin/` に materialize |

hook コマンドには enable 時に PATH 解決した `skill-graph` の絶対パスを
埋め込む。mise shim・npm global bin・brew のいずれのパスでも動作する。
明示指定は `--bin <path>`。

### 挙動

- prompt 受付時に関連スキルを推薦し `additionalContext`（または
  OpenCode の system context）として注入。
- SKILL.md の実読み込みを検出すると `skill_load` イベントを発行し、
  宣言された依存スキルを追加推薦。
- すべて fail-open: バイナリ不在・内部エラーでもセッションをブロック
  しない（hook は常に exit 0、plugin は spawn 失敗を握りつぶす）。
  パッケージ削除後に壊れた hook が残っても runtime の通常動作は妨げない。
- イベントは `${TMPDIR}/skill-graph-events.jsonl`（JSONL）へ。
  prompt は SHA-256 hash のみで本文・コマンド・tool response は
  **一切保存しない**。`SKILL_GRAPH_EVENT_LOG` で保存先変更、
  `SKILL_GRAPH_HOOK_NOSTATE=1` で全書き込み無効化、
  `SKILL_GRAPH_EVENT_LOG_MAX`（既定 64MiB）でローテーション。

## Update / Uninstall

- **更新**: 各パッケージマネージャの通常手順（`mise upgrade`、
  `npm install -g @rebuildup/skill-graph@latest`、`brew upgrade`、
  `cargo install --force`）。hook コマンドのパスが変わらない限り
  adapter の再 enable は不要。パスが変わった場合は
  `adapter disable && adapter enable` で張り替える。
- **無効化（一時）**: `SKILL_GRAPH_HOOK_NOSTATE=1` でイベント書き込み停止。
- **adapter のみ無効化**: `skill-graph adapter disable <rt>`。
  追加したエントリのみ除去し、他の hook/plugin・設定は保持。
  `{}` まで減った設定ファイルは削除される。
- **完全削除**: パッケージマネージャの uninstall に加え、事前に
  `adapter disable` を実行して hook/plugin 参照を除去する
  （残っても fail-open だが、設定を清潔に保つため推奨）。

## v0.3.x からの移行

v0.3.x の `install.sh` / `adapters/install.sh` で導入した環境は、
新版インストール後に一度だけ移行する:

```bash
skill-graph adapter enable <rt>   # 旧 skill-graph-recommend-hook /
                                  # /skill-graph plugin 参照を自動検出・除去して置き換え
skill-graph adapter status <rt>   # enabled を確認後、旧配置を削除
rm -rf ~/.local/share/skill-graph ~/.local/bin/skill-graph
```

Codex の `/hooks` trust はコマンド文字列の hash-pin のため、移行で
コマンドが変わると **一度だけ再 trust が必要**（ユーザー操作）。

## Privacy

- 保存するのは `contracts/v1/skill-usage-event.schema.json` のイベント
  のみ。会話本文・コマンド・tool response は永続化しない。
- transcript / `opencode.db` の解析は read-only。`observe` はソースを
  変更しない。
- 既定の保存先は `${TMPDIR}`（揮発領域）のみ。永続的な観測が必要な場合は
  `SKILL_GRAPH_EVENT_LOG` にユーザー指定パスを明示する（opt-in）。

## Known limitations

- macOS バイナリは提供するが実機検証は限定的。
- Codex の skill-load 検出は `PostToolUse` matcher `Bash` のコマンド分類
  （Codex に Skill tool は存在しない）。
- bun は os/cpu gating を適用しないため全 platform package を取得する
  （shim が正しい arch を選択するため動作上の問題はない）。
- 永続キャッシュ・Embedding・グラフDB は意図的に未実装（v1.0.0 spec で
  非採用を確定。MCP server は実装済み）。

## Development

`AGENTS.md`（開発契約）、`docs/specs/`（仕様・評価・pilot 計画）、
`docs/evidence/`（実測記録）、`docs/adr/`（設計判断）が正本。
品質ゲート: `mise run check`（fmt + clippy + test）。

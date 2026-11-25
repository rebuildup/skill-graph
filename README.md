# skill-graph

Agent Skills の発見・参照解決・推薦を支援する、エージェント非依存のツール。

標準の Agent Skills を変更せず、導入済みスキルの `SKILL.md`、Markdown リンク、frontmatter、作業コンテキストから、必要なスキルへ到達できるようにすることを目的とする。

## Status

v0.1.0（実験的初期リリース候補）。コアエンジン（Discovery / Parser / Resolver / Recommendation）、CLI、Agent Plugins v1.0.0 準拠パッケージ、Claude Code アダプタ（opt-in hooks）を含む。評価・実機検証の証跡は `docs/evidence/` を参照。

## Documents

- 開発契約: `AGENTS.md`
- 開発環境: `docs/development.md`
- アーキテクチャ: `docs/architecture.md`
- 仕様: `docs/specs/`
- 設計判断: `docs/adr/`
- 評価計画: `docs/specs/evaluation.md`
- リリース検証記録: `docs/evidence/2026-11-25-r01-release-verification.md`

## Install

前提: Rust toolchain（`rust-toolchain.toml` 参照）と mise。

```bash
mise install
cargo build --release
# binary: target/release/skill-graph
```

PATH への恒久登録は行わない。必要な場合のみ `SKILL_GRAPH_BIN` または絶対パスで参照する。

## Usage

```bash
skill-graph discover --root <skills-dir>
skill-graph recommend "<task description>" --root <skills-dir> --max 3
skill-graph resolve <skill-name> --root <skills-dir>
skill-graph bench evals/dataset/cases.jsonl
skill-graph perf --sizes 50,100,200,500
```

推奨ルートは繰り返し `--root` で指定する（デフォルトは慣例ディレクトリ）。

## Claude Code adapter (opt-in)

`adapters/claude-code/skill-graph/` は Agent Plugins v1.0.0 準拠のプラグインで、セッション限定で読み込む。

```bash
claude -p "<task>" \
  --plugin-dir /path/to/skill-graph/adapters/claude-code/skill-graph \
  --setting-sources project --no-session-persistence \
  --output-format stream-json --include-hook-events --verbose \
  --permission-mode bypassPermissions
```

Hook は `UserPromptSubmit`（プロンプト推薦）と `PostToolUse` matcher `Skill`（読み込み済みスキルの宣言依存 `requires-skills` 推薦）の2系統。推薦がなければ無出力、内部エラーでも fail-open。状態・ログは `${TMPDIR}` 配下のみ。

環境変数: `SKILL_GRAPH_BIN`（バイナリパス）、`SKILL_GRAPH_ROOTS`（colon 区切りルート。既定: `~/.claude/skills`, `~/.agents/skills`, `$PWD/skills`, `$PWD/.claude/skills`, `$PWD/.agents/skills`）、`SKILL_GRAPH_HOOK_LOG`、`SKILL_GRAPH_HOOK_STATE`、`SKILL_GRAPH_HOOK_NOSTATE=1`（無保存モード）。

## Supported runtimes

- **Claude Code 2.1.x**: hooks 実測済み（`UserPromptSubmit`, `PostToolUse:Skill`）。live 検証は Kilo anonymous free route で実施。
- **Codex / OpenCode**: 未対応（adapter なし）。CLI 単体はランタイム非依存で利用可能。

## Known limitations (v0.1.0)

- 推薦はテキスト一致 + `requires-skills` 1-hop の構造的 admission。語形変化（`diagnose`↛`diagnosis`）や意味的近さは扱わない。
- stopword フィルタは英語のみ。日本語クエリの精度は dataset 範囲でのみ検証済み。
- 実機比較は n=1/cell・単一モデル・単一ルート。統計的な改善効果は未確立。
- 永続キャッシュ・MCP・グラフDB・Embedding は未実装（効果実証まで保留）。
- 同名スキルの識別契約はエンジン層で検証済み。live ランタイムでの同名衝突ケースは未検証。
- Codex/OpenCode adapter、PATH 不要の恒久インストール経路は v0.2.0 以降。

## Uninstall

- CLI: `target/release/skill-graph` を削除するだけ。設定・ログ・キャッシュは生成しない。
- Claude Code adapter: `--plugin-dir` はセッション限定のため永続物なし。恒久インストールを行っていないため除去対象なし。万が一 `claude plugin` でインストールした場合は `claude plugin uninstall skill-graph`。
- hook state/log は `${TMPDIR}/skill-graph-hook-*` のみ。`SKILL_GRAPH_HOOK_NOSTATE=1` で生成自体を抑止可能。

## Principles

- 標準 Agent Skills を原則として変更しない。
- 対象プロジェクトへキャッシュ、ログ、管理ファイルを生成しない。
- 推薦は根拠と観測状態を返し、候補がなければ無出力とする。
- CLI、Hook、Plugin、将来の MCP は共通エンジンの薄い入口とする。

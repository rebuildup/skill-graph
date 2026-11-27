# Development

## Requirements

- Rust stable toolchain（`rust-toolchain.toml` で固定）
- mise（タスク実行）
- Git
- Bun は補助スクリプトが必要な場合のみ使用する

## Setup

```bash
mise install
cargo fetch
mise run check
```

## Repository layout

```text
src/                 CLI とコアエンジン（cli/hook/adapter/mcp/observe/events 等）
tests/               統合テスト（core/hook/adapter/mcp/observe/contract/security/cli/detect）
contracts/v1/        公開 JSON Schema（CLI 出力・Skill Usage Event）
fixtures/skills/     テスト・評価用の最小スキル群
evals/               評価データセットとハーネス
examples/            実験用バイナリ例（x02/x03 等、install 対象外。証跡再現用に保持）
plugins/skill-graph/ Agent Plugins v1.0.0 準拠 package（plugin.json + skills/）
npm/                 @rebuildup/skill-graph npm package 群（metapackage + platform optional deps）
docs/specs/          製品・標準・評価・互換性仕様（正本と履歴の区別は specs/README.md）
docs/adr/            設計判断記録
docs/release/        リリースノート
docs/evidence/       実測結果と再現手順
quality/             品質契約・補助設定
.agents/skills/      Agent Skills の正本（skills CLI の canonical 配置）
skills/              openclaw 向け agent view（`.agents/skills` への per-skill symlink）
.claude/skills/      Claude Code 向け agent view（同上）
.devin/skills/       Devin 向け agent view（同上）
skills-lock.json     skills CLI の導入 revision / hash 記録
```

`experiments/` は廃止した。検証済みの Claude Code 実験資産は公式入口
`plugins/skill-graph/` と adapter 系実装に置き換わっており、内容は Git
履歴（`experiments/claude-code/`）を参照する。

### Agent Skills の配置

skills CLI（vercel-labs/skills）の配置契約に従い、canonical copy は
`.agents/skills/` のみが実ファイルを持つ。`skills/`・`.claude/skills/`・
`.devin/skills/` は skills CLI の symlink install と同じ形の per-skill
symlink（例: `.claude/skills/<name>` → `../../.agents/skills/<name>`）で、
各 runtime の project skill discovery path を満たす。discovery
（`src/discovery.rs`）は symlink を辿らないため、同一 skill が複数 root
から重複発見・重複 parse されることはない。

`skills-lock.json` の `installMode` は現在 `"copy"` であり、
`bunx skills update` / `experimental_install` を実行すると agent 向け
directory が実ファイルの複製に置き換わる。その場合は commit 前に
symlink を張り直す（`.agents/skills/` を正本として `ln -s` で
`skills/`・`.claude/skills/`・`.devin/skills/` 配下に per-skill symlink
を作る）。複製をそのまま commit しない。

## Commands

| Command | Purpose |
|---|---|
| `mise run fmt` | `cargo fmt` |
| `mise run lint` | `cargo clippy` |
| `mise run test` | `cargo test` |
| `mise run bench` | 評価・性能ハーネス |
| `mise run check` | fmt + lint + test |
| `cargo run -- search <query>` | CLI の基本検索 |
| `cargo run -- recommend <query>` | 根拠付き推薦 |

## Conventions

- コードとコメントは英語、内部ドキュメントと Issue は日本語。
- 新規 Python script は作らない。必要なら ADR で例外理由を記録する。
- 公開関数・モジュールには doc comment を付ける。
- 正本ドキュメントと実装がずれたら、実装を直すか Spec / ADR を先に更新する。

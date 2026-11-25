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
src/                 CLI とコアエンジン
fixtures/skills/     テスト・評価用の最小スキル群
evals/               評価データセットとハーネス
docs/specs/          製品・標準・評価・互換性仕様
docs/adr/            設計判断記録
docs/evidence/       実測結果と再現手順
quality/             品質契約・補助設定
skills/              プロジェクトで利用する Agent Skills
skills-lock.json     skills CLI の導入 revision / hash 記録
```

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

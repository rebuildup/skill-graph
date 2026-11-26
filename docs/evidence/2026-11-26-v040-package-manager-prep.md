# v0.4.0 パッケージマネージャ配布 — 準備検証記録

2026-11-26。`release-0-4-0` 上の #100–#108 実装に対する配布チャネル別の
検証結果。外部レジストリへの正式公開は未実施（承認・認証待ち）。

## 実装済み

- `skill-graph hook <runtime> <mode>`: v0.3.x shell recommend-hook の
  Rust 移植。イベント契約・additionalContext 形式は同一、fail-open。
- `skill-graph adapter enable|disable|status <runtime> [--user|--project]
  [--bin PATH]`: v0.3.x `adapters/install.sh` の Rust 移植。marker 識別で
  v0.3.x 形式（`skill-graph-recommend-hook`、`/skill-graph` plugin）と
  v0.4.x 形式を区別し、enable が legacy を自動置換、disable が完全復元
  （dotfiles compose overlay 掃除を含む）。OpenCode plugin は
  `src/assets/opencode-plugin.ts` を binary に内包し
  `~/.local/share/skill-graph/opencode-plugin/` へ materialize。
- `install.sh`・`adapters/` ツリー廃止。release workflow は
  per-target binary + tarball + `sha256sums.txt` のみを成果物化。

## チャネル別検証

### mise（`github:rebuildup/skill-graph` backend）

- release tarball 資産（`skill-graph-<target>.tar.gz`）は v0.4.0 でも
  同名で継続公開されるため、pc-setup `mise.global.toml` の既存エントリは
  変更不要。`@latest` 解決は minimum_release_age の対象になるのみで
  lock 済み install には影響しない。
- 実機（WSL2）では `mise` shim パスが `adapter enable` の PATH 解決で
  正しく絶対パス化されることを確認（`--bin` 明示も可）。

### npm（`@rebuildup/skill-graph`）

- 構成: metapackage + `@rebuildup/skill-graph-{linux,darwin}-{x64,arm64}`
  optional deps。postinstall download/build なし。bin shim
  （`bin/skill-graph.js`）が `require.resolve` で platform package を
  解決し native binary を exec。
- ローカル実測（registry 未公開のため `file:` tarball で代替）:
  - `npm install`: os/cpu gating で linux-x64 のみ導入、
    `skill-graph --version` 正常
  - `pnpm install`: 同上
  - `bun install`: os/cpu を適用せず全 platform package を取得するが、
    shim が正しい arch を選択するため動作は正常（帯域のみの差）
  - `npm pack` dry-run: metapackage 1.6kB shim、platform pkg は
    binary のみ
- 未検証: 実レジストリでの install/pin/upgrade/uninstall
  （初回 publish 後に実施）。publish には `NPM_TOKEN` secret が必要。

### Homebrew（`rebuildup/tap`）

- `rebuildup/homebrew-tap` に `Formula/skill-graph.rb` を追加
  （draft PR: rebuildup/homebrew-tap#1）。`on_macos`/`on_linux` ×
  `on_intel`/`on_arm` で release tarball を参照。
- sha256 は v0.4.0 release 資産公開後に `sha256sums.txt` から転記して
  から merge する（placeholder のままでは `brew audit` が fail）。
- `brew install/upgrade/uninstall` の実測は macOS 実機で release 後に実施。

### crates.io（`skill-graph`）

- crate 名 `skill-graph` は未使用を確認。`Cargo.toml` に `readme` と
  `include`（`src/`・manifest・LICENSE・README のみ）を追加し、
  `cargo publish --dry-run` が 26 files / 208KiB で green。
- publish には crates.io token（`CARGO_REGISTRY_TOKEN`）が必要。
  version は Cargo.toml と同期して 0.4.0 へ bump する。

## ライフサイクル保証（設計 + テスト）

- install は常に opt-in: package install だけでは hook は有効化されない。
- package 削除後: hook/plugin は fail-open（exit 0 / spawn 失敗を握りつぶし）
  のため runtime の通常動作を妨げない。`adapter status` が broken を報告。
- `adapter disable` は seeded config をバイト単位で復元
  （runtime×scope 6 組合せ + legacy 混在を `tests/adapter.rs` で検証）。
- Codex `/hooks` trust 境界は維持: v0.3.x → v0.4.x 移行でコマンド文字列が
  変わるため **一度だけ再 trust が必要**（ユーザー操作、仕様通り）。

## 残件（公開承認・外部依存）

- `NPM_TOKEN` / crates.io token / homebrew-tap merge 権限の確認と付与
- v0.4.0 release 資産公開後: brew formula sha256 転記、npm publish
  workflow 実行、cargo publish、実レジストリ e2e
- pc-setup#242: minimum_release_age 解禁（2026-11-27 12:19 JST）後に
  CI re-run → merge → `~/.config/mise/config.local.toml` の暫定 pin 除去

# ADR-0004: パッケージマネージャ第一の配布アーキテクチャ

Date: 2026-11-26
Status: accepted (v0.4.0)

## Context

v0.3.x では GitHub Release のバイナリ tarball + adapters bundle を正本とし、
root `install.sh`（curl|bash）と `adapters/install.sh`（shell + jq）を
導入・有効化経路とした。しかし以下の問題が判明した。

- curl|bash はサプライチェーン上の信頼境界が弱く、パッケージマネージャの
  lock・checksum・sandbox 機能を利用できない。
- CLI・adapter・contracts が別成果物（binary tarball / adapters bundle /
  install.sh）のため、version 間の不整合・旧参照残存が構造的に起き得る
  （v0.3.1 で marker 識別により緩和したが、根本的には単一成果物化が望ましい）。
- adapter 有効化に `jq`・shell・展開済みファイル群を要求し、package manager
  が管理する領域と独自レイアウト（`~/.local/share/skill-graph/<ver>/` +
  `current` symlink）が二重管理になる。

ユーザー要求: 一般利用者が既存パッケージマネージャで導入・version pin・
更新・削除できることを正式要件とし、GitHub Release を共通バイナリ配布元
として全パッケージを同一 SHA・同一バージョンから生成する。

## Decision

### 単一成果物: skill-graph バイナリ

CLI・hook ロジック・OpenCode plugin・contracts をすべてバイナリに内包する。

- `skill-graph hook <claude-code|codex> <prompt|skill-load>`: 従来の
  `adapters/*/bin/skill-graph-recommend-hook`（bash+jq, ~200行）の Rust 移植。
  stdin の hook payload を読み、Skill Usage Event を発行し、recommendation
  を `hookSpecificOutput.additionalContext` として stdout へ返す。
  jq pre-filter・dedup marker・event log rotation・`SKILL_GRAPH_HOOK_NOSTATE`
  等の既存契約をすべて Rust 側で再実装する。
- `skill-graph adapter <enable|disable|status> <claude-code|codex|opencode>
  [--user|--project]`: 従来の `adapters/install.sh` の Rust 移植。
  hook command は `<絶対パス>/skill-graph hook <rt> <mode>` を設定へ merge
  する。`--project` は cwd 基準。
- OpenCode plugin（`adapters/opencode/skill-graph/index.ts`, 223行）は
  `include_str!` でバイナリに内包し、`adapter enable opencode` が
  `~/.local/share/skill-graph/opencode-plugin/index.ts`（version 非依存の
  安定パス）へ書き出す。plugin は `skill-graph` を PATH/`SKILL_GRAPH_BIN`
  経由で spawn するため、バイナリ更新は plugin 側の変更なしに追従する。
- 旧形式（`skill-graph-recommend-hook` script 参照、`/skill-graph` plugin
  specifier、versioned `adapters/` path 参照）は `adapter enable` が
  marker 検出で同時に除去し、1コマンドで v0.3.x からの移行を完了させる。

### 配布チャネル

GitHub Release の per-target tarball（`skill-graph-<target>.tar.gz` +
`sha256sums.txt`）を唯一のバイナリ正本とし、以下を同一 SHA から生成する。

| Channel | Package | 方式 |
|---|---|---|
| npm / Bun / pnpm | `@rebuildup/skill-graph` | metapackage + per-platform optional deps（biome/esbuild 型）。postinstall download なし。bin は Node shim が platform package のバイナリを exec |
| Homebrew | `rebuildup/tap/skill-graph` | 既存 tap に formula 追加。release tarball を `on_macos`/`on_linux` + arch 別 url/sha256 で参照 |
| mise | `github:rebuildup/skill-graph` | 既存 github backend（pc-setup #241 と同経路）。registry 追加は popularity bar 未達のため行わない |
| crates.io | `skill-graph` | `cargo install skill-graph`（source build）。名前は空き確認済み。`cargo binstall` 対応は将来検討 |
| Nix | — | 今回は対象外（pkgs PR は利用実績が出てから） |

### 廃止するもの

- root `install.sh`（curl|bash 経路）: v0.4.0 で削除。release asset からも除外。
- `adapters/` ツリー（install.sh, hook scripts, plugin file, hooks.json
  templates）: CLI 内包に置き換え、v0.4.0 で削除。
- `skill-graph-adapters.tar.gz` release asset: 廃止。

## Consequences

- package manager が管理するのはバイナリのみ。adapter 状態（hook 設定）は
  常に `skill-graph adapter` が管理し、PM の管理領域と競合しない。
- PM 削除後に hook 設定が残っても、hook command のバイナリ不在は各
  runtime の fail-open で通常動作を妨げない（`adapter status` が broken を
  検出して報告）。OpenCode plugin は materialize 先に残るが binary 不在時は
  no-op で fail-open。
- `jq` 依存が消える（hook の pre-filter は Rust 化）。shell 依存も消え、
  Windows native も理論上可能になる（adapter が POSIX shell を要求しない）。
- npm publish・crates.io publish・brew tap 更新にはそれぞれ認証情報が必要
  （`NPM_TOKEN` / `CARGO_REGISTRY_TOKEN` / tap repo write）。公開承認は
  ユーザーの明示承認を要する。

## Alternatives considered

- **npm 単一パッケージ + postinstall download**: postinstall で GitHub Release
  から取得する方式は reject（postinstall は npm/pnpm/bun で挙動差があり、
  offline/enterprise 環境で失敗し得る。optional deps の方が native）。
- **adapter ファイルを npm package に同梱**: PM ごとに同梱方法が異なり、
  binary 同梱の方が単一成果物の要件をより強く満たす。plugin のみ例外的に
  materialize が必要（OpenCode が file path を要求するため）。
- **mise registry 追加**: popularity bar（数千 star・外部利用）を現時点で
  満たさない。`github:` backend で既に利用可能。
- **homebrew-core への昇格**: 初期は self tap。利用実績・star が出た段階で
  homebrew-core PR を検討。

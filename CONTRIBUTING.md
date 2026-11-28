# Contributing

skill-graph は release-driven development で進める。GitHub Issue、Spec、ADR、PR を正本とし、会話だけに残る設計判断を作らない。

## Workflow

1. GitHub Issue を作成し、目的、要求、対象範囲、非目標、依存、受入条件、検証方法を記録する。
2. `Issue番号` だけを名前にした ticket branch を作る。
3. 実装し、`mise run check` を通す。
4. ticket PR を release branch へ出す。
5. release candidate で統合・評価を行い、`release-x-y-z -> main` はユーザー承認後に merge commit で統合する。

## Change rules

- 仕様変更は先に Spec または Issue を更新する。
- 設計判断は ADR に記録する。
- 公開 API、保存形式、権限境界、外部依存の変更は明示的にレビューする。
- `main` への直接 push、force push、rebase merge は行わない。
- Agent Skills の正本は `.agents/skills/` のみ。`skills/`・`.claude/skills/`・`.devin/skills/` は symlink であり、`bunx skills` の copy install で実ファイルに戻った場合は symlink に戻してから commit する（詳細: `docs/development.md`「Agent Skills の配置」）。

## Verification

通常 ticket では `mise run check` を通す。これはローカルで完結する
全 correctness gate を含む: `cargo fmt --check`、`cargo clippy`、
`cargo test`、OpenCode plugin の bun contract test、release 系
shell suite（`test-release-verify.sh` / `test-freeze-manifest.sh` /
`test-npm-publish.sh`）。

`mise run check` が**含まない**もの（#225 F07 — check green を
包括保証と混同しない）:

- plugin の host API 型検査（`mise run check-plugin-types`、
  `@opencode/plugin` の install が必要）
- 4 ターゲットの packaging / native install 検証
  （`artifact-verify.yml`、freeze 時に集約）
- bench / eval / perf（`mise run bench` / `mise run perf`、
  release candidate での証跡測定）
- npm/crates.io publish・provenance 等の公開後検証

重い benchmark や実機検証は release candidate で集約する。ただし、実現可能性の判断に必要な最小計測は ticket 内で行ってよい。

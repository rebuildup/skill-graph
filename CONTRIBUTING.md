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

## Verification

通常 ticket では次を最低限確認する。

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- 変更に対応する fixture / contract test

重い benchmark や実機検証は release candidate で集約する。ただし、実現可能性の判断に必要な最小計測は ticket 内で行ってよい。

# ADR-0001: Core engine language for MVP

Status: accepted for MVP  
Date: 2026-11-25

## Context

skill-graph の中核は、ローカル filesystem 上の `SKILL.md` を解析し、参照解決と推薦を行う。配布形態として single binary、CLI、将来の Plugin / Hook adapter を想定する。Rust と TypeScript の両方が候補だった。

## Decision

MVP のコアエンジンは Rust で実装する。CLI も Rust の単一バイナリとする。

## Rationale

- 単一バイナリ配布と cold start に有利。
- filesystem 走査、Markdown / YAML 解析、path 解決を依存少なく実装できる。
- Hook から起動する場合の runtime 依存を減らせる。
- Agent 非依存のコアとして、特定 runtime の plugin 形式へ結合しない。

TypeScript は adapter、評価補助、または特定 runtime が要求する場合の薄い入口として再検討する。コアエンジンを TypeScript に戻すかは、C03 / E03 の benchmark と配布制約で再評価する。

## Consequences

- `Cargo.toml` / `rust-toolchain.toml` を version source とする。
- Plugin / Hook adapter はコア契約を変えずに追加できる構造にする。
- 外部公開時の署名・公証・OS 別配布は P01 で別途検証する。

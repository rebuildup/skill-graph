# ADR-0002: MVP recommendation approach

Status: accepted  
Date: 2026-11-25

## Context

E02–X03・C03 の実験結果を統合し、MVP に採用する推薦方式・介入経路・性能目標値を確定する。対象は評価ハーネス、オンメモリ Discovery / Parser / Resolver、推薦エンジン、最小 CLI、単一ランタイム（Claude Code）向け最小 Hook である。

## Decision

1. **推薦エンジンは group F（text search + 参照 graph）を採用する。** 現行の admission 規則（name / description / context の直接適合 + admitted skill の 1-hop `requires` による necessity 付与）を MVP の推薦ポリシーとする。Markdown link・mention・body・backlink は weak evidence に留め、単独では admit しない。
2. **介入経路は `UserPromptSubmit` hook を第一候補とし、`additionalContext` で top-3 を注入する。** 推薦なし・重複時は無出力とする。SessionStart の compact 注入は単独経路にしない（X02 で recall 低下を実測）。
3. **性能目標値を確定する。** recommend warm p95 < 100ms、Hook 全体 < 250ms、1 回の注入あたり推薦最大 3 件。C03 実測（release、500 skills で process p95 約 13ms、warm recommend p95 約 0.7ms）と X03 実測（debug fixture で hook end-to-end 約 42ms）はいずれも予算内。
4. **永続キャッシュ・グラフDB・Embedding・MCP・多 runtime 同時対応は MVP に含めない。** C03 でキャッシュ不要が確認されたため O01 は発動条件未達として deferred とする。

## Rationale

- **X01**: description 改善のみでは paired subset で指標に測定可能な差が出なかった。description は既に直接スコアの入力であり、独立した介入機構としては採用しない。
- **X02**: context のみの compact start は required_recall 0.758・empty 11 に低下し、単独経路として不十分。baseline への参照 union は forbidden/precision を悪化させ、選択的 admission（現行規則）の維持を支持する。
- **X03**: E（text-only）と F（text+graph）は required_recall 1.0・forbidden 0 で同等だが、F は `requires` 経由で optional hit を 5→14 に増やし recommended_total を 43→53 に増やした。precision_acceptable はほぼ不変（0.884 vs 0.887）。「エージェントが依存の必要性を認識できない」ことが本プロダクトの対象問題であるため、宣言済み依存を surface できる F を採用する。
- **C03**: 50–500 skills の synthetic corpus で cold discover+build 約 7.7ms、CLI process p95 約 13ms（release）。Hook 予算に対して 1–2 桁の余裕があり、永続キャッシュの必要性は実証されていない。

## Consequences

- MVP 構成: eval harness + オンメモリ engine + `recommend` CLI + Claude Code 向け `UserPromptSubmit` hook adapter（P01 / P02、Issue #28 / #29）。
- `max=3` を hook 注入の上限とする。CLI の `recommend --max` 既定値 3 と整合する。
- 実 corpus での latency・recall は synthetic/fixture より不確実。P01/P02 で実 skills 環境での再計測を行う。
- Invocation recall・timeliness・成果物品質は未計測のまま残る。Claude Code 認証後に S02（#4）の観測項目を再検証し、必要なら follow-up Issue を起こす。
- O01（永続キャッシュ）・O02（MCP）・I01（標準化）は発動条件未達として deferred を維持する。

> 注記（2026-11-26 追記）: 本 ADR は MVP スコープの決定記録である。
> その後 O02（MCP）は v1.0.0 spec §1 / ADR-0005 で採用・実装済み、
> O01 / I01 は v1.0.0 spec §6 で不採用・対象外が確定した
> （`docs/backlog.md` の最終判定を参照）。

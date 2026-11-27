# ADR-0006: 多言語推薦のための skill description / keywords 記述規約

Date: 2026-11-27
Status: accepted (issue #184)

> 旧称注記: 本文中の `project-init` / `rebuildup/project-init` は旧称であり、
> 現行の canonical repository は `rebuildup/aude-skills`（改名追跡:
> rebuildup/aude-skills#212）。


## Context

#172 の holdout eval で、skill 名・description 語彙を含まない自然言語
query に対する recall@3 が 0.904 に低下した。miss の主因は
cross-lingual / 言い換えの語彙ギャップであり、admission の構造条件
（`docs/specs/core-contract.md`）ではなく「query と skill metadata の
間に共通語彙が存在しない」ことが根因だった。

検討した代替案:

- **Embedding / 意味検索**: v1.0.0 spec §6 で外部モデル依存を理由に
  不採用済み。採用する場合は別 ADR と runtime 依存の再評価が必要。
- **admission 閾値の緩和**（description 1 語一致での admit 等）:
  holdout では有効だが、一般 corpus での false positive を無制限に
  増やし、`forbidden_hits` と precision を悪化させる。却下。
- **stemming / 活用形正規化**: #184 で singular 形 variant の emit を
  試したが、対象 miss を解消できず forbidden_hits を 2→4 に悪化させた
  ため却下（証跡: `docs/evidence/2026-11-27-issue184-crosslingual.md`）。
- **組み込み glossary の拡張**: glossary は CJK→英語の方向のみ有効で、
  今回の miss は英語 query ↔ 和文/英語 description の言い換えであり、
  辞書では網羅できない。

## Decision

skill の推薦到達性は、description 本文と frontmatter `keywords`
（skill-graph が description match text に連結する既存の opt-in
仕組み、`src/parser.rs`）で担保する。規約:

1. `description` はその skill が「いつ使うか」が分かる文とし、
   利用者が実際に使う言語・言い換えを `keywords` に列挙する。
   特に和文 description の skill には、英語 query から到達するための
   英語 alias を `keywords` に持たせる（逆も同様）。
2. `keywords` は corpus 内で希少な具体語を選ぶ。`the`/`new` のような
   汎用語は df 条件で弾かれるが、そもそも admission 証拠として弱い
   ため避ける。
3. skill 固有の語彙追加は、その skill を管理する upstream repo
   （project-init catalog 等）への提案を正本とする
   （`docs/specs/v0.3.0-integration-boundaries.md` の方針を維持）。
   本 repo の `.agents/skills/` は skills CLI 管理の canonical copy
   であり、upstream 反映前の暫定措置としてのみ直接編集し、
   `skills-lock.json` の hash との乖離は upstream merge で解消する
   前提とする。

## Consequences

- engine（`src/search.rs`）の admission 条件は変更しない。
  `keywords` は既に description match text・document frequency・
  context admission の対象であり、契約変更を伴わない。
- #184 では diagnosing-bugs / onboarding / quality-gate に英語
  `keywords` を、fixture の gamma-skill に `hotfix` を追加し、
  holdout recall@3 を 0.923 → 1.0 に改善した。
- 今後 skill を追加・更新する際、holdout eval で miss が出た場合の
  第一候補は `keywords` の語彙補完であり、admission 緩和ではない。
- upstream（rebuildup/project-init）へ同内容の keywords 追加を
  Issue #203 として提案済み。upstream merge 後は `bunx skills update` で
  正本へ反映され、暫定編集は差分なしに解消される想定。

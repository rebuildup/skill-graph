# ADR-0007: skill-graph は optional tool であり、provisioning 失敗を agent 起動へ波及させない。native Windows は非対応を維持する

Date: 2026-11-28
Status: accepted (issue #315)

Related: rebuildup/pc-setup#259, rebuildup/pc-setup#241,
`docs/specs/v0.3.0-integration-boundaries.md`（4者責務境界）、
`docs/specs/v1.0.0.md`「非目標」

## Context

2026-11-28、native Windows 11 / mise 2026.11.7 の実機で、pc-setup の
machine-global `mise.global.toml` に登録された `github:rebuildup/skill-graph`
が `No matching asset found for platform windows-x64` で `mise install`
全体を失敗させ、mise shim 経由で起動する `claude` まで exit 1 で起動
不能になった（rebuildup/pc-setup#259、同一原因の upstream 追跡が本
issue #315）。v0.3.0 の責務境界は「binary provisioning は pc-setup、
adapter 有効化は user opt-in、相互に必須依存にしない」を定めていたが、
**provisioning 段階の失敗が独立ツールの起動を阻害する経路**までは
カバーしていなかった。

今回の障害で2つの事実が確定した。

- **mise shim の auto-install が障害増幅器になる。** `claude` 等の
  machine-global tool は shim 起動時に未導入 tool の install を試行する。
  global config 内の1エントリの解決失敗が、要求した tool とは無関係な
  全 shim 呼び出しを道連れにする。したがって「hook の fail-open」だけ
  では不十分で、**起動前の provisioning / auto-install 経路**が境界の
  対象でなければならない。
- **native Windows の実利用が存在する。** v1.0.0 spec 非目標の
  「Windows native 対応（WSL が対象）」は WSL 利用を前提としていたが、
  maintainer は native Windows 上でも Claude Code を日常的に使っており、
  非対応を明示するだけでは実害を防げないことが実機で証明された。
  対応範囲の再評価が必要になった。

## Decision

### 1. optional tool 境界の拡張

skill-graph は **optional tool** であり、その不在・導入失敗・非対応
OS・無効化状態は、既存 agent や他ツールの起動・導入を阻害しては
ならない。この境界は次の3層で成立させる。

- **provisioning（pc-setup 等の machine baseline）**: skill-graph の
  install エントリは OS 条件等により対象 OS を限定し、解決不能な
  platform では install 対象にしない。optional tool を他ツールの
  起動前提にする構成（必須依存・失敗を伝播する auto-install 経路）
  を禁止する。検証は hook の unit test ではなく、実際の
  `mise install` / shim 起動経路で行う。
- **activation（adapter / hook / plugin / MCP 登録）**: 従来通り
  user/project の opt-in。provisioning が binary を置くだけでは何も
  有効化されない。
- **runtime（skill-graph 本体）**: hook/plugin/MCP は fail-open を
  維持し、binary 不在時に agent 本体の動作を妨げない。

pc-setup 側の具体修正（`os = ["linux", "macos"]` 付与・CI 契約固定・
lock 再生成）は rebuildup/pc-setup#259 / PR #260 が正本であり、本
ADR は upstream 側の契約を固定する。

### 2. native Windows 対応: 非対応を維持する

実利用の存在を踏まえて再評価した結果、**v0.5.0 / v1.0.0 でも native
Windows 対応は非目標を維持**する。Windows での skill-graph 利用経路は
WSL とする。

- skill-graph の価値は skill discovery / recommendation であり、
  native Windows でこれが使えないことは agent 本体の起動を阻害しない
  （本 ADR §1 の境界が前提）。
- Windows 対応には release asset 追加に加え、adapter/hook の Windows
  での動作、state/safety 契約、CI matrix の拡張が必要であり、
  v0.5.0 blocker である本件の復旧とは独立した別規模の作業になる。
- 再評価の trigger: (a) Windows ネイティブ環境で skill-graph の
  adapter/MCP を必須とする利用が発生した場合、(b) Windows 向け
  release asset の需要が pc-setup 以外からも確認された場合、
  (c) WSL では代替できない Windows 固有の skill 配置が標準化した
  場合。その際は配布・native 実行・adapter・状態/安全性・CI matrix
  の契約全体を対象とする新規 ADR で採否を決める。

## Consequences

- machine-global baseline に optional tool を載せる際の os 条件・
  失敗分離は pc-setup#259 の CI 契約で機械的に固定される。
- 「非対応 OS で install 対象にならず、かつ他ツールの起動を阻害
  しない」ことが #315 の受入条件であり、Windows 実機での
  `mise install` / `claude` 起動成功がその証拠となる
  （`docs/evidence/2026-11-28-315-windows-provisioning.md`）。
- native Windows 対応を将来採用する場合、本 ADR を supersede する
  新規 ADR が必要。

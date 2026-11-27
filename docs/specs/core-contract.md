# Core Contract Spec

## Compatibility

- 標準 Agent Skills を原則として変更せず解析できる。
- 独自 frontmatter、manifest、link syntax を必須にしない。
- Progressive Disclosure を維持し、全 `SKILL.md` 本文を常時読み込まない。
- 導入済み、利用可能、読み込み済み、適用済み、観測不能を区別する。
- 同名スキル、異なる配布元、異なる revision を誤って同一視しない。
- 明示的依存宣言と、単なる関連リンクを区別する。

## Environment non-pollution

- 対象プロジェクト内に cache、log、management file を生成しない。
- 既存の Agent Skills、設定ファイル、session log を無断で変更しない。
- グローバル PATH 登録、shell 起動ファイル変更を必須にしない。
- plugin に実行機能を同梱しても、global CLI install を要求しない。
- 完全な no-save mode を提供する。
- 常駐 process を必須にしない。
- Hook は明示的に導入・有効化でき、利用者が選択できる。

## Security

- スキル文書を信頼できない解析データとして扱う。
- 解析対象の script を実行しない。
- 探索範囲外への path traversal と symlink 越境を防ぐ。
- Discovery は symlink 化した `SKILL.md` と symlink directory を辿らず、Markdown link resolution は発見済み skill root 配下に限定する。
- 外部 link を無断取得しない。
- 外部 API / network を中核機能の必須条件にしない。
- ユーザープロンプト、会話全文、秘密情報を永続保存・外部送信しない。
- エージェントの既存権限・承認・sandbox・Hook 信頼境界を迂回しない。

## Recommendation quality

- 推薦候補がなければ無出力とする。
- 推薦理由と観測根拠を提示する。
- 人気集中だけを強化する algorithm にしない。
- 不要な skill を大量に読み込ませない。
- 推薦処理の障害で通常開発を停止させない。
- skill の読み込みを成果物品質の保証として扱わない。
- 推薦 admission は direct relevance または explicit `requires` dependency に限定する。direct relevance は以下のいずれかとする（`src/search.rs` の `directly_admitted` + exact-name admission）:
  - query が skill name に完全一致（exact name。parse-error skill は対象外）
  - query term が skill name に 2 語以上一致、または 1 語一致かつ corpus 内で低頻度（df ≤ max(2, corpus の 10%)）かつ 4 文字以上
  - name hit 1 語以上 + description hit 2 語以上（name が identity を anchor し description が主題適合を確認する `name_plus_description`）
  - description hit 2 語以上、うち少なくとも 1 語が低頻度、かつ hit が **2 つ以上の独立した provenance source** から来ること（単一の CJK evidence unit が出す bigram cluster + glossary 展開を独立証拠と数えない）
  - context term が description に 2 語以上一致し、同様に 2 つ以上の独立 source から来ること
  - 加えて、admitted skill の explicit `requires` link 先は necessity として伝播 admit される（parse-error skill は seed にも target にもならない）。
  document frequency は name + description を対象に計算するが、description 側には frontmatter の `keywords` と組み込み glossary による CJK→英語展開（`expand_gloss`）を含む（`src/search.rs` の `document_frequency`）。Markdown link・名前言及・backlink・body text・link target の一致は relatedness / weak evidence であり、単独では推薦しない。
  term 抽出の対象 script は ASCII 英数字と CJK（ひらがな・カタカナ・漢字・ハングル）のみ。それ以外の文字体系（キリル・アラビア・ギリシャ・アクセント付きラテン等）は現行実装では term を生成しない既知の境界であり、対応が必要になった場合は Unicode 対応の tokenization を別途設計する。

## Storage modes

| Mode | 初期方針 |
|---|---|
| Stateless | 既定。メモリのみで解析・推薦。 |
| Session-scoped | 必要時のみ、runtime 管理領域または OS 一時領域に最小状態を保持。保存内容・境界・期限・削除方法を定義してから導入。 |
| Cached | on-memory benchmark で必要性が確認されるまで実装しない。 |
| Instrumented | 明示的に有効化した評価・統計記録のみ。 |

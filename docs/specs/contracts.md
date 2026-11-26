# Machine-readable Contracts (v1)

JSON Schema の正本は `contracts/v1/` に置く。CLI の `--format json` 出力はこれらに従う。

| Schema | 対象 |
|---|---|
| `skill-summary.schema.json` | discovery / resolve / recommend が返す skill の公開 summary |
| `discovery-output.schema.json` | `discover --format json` |
| `recommendation.schema.json` | `recommend` / `search --format json` |
| `resolve-result.schema.json` | `resolve --format json` |
| `skill-usage-event.schema.json` | hook/plugin が発行し `observe` が正規化する Skill Usage Event（JSONL, v1） |

## Skill Usage Event（v1）

`kind` は `prompt_submit` / `recommendation` / `skill_load` の3種。prompt 本文・
コマンド・tool response は保存せず、prompt は `prompt_hash`（SHA-256 hex）のみ。

`skill_load.detection` は観測の強さを表す:

| detection | 意味 |
|---|---|
| `native_invocation` | ランタイム標準の skill 起動機構が発火（Claude `Skill` tool 等） |
| `file_read` | SKILL.md 本文の読み取りを内容（frontmatter 等）で裏付け |
| `path_reference` | read 系アクセスが SKILL.md パスを参照したが内容未検証 |
| `inferred` | 弱いヒューリスティック（例: OpenCode `prompt.skills` 選択） |
| `mention` | 読み取り証拠のない名前・パスへの言及。load 集計から除外 |
| `applied` | 適用確認。現行 emitter は未使用（予約） |

`observed_via` は `hook` / `plugin` / `transcript` / `db`。emit 元は
`${TMPDIR}/skill-graph-events.jsonl`（`SKILL_GRAPH_EVENT_LOG` で変更可）に
JSONL で追記し、`observe <path>` が contract 形式の行を検証付きで
パススルー集計する。旧 `SKILL_GRAPH_HOOK_LOG` の独自形式ログは廃止
（`${TMPDIR}` 配下の揮発領域のみで永続データなし。移行: 新しい
`SKILL_GRAPH_EVENT_LOG` を参照）。

## 状態区分

- `availability` は discovery 時点の `available` / `parse_error` のみを表す。
- installed / available / loaded / applied / unobservable の区別のうち、loaded・applied・観測不能は runtime 側の観測が必要であり、この v1 contract では `availability` に混ぜない。

## 曖昧性と no-output

- `ResolveResult` は `name` / `path` / `unresolved` の tagged union。同名 skill が複数ある場合は `kind=name` かつ `ambiguous=true` とし、勝手に一意化しない。
- `recommend` は候補がなければ空配列 `[]` を返す。推薦理由と観測根拠は各要素の `reasons` / `observation` に必須で含める。
- cache・MCP・高度な graph algorithm はこの contract の対象外。

# Machine-readable Contracts (v1)

JSON Schema の正本は `contracts/v1/` に置く。CLI の `--format json` 出力はこれらに従う。

| Schema | 対象 |
|---|---|
| `skill-summary.schema.json` | discovery / resolve / recommend が返す skill の公開 summary |
| `discovery-output.schema.json` | `discover --format json` |
| `recommendation.schema.json` | `recommend` / `search --format json` |
| `resolve-result.schema.json` | `resolve --format json` |
| `skill-usage-event.schema.json` | hook/plugin が発行し `observe` が正規化する Skill Usage Event（JSONL, v1） |

## MCP tool surface（`skill-graph mcp`、ADR-0005）

MCP server は read-only 3 tools を公開し、戻り値は対応する CLI
`--format json` 出力と同一の JSON shape を `structuredContent` と
text content の両方で返す。

| MCP tool | 対応 CLI | 返す contract |
|---|---|---|
| `recommend` | `recommend --format json` | `recommendation.schema.json`（recommendation 配列） |
| `discover` | `discover --format json` | `discovery-output.schema.json` |
| `resolve` | `resolve --format json` | `resolve-result.schema.json` |

引数 `roots` は tool ごとに optional。未指定時は `SKILL_GRAPH_ROOTS`
→ 既定 root の順で解決する（hook/plugin と同一規則）。tool は event
log・dedup state・設定を変更せず、Skill Usage Event を発行しない
（`observed_via` に `mcp` は存在しない）。

## observe 出力の扱い

- `observe <path>`（`--summary` なし）の出力は `Observation` envelope
  （files_scanned / sessions / skill_loads / loads / runtimes / errors
  等）であり、`contracts/v1/` のいずれの schema にも従わない
  **contract 対象外（exempt）**。ただし envelope 内の `events[]`
  要素は `skill-usage-event.schema.json` の contract 形式の
  Skill Usage Event である。
- `observe <path> --summary` の出力は delivery / conversion /
  unnecessary-load / timeliness / context consumption 等の **集計指標**
  （runtime 別 `by_runtime` 内訳つき）であり、同様に schema 対象外の
  派生出力。集計 shape は観測パイロット指標
  （`docs/specs/v0.3.0-observation-pilot.md`）に対応し、公開 contract
  として schema 化する場合は別途 contract 改訂が必要。
- event log の JSONL 行は `kind` / `schema_version` を持つ行のみ
  contract イベントとして deserialize され、`schema_version` が
  `SCHEMA_VERSION` と一致しない行は `lines_skipped` に計上される。

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
JSONL で追記し、`observe <path>` が contract 形式の行を deserialize
検証（`kind`/`schema_version` 確認・`SCHEMA_VERSION` 一致）したうえで
パススルー集計する（詳細は上記「observe 出力の扱い」）。旧
`SKILL_GRAPH_HOOK_LOG` の独自形式ログは廃止
（`${TMPDIR}` 配下の揮発領域のみで永続データなし。移行: 新しい
`SKILL_GRAPH_EVENT_LOG` を参照）。

## 状態区分

- `availability` は discovery 時点の `available` / `parse_error` のみを表す。
- installed / available / loaded / applied / unobservable の区別のうち、loaded・applied・観測不能は runtime 側の観測が必要であり、この v1 contract では `availability` に混ぜない。

## 曖昧性と no-output

- `ResolveResult` は `name` / `path` / `unresolved` の tagged union。同名 skill が複数ある場合は `kind=name` かつ `ambiguous=true` とし、勝手に一意化しない。
- `recommend` は候補がなければ空配列 `[]` を返す。推薦理由と観測根拠は各要素の `reasons` / `observation` に必須で含める。
- cache・高度な graph algorithm はこの contract の対象外。MCP tool surface は ADR-0005 / `docs/specs/v1.0.0.md` を参照。

## Skill root の解決

CLI・hook・MCP・plugin の全入口で同一規則を使う（`src/discovery.rs::resolve_roots`）:

1. `SKILL_GRAPH_ROOTS`（`:` 区切り、存在する dir のみ）— 設定されていれば常に最優先。operator が agent 供給の引数より優先されることを保証する（ADR-0005）。
2. 明示引数（CLI `--root`、MCP tool `roots`。MCP は 32 件上限）
3. 呼び出し側の既定 root（CLI/plugin 既定、hook は runtime 別 fallback）

`discover` は canonical 化した SKILL.md パスで重複を除くため、包含・重複する root を渡しても同一 skill を二重計上しない。

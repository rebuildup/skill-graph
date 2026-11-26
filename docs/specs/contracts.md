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
- passthrough event の `session_id` / `skill_path` / `tool` /
  `skill_name` は emit 側 constructor と同じ上限に正規化される
  （`session_id` は `[A-Za-z0-9._-]`・128 文字、`skill_path` 1024、
  `tool`/`skill_name` 256 文字）。`recommended[].name` は schema の
  `minLength: 1` より厳しく valid skill name を要求する — schema 適合でも
  観測側の join キーにならない名前は `lines_skipped` に計上される。
- `detect` は 1 payload あたり最大 64 event を emit する
  （`MAX_EVENTS_PER_PAYLOAD`）。超過分は打ち切られ、通知はない。
- dedup event の記録は adapter 固有: Rust hook は repeat を
  `emitted:false, reason:"dedup_skip"` の recommendation event として
  残すが、OpenCode plugin は dedup 時に event を出さない。`reason`
  語彙と dedup event の有無は runtime 間で一致せず、delivery_rate の
  分母構造が異なる（pilot spec は runtime 間比較を前提としない）。
  plugin の `emitted` は injection キュー投入時点の値であり、context
  hook での実注入までは保証しない（pending FIFO eviction で未注入に
  なりうる）。
- `observe --summary` の `by_session` は `(runtime, session_id)` キーの
  辞書順先頭 500 session まで。`collection_errors` / `errors` /
  `errors_omitted` / `by_session_omitted` で打ち切り・収集失敗を通知する。

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

## hook/plugin の共有 TMPDIR 書き込み安全契約

イベントログ・dedup marker・session state は共有 TMPDIR 配下に置かれうるため、
Rust hook（`src/hook.rs`）と OpenCode plugin（`src/assets/opencode-plugin.ts`）は
同一の安全契約に従う（issue #169、監査 D-01/D-02）:

- open は `O_NOFOLLOW | O_NONBLOCK`（TS は `O_NOFOLLOW`）+ 作成時 mode `0600`。
  symlink・FIFO・socket は open 段階で拒否され、FIFO による hook 停止を防ぐ。
- ローテーション前のサイズ判定は symlink を辿らない
  （Rust: `symlink_metadata`、TS: `lstatSync`）。symlink は rename しない。
- open 済み fd の metadata で `uid == euid`・regular file を検証し、
  書き込み対象はさらに `nlink == 1` を要求する。不一致・非 regular・
  hardlink は fail-open で書き込まない（plugin は uid 検査のみ）。
- 権限 tightening は fd ベース（`fchmod` / `File::set_permissions`）のみ。
  path ベースの chmod は TOCTOU で symlink 先を変更しうるため禁止。
- 読み取り側も同じ fd 検査を通し、信用できない state は空として扱う。
- `SKILL_GRAPH_HOOK_NOSTATE=1` は event log・marker・session state・
  debug 出力を含む一切の書き込みを抑止する。debug 出力は stderr
  （`console.error`）のみで、ファイルへは書かない。

## 状態区分

- `availability` は discovery 時点の `available` / `parse_error` のみを表す。
- installed / available / loaded / applied / unobservable の区別のうち、loaded・applied・観測不能は runtime 側の観測が必要であり、この v1 contract では `availability` に混ぜない。

## 曖昧性と no-output

- `ResolveResult` は `name` / `path` / `unresolved` の tagged union。同名 skill が複数ある場合は `kind=name` かつ `ambiguous=true` とし、勝手に一意化しない。
- `recommend` は候補がなければ空配列 `[]` を返す。推薦理由と観測根拠は各要素の `reasons` / `observation` に必須で含める。
- cache・高度な graph algorithm はこの contract の対象外。MCP tool surface は ADR-0005 / `docs/specs/v1.0.0.md` を参照。

## Skill root の解決

CLI・hook・MCP・plugin の全入口で同一規則を使う（`src/discovery.rs::resolve_roots`）:

1. `SKILL_GRAPH_ROOTS`（プラットフォームの path-list 区切り）— **非空値が設定されていれば常に最優先**で、設定値が探索範囲の全体を定義する。存在しない・ファイル・権限不足の entry は `discover` の `errors` で診断され、有効 entry のみが走査される。**有効 entry が 0 件でも explicit 引数や既定 root へは拡大しない**（空の走査 + 診断に縮退する）。空文字列は未設定と同等。非 UTF-8 値も `var_os` で「設定済み」と判定される。operator が agent 供給の引数より優先されることを保証する（ADR-0005）。
2. 明示引数（CLI `--root`、MCP tool `roots`。MCP は 32 件上限）— env 未設定時のみ
3. 呼び出し側の既定 root（CLI/plugin 既定、hook は runtime 別 fallback）— env 未設定かつ明示引数なしのときのみ

`discover` は canonical 化した SKILL.md パスで重複を除くため、包含・重複する root を渡しても同一 skill を二重計上しない。

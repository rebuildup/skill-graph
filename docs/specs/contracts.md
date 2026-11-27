# Machine-readable Contracts (v1)

JSON Schema の正本は `contracts/v1/` に置く。CLI の `--format json` 出力はこれらに従う。

| Schema | 対象 |
|---|---|
| `skill-summary.schema.json` | discovery / resolve / recommend が返す skill の公開 summary |
| `discovery-output.schema.json` | `discover --format json` |
| `recommendation.schema.json` | `recommend` / `search --format json` |
| `resolve-result.schema.json` | `resolve --format json` |
| `skill-usage-event.schema.json` | hook/plugin が発行し `observe` が正規化する Skill Usage Event（JSONL, v1） |

## 横断的な不変条件（#144 監査の構造的要件）

22 ラウンドの監査で反復発見された欠陥クラスを構造的に排除するため、
全実装が従う 3 つの不変条件をここに明文化する。監査は個別 bound の
有無ではなく、これらの不変条件が全入力経路で成立するかを検査する。

1. **保持量の有限性** — 外部入力 (ファイル、stdin、JSON payload、
   transcript、plugin event、dataset) から保持する全 collection
   (Vec/HashMap/String/PathBuf) は byte 数・要素数のいずれかの上限を
   持つ。上限超過時は detail を捨てて counter/診断のみ残すか、入力を
   reject する。retained 推定は実際の割当を過小評価してはならない
   (clone・展開・前処理を含む保守的見積)。
2. **charge-before-work** — 入力サイズに比例する仕事 (走査・比較・
   分類・正規化・直列化) は、実行前に保守的な上限を work/byte
   budget へ課金する。cap が保持量にしか効かず、保持ゼロのまま
   CPU だけ消費する経路を残してはならない。
3. **順序保存 truncation** — 件数・byte 上限による打ち切りは、定義
   された順序 (observe では feed-line seq、その他では決定的 sort)
   の真の prefix を emit する。category 順・発見順など入力順と
   異なる順での打ち切りは、優先順位の偏りを生むため禁止。
   打ち切り後の counter は全入力を集計し続ける (detail のみ欠落)。

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
→ 既定 root の順で解決する（hook/plugin と同一規則）。`roots` を
明示した場合は配列であることが必須（非配列は `invalid_params`）。
配列が存在する場合、その配列が探索範囲を定義する: `[]` や全要素が
非文字列・32 件超過で落ちた場合でも既定 root へは拡大せず、
診断付きの空の走査に縮退する（#177 と同一規則）。tool は event
log・dedup state・設定を変更せず、Skill Usage Event を発行しない
（`observed_via` に `mcp` は存在しない）。tool 計算は 1 件ずつ
直列化される: client が call を pipeline しても同時に走る graph
構築は 1 つまでで、待機は async 側で行うため blocking pool を
消費しない（#144 r19）。待機 queue は持たない: permit は
`try_acquire` で取り、計算中の call がある間に届いた pipelined
call は `internal_error` ("skill-graph is busy") で即 reject する。
各 handler task は parse 済みの引数 `JsonObject` を保持するため、
queue 無制限は rmcp の 16 MiB/line 上限を Value 増幅 (~16-30×)
で迂回する OOM 経路になる（#144 R23）。逐次 request/response
の client は影響を受けない。引数 tree は clone せず blocking task
へ move する。

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
- passthrough event の payload 由来文字列フィールドは emit 側
  constructor と同じ上限に正規化される: `session_id` は
  `[A-Za-z0-9._-]`・128 文字。canonical な id（全て valid charset・
  128 文字以内・generated 形状でない）は素通しし、それ以外は
  valid 文字の先頭 95 文字 + `-` + 元 id の SHA-256 先頭 32 hex
  （計 ≤128 文字）に変換される。generated 形状（末尾が `-` +
  32 桁 lowercase hex）は reserved であり literal id としても
  変換対象になるため、passthrough 名前空間と生成名前空間は
  構造的に衝突しない（#225 r5: 旧 8 hex 接尾辞では literal id が
  別 id の変換結果と一致し得た）。charset フィルタで文字が
  除去された id・128 文字超の id・全滅 id（`u-<hash>`）はいずれも
  元 id の hash を含むため merge されない（`sess!1` と `sess?1`、
  および `sess!1` の変換結果に等しい literal id は別 id）。
  冪等性は契約ではない: charset 損失を伴う写像では冪等性と
  単射性は両立しないため、sanitize は ingest で一度だけ適用する。
  `skill_path` 1024、`tool`/`skill_name`/`recommended[].name` 256、
  `reason` 1024、`prompt_hash`/`skill_digest`/`recommended[].digest` 128、
  `timestamp` 64 文字。`recommended` は最大 32 件
  （`MAX_RECOMMENDED`）で打ち切られる。`skill_load.skill_name` は
  `recommended[].name` と同じ valid skill name 規則を要求し、違反行は
  `lines_skipped` に計上される（constructor が emit し得ない名前を
  funnel join キーに流入させない）。OpenCode plugin の `safeSessionID` は同一規則を
  実装し、`src/assets/opencode-plugin.test.ts` が parity を検証する。
  `recommended[].name` は
  schema の `minLength: 1` より厳しく valid skill name を要求する —
  schema 適合でも観測側の join キーにならない名前は `lines_skipped`
  に計上される。`recommend_ms` は schema 上 `integer` だが観測側は
  `u64` のみ受理し、負値・`u64` 超過は `lines_skipped` に計上される
  （schema より厳しい reject はこの項の定義済み動作）。
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
- `SKILL_GRAPH_HOOK_NOSTATE=1` の dedup 意味論は adapter 間で異なる:
  Rust hook は state marker を無効化し repeat prompt 毎に recommend を
  再実行するが、OpenCode plugin の in-memory dedup map は NOSTATE 下でも
  有効で、repeat `(session, prompt)` と skill-load 依存パスを抑制し続ける。
  NOSTATE は永続化の抑止であり、plugin のメモリ内重複排除は残る。
- `observe --summary` の `by_session` は `(runtime, session_id)` キーの
  辞書順先頭 500 session まで。`collection_errors` / `errors` /
  `errors_omitted` / `by_session_omitted` で打ち切り・収集失敗を通知する。
- 集計の分母規則: `unnecessary_load_rate_pct` は session 帰属 load
  （`session_attributed_loads`）のみを分母・分子の対象とし、session id を
  持たない unattributable load は率から除外される（`load_grade_loads` との差が
  帰属不能分）。`loads_per_session` は全 load-grade load を session 数で
  割る top-level 指標であり、帰属不能 load を含む。
  `context_consumption.emitted_per_session_*` の分母は emitted
  recommendation を 1 件以上持つ session のみ（全 session ではない）。
  `Observation.sessions` は生の session id の distinct 数であり、
  funnel の `(runtime, session_id)` 集計とは sanitize 差異で一致しない
  ことがある（best-effort count）。
- `observe` の保持量には集約上限がある: 入力ファイル数 4096、
  ディレクトリ再帰深さ 64、走査ディレクトリ数 65,536・
  ディレクトリエントリ総数 1,000,000、走査エラー 1024 件
  （収集・ファイル毎の双方を含む。latched な打ち切り通知
  数件を別途含みうる）、ファイル毎 256 MiB
  （`.db` は `-wal` 込み）、JSONL 行・`.db` 行の `data` ペイロード毎
  4 MiB（文字数・UTF-8 byte 数の双方で判定。`session_id`/`type` 列も
  substr + length で同上限を適用し、超過行は skip する。
  `serde_json::Value` は入力の ~16-30 倍を占めるため、
  ファイル上限だけでは 1 行で数 GB を確保しうる）、retained
  events/loads 各 1,000,000、retained detail 合計 64 MiB
  （イベント/ロードの推定バイト数 —
  count 上限だけでは contract 適合の巨大イベントでメモリを
  尽くしうるため。`skill_load.file` は走査後に backfill されるため、
  走査中のファイルパス長を事前に課金して過小計上を防ぐ）。
  `sessions` 集計は生の session id を保持せず hash のみを保持し、
  cardinality 1,000,000 で打ち切る。`skill_loads` の name-keyed
  カウンタは 65,536 件で打ち切り、以降の新規 name は計上しない
  （既存 name の加算は継続）。
  observables として受理するのは regular file のみで、FIFO・
  device・socket は拡張子に関わらず走査対象外（FIFO の
  `read_to_string` による永久ブロックを防ぐ）。JSONL の読み取りは
  `O_NOFOLLOW | O_NONBLOCK` + fd 再検証 + `take()` 上限で行い、
  収集時検査との間の FIFO 差し替え・サイズ超過を防ぐ。Codex
  rollout の pending access 集約はファイル毎 65,536 件までで、
  超過分は path を持たない compact record（`MAX_OVERFLOW_ACCESSES`
  65,536 件）として counters/funnel の正確さを保ったまま保持する。
  いずれの打ち切りも `errors` に latched 通知が入り、
  `runtimes`/`skill_loads` の集計カウンタは正確なまま維持される
  （`skill_loads` の name cardinality 打ち切りを除く）。
  なお走査中は canonical 化した dir path（最大 ~4 KiB ×
  65,536 dir ≒ worst case ~270 MB）を visited set として保持する
  — 256 MiB ファイル上限と同程度の transient であり、dir 数上限が
  そのまま memory 暗黙上限になる点は既知の residual。
  transcript scanner（Claude/Codex）が finish まで保持する
  record にも count+byte の共有予算がある（`MAX_SCAN_ITEMS`
  65,536・`MAX_SCAN_BYTES` 64 MiB/ scanner）: tool_use input は
  `value_bytes` 推定で課金（node overhead を多めに見積もるが
  あくまで推定であり、実保持量は名目値の数倍になりうる）、
  tool output は先頭 256 KiB のみ保持
  （frontmatter 検証は head のみを読む）、session_id は ingest で
  sanitize・timestamp は 64 chars・call_id は 1,024 bytes・
  access path は 4,096 bytes に丸め、Codex の
  PendingCall/OverflowAccess が保持する session/timestamp/call_id
  は `Rc<str>` で共有する（1 record あたりの複製を O(1) にする）。
  予算超過分は drop され `errors` に件数が通知される。
  `.db` の open も JSONL と同じ fd 検証（`O_NOFOLLOW|O_NONBLOCK` +
  fstat で regular file・256 MiB 以下）を SQLite open の直前に
  行うが、検査と rusqlite の path open の間の差し替え窓は残る
  （fd ベースの SQLite open は `-wal` sibling を解決できないため
  採用しない。project config の TOCTOU residual と同じ受容区分）。
  canonical 化した JSONL path も同じ区分: `O_NOFOLLOW` は最終
  component のみを守り、収集後に directory component が symlink
  へ差し替えられる窓は残る（`.db` と同じ受容 TOCTOU）。
  hardlink で `.jsonl` と `.db` が同一 inode を指す場合、format
  判定は traversal が先に見た名前の拡張子に依存する
  （1 file は 1 形式として走査される）。
  `skill-graph detect` の stdin も hook と同じ 4 MiB parse 前
  閾値と bounded drain を持つ。OpenCode plugin の
  `prompt.skills` 由来 skill_load は 1 prompt あたり 1,024 entry・
  session あたり 4,096 name で打ち切る。plugin が扱う外部 payload
  には上限がある（全て UTF-8 byte 数で計測 — UTF-16 code unit
  数では multibyte 文字を過小計上する）: `resultText` は 64 KiB
  までに code point を壊さず切り詰めて保持（join の改行分も
  課金）、`tool_input` の JSON parse は 1 MiB 以下のみ（超過は
  event 化を skip）、`skill-graph` への spawn は stdin payload が
  1 MiB 以下の時のみ（fail-open: 上限超過時は注入なしで継続）、
  `prompt.text` は 1 MiB に切り詰めて hash/dedup/query に使用、
  `detect` 子プロセスの stdout から parse する event は
  1 payload あたり 64 件まで（Rust 側 `MAX_EVENTS_PER_PAYLOAD`
  と同じ）。pending 注入キューは session あたり 64 entry で
  打ち切る。session id を帰属不能（sanitize 結果が空）とする
  prompt/tool event は per-session state を一切共有しない:
  dedup・recommendation queue・session continuity（`sessionCtx`）
  を bypass し、context hook は空 id で早期 return する
  （Rust hook の `state_disabled` と同じ契約 — 匿名 session 同士が
  状態を merge・漏洩しない）。`detect` spawn 予算のみ、帰属不能
  session 全体で共有する単一の fail-closed 上限を持つ
  （resource bound であり recommendation state ではない。
  枯渇時は匿名 session の detection のみ止まる）。
- `observe --summary` の conversion 集計は決定的に制限される:
  name 別 load リストの cursor で通常時はほぼ線形だが、timestamp
  欠落時の encounter-order fallback は rec×load の組合せ検査を
  最大 8,000,000 回で打ち切る。この予算は `summary()` の全
  funnel pass（全体・runtime 別・session 別）で共有され、pass 毎に
  リセットされない（超過後の rec は convert しない。
  rec 順序は固定なので結果は決定的）。timestamp を持たない rec が
  1 件でも混在する session では、timestamp による cursor kill を
  無効化する（encounter-order fallback が timestamp-dead 判定済みの
  load を位置的に eligible としうるため。消費済み load の kill は
  有効のまま）。timestamp を持たない rec の変換勝者は timestamp
  sort 位置ではなく encounter order で選ぶ（eligibility と同じ
  順序で「最初」を定義するため）。transcript 由来 event と
  contract-log passthrough event は file 内の feed line index を
  共有して merge され、`obs.events` は feed line 順を保持する
  （timestamp sort は `funnel_for` の下流 (timestamp, index) sort が
  担う）。timestamp が欠落・同値の場合も真の出現順が保たれ、
  untimestamped rec の `li > ri` fallback は encounter order を
  比較する。transcript scanner の emit は `emit_runs()` が生成する
  seq-ordered lazy run を `merge_emit_runs` が feed line index で
  k-way merge し、共有 budget（`MAX_OBSERVE_EVENTS` 件・
  `MAX_OBSERVE_RETAINED_BYTES` 推定 byte、両 scanner 合算）で
  feed 順に制限する。budget が最初に event を拒否した時点で
  `closed` latch が立ち、以降の event は全て detail を捨てて
  counter map のみ更新するため、emit される event 列は常に feed
  順の真の prefix になる（passthrough pending buffer も同一の
  `closed` latch 意味論: `PendingBuffer` が count+byte 予算の
  初回拒否で latch し、以降の event は counter のみ更新する）。
  tool_use 1 件あたりの classification 数も
  `MAX_EVENTS_PER_PAYLOAD`（64）で打ち切る（hook の per-payload
  cap と同じ上限）。1 file scan 中の transient 保持は pending
  buffer・両 scanner の内部状態・emit runs・`obs.retained` の各段が
  独立に `MAX_OBSERVE_EVENTS`/`MAX_OBSERVE_RETAINED_BYTES` または
  `MAX_SCAN_BYTES`（scanner 毎）で bound されるため、peak は推定
  ~320 MiB — 各段の予算は同一定数を再利用する受容 residual
  （#144 R25 LOW）。`--summary` の `session_events`/`funnel_for`
  は集計用の transient map（`runtime:session_id` キー等）を
  event 数比例で構築するため、最悪入力では retained
  `MAX_OBSERVE_RETAINED_BYTES` budget の数倍の peak memory に
  なる — CPU は共有 pair-check 予算で有限であり、memory 増幅は
  受容 residual として記録する（#144 R23 LOW）。
- `timeliness_ms` は RFC 3339 timestamp のミリ秒精度（小数秒は ms に
  丸め）で rec→load の差分を取る。offset は必須（`Z`/`z` または
  `±hh:mm`、コロン必須）で、小文字 `t`/`z` は RFC 3339 が許容する
  ため受理する。offset なし・コロンなし offset・暦上不正な値は
  "unknown time" 扱いで timeliness から除外される。

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

- open は `O_NOFOLLOW | O_NONBLOCK` + 作成時 mode `0600`（TS も同じ
  flag を渡す）。
  symlink・FIFO・socket は open 段階で拒否され、FIFO による hook 停止を防ぐ。
- ローテーション前のサイズ判定は symlink を辿らない
  （Rust: `symlink_metadata`、TS: `lstatSync`）。symlink は rename しない。
- open 済み fd の metadata で `uid == euid`・regular file を検証し、
  書き込み対象はさらに `nlink == 1` を要求する。不一致・非 regular・
  hardlink は fail-open で書き込まない（plugin も `nlink` を検査し
  Rust と parity を保つ）。
- Rust の state dir（`${TMPDIR}/skill-graph-hook-state`）は open 時に
  `O_NOFOLLOW|O_DIRECTORY` で dirfd を取得し euid 所有を検証したうえで、
  全 state file 操作を `openat`/`fstatat` でその fd 相対に行う。
  dir 自体の permission tightening も検証済み dirfd への `fchmod` で
  行い、dirfd 確立前後を通じて path 再解決に依存する操作を残さない。
  検査と操作の間に state dir エントリを差し替える競合は構造的に
  不可能（#225）。state file 名は `<runtime>-<session>` を含み、
  runtime 間・未知 session 間で dedup/recommended state を共有しない
  （session id が sanitize で空になる場合は state 自体を無効化）。
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

1. `SKILL_GRAPH_ROOTS`（プラットフォームの path-list 区切り）— **非空値が設定されていれば常に最優先**で、設定値が探索範囲の全体を定義する。存在しない・ファイル・権限不足の entry は `discover` の `errors` で診断され、有効 entry のみが走査される。**有効 entry が 0 件でも explicit 引数や既定 root へは拡大しない**（空の走査 + 診断に縮退する）。空文字列は未設定と同等。非 UTF-8 値も `var_os` で「設定済み」と判定される。operator が agent 供給の引数より優先されることを保証する（ADR-0005）。OpenCode plugin のみ `:` 区切りで split し、空 entry を drop する
（Rust `split_paths` は空要素を保持して診断に回すが、plugin は
`--root ""` を spawn できないため drop する。Windows は非対応の
ため実害なし）。
2. 明示引数（CLI `--root`、MCP tool `roots`。いずれも 32 件上限
   — CLI は超過分を捨てて stderr に通知、MCP は診断付きで drop）—
env 未設定時のみ
3. 呼び出し側の既定 root（CLI/plugin 既定、hook は runtime 別 fallback）— env 未設定かつ明示引数なしのときのみ

`discover` は canonical 化した SKILL.md パスで重複を除くため、包含・重複する root を渡しても同一 skill を二重計上しない。

## adapter の compose overlay（opt-in）

`skill-graph adapter` は管理対象 config（`~/.claude/settings.json` 等）
以外のファイルを既定では一切読み書きしない。dotfiles 管理の
`settings.local.json` のように、compose 時に user config を再 import
して古い skill-graph hook が復活しうる環境では、operator が
`SKILL_GRAPH_EXTRA_OVERLAY=<path>` を明示設定した場合に限り、
`enable` / `disable` / `status`（claude-code `--user`）がその overlay
内の skill-graph 所有 hook を除去・計上する。env 未設定時は overlay
機構は完全に無効であり、個人環境固有のパスはバイナリに埋め込まない
（#174 / I-04）。

`adapter status` の `broken` 判定は、config 内の全所有 v0.4.x hook
command が埋め込む bin パスについて存在・実行ビットを非実行で検査
する（#167: config 由来パスは信頼できない入力であり実行しない）。
`enabled` は「エントリ存在 + bin 存在/実行可能」のみを意味し、
bin のバージョン能力は status では未検証である。

bin 解決経路の非対称は既知の仕様: hook 系 runtime は enable 時に
`resolve_bin`（`--bin` → `SKILL_GRAPH_BIN` → PATH probe）で解決した
絶対パスを config に埋め込むのに対し、opencode plugin は実行時に
`SKILL_GRAPH_BIN` → PATH の順で解決する。enable 後に
`SKILL_GRAPH_BIN` を変更しても hook command の埋め込みパスは
変わらない（plugin のみ追従する）。`SKILL_GRAPH_BIN` が
path 形式（separator 含有）の相対値の場合、adapter の存在検査は
adapter 自身の cwd 基準、plugin の spawn は OpenCode server の
cwd 基準で解決するため、cwd が一致しない環境では診断と実行が
ずれうる（operator 制御の env であり既知の非対称）。
`SKILL_GRAPH_BIN` の空文字列は両側で未設定と同等に扱う。

## adapter/plugin の信頼できない入力境界

- adapter が読む config（`settings.json` / `opencode.json` /
  `hooks.json`）は regular file のみ受理し、`O_NONBLOCK` open +
  open 後 fstat で型を再検証、読み取りは 4 MiB 上限
  （`MAX_CONFIG_BYTES`）。FIFO・`/dev/zero` 等の symlink 先・
  巨大ファイルは `status`/`disable` 経路でも hang/OOM しない。
  parse は strict JSON（`serde_json`）であり、JSONC の `//`
  コメントや trailing comma を含む config は `invalid json`
  エラーになる — fail-closed で書き換えは起きないが、comment 入り
  config では adapter が使えない（書き込み側は常に strict JSON を
  出力し、comment は保存されない）。
- adapter の env 由来パス（`HOME`、`CODEX_HOME`、`XDG_CONFIG_HOME`、
  `XDG_DATA_HOME`、`SKILL_GRAPH_EXTRA_OVERLAY`）は絶対パスのみ有効。
  相対値は未設定として既定解決に縮退し、`file://` spec
  には絶対パスのみを使う。hook 側も同規則で `TMPDIR`・
  `SKILL_GRAPH_EVENT_LOG`・`SKILL_GRAPH_HOOK_STATE` の相対値を
  未設定として扱う（hook の cwd は project tree でありうるため）。
  OpenCode plugin 側も `SKILL_GRAPH_EVENT_LOG` と `tmpdir()` の
  戻り値に同じ absolute-only 規則を適用する（server の cwd は
  任意の project tree でありうるため）。
  なお非 UTF-8 の絶対パスは adapter の env について Unix の
  `var_os` 経由で受理される（path bytes を失わないため）。
  `file://` spec は Unix では path の raw bytes を percent-encode
  するため非 UTF-8 dir でも spec が実 path を指す。
  hook/plugin の env（`TMPDIR`・`SKILL_GRAPH_EVENT_LOG`・
  `SKILL_GRAPH_HOOK_STATE`）は UTF-8 のみ受理し、非 UTF-8 値は
  未設定として縮退する（hook は `var` の UTF-8 検証で、plugin は
  Node/Bun が非 UTF-8 byte を U+FFFD に decode するためその
  置換文字を含む値を拒否する。完全 parity は不可能 — plugin は
  valid UTF-8 の U+FFFD を含む正当な path も拒否しうるが、
  hook 側は `var` で受理するため、その環境では両入口の event log
  が分かれうる。raw-bytes env access がない以上の受容差異）。
  UTF-8 を
  要求するのは hook command へ埋め込む
  `--bin`/`SKILL_GRAPH_BIN`/`PATH` 解決結果のみで、
  その場合は `enable` がエラーを返す。
- legacy hook 検出の shell 解析は quote/escape を考慮する:
  `'FOO=bar'` や `FOO\=bar` のように quote・escape された `NAME=`
  は env assignment ではなく command word であり、foreign hook と
  誤認して削除しない。`env` コマンドの `name=value` operand は
  quote 除去後の text で判定する。`;`/`&&`/`||`/`|` 等の command
  separator は word 解析の対象外であり、compound command 内の
  hook 呼出しは所有権検出・除去の対象にならない（under-claim で
  あり foreign command を巻き込まない方向に倒す既知の制限）。
  同様に v0.4.x hook command の所有権検出は command word が
  literal first word であることを要求し、`FOO=bar` 前置や
  `env -u NAME` のような env 前置を持つ v0.4.x hook は認識
  しない（under-claim：既存 entry を残したまま新規追加しうる）。
  なお `'FOO'=bar` のように name を quote した代入は POSIX shell
  では有効な assignment だが、本 parser は command word とみなす
  （under-claim 方向）。path 修飾された `env`（`/usr/bin/env`）は
  file name 判定で認識するが、`nice env`/`command env` のような
  さらに外側の wrapper は対象外。
  v0.4.x hook command の parse は binary path の single/double
  quote と引数の quote を受理する。
- transcript/exec の read 判定 heuristic（`segment_has_read_verb`）
  は quote/escape を考慮しない: `'FOO=bar' cat x/SKILL.md` のように
  quote された `NAME=` を env assignment とみなして read verb を
  skip しうる。detection の over/under-claim 双方向の既知制限で
  あり、foreign 設定を壊す方向には働かない。
- plugin specifier の境界判定で、marker 直後の `%` は follower と
  みなさない（`file_url_encode` は marker 直後に raw `%` を出力
  せず、`%2F` 等の percent-encoding で path が継続しうるため）。
  逆に marker 判定は suffix 一致のため、`skill-graph/opencode-plugin`
  を含む別 prefix の spec（vendor 配下の copy 等）も我々の
  plugin とみなして enable/disable の対象になりうる。versioned
  path を literal 一致で判定できない設計上の trade-off であり、
  境界規則は `opencode-plugin-fork`・`my-skill-graph/…`・
  `%2F` 継続を正しく拒否する。
- project scope の enable/disable は write 直前に
  `check_project_symlink` を再検査するが、検査と rename の間の
  symlink 差し替え（TOCTOU）は完全には閉じない。影響は project
  ディレクトリ内 config の置換に限定される設計（dirfd pin や
  `openat2(RESOLVE_BENEATH)` は採用しない）。この残余 race は
  既知の制限として記録する。
- `adapter status`/`disable` は payload dir を env
  （XDG_DATA_HOME/HOME）から再解決せず、config 内 `file://` spec を
  percent-decode して解決する。enable 後に env が変わっても spec が
  指す実 payload を検査・削除するため、状態報告と削除対象が
  OpenCode の実挙動と一致する（#144 r17）。enable は config write
  失敗時に今回作成した payload のみを rollback する。
- payload 削除の対象は「managed layout」（絶対パス・`.`/`..`
  なし・`skill-graph/opencode-plugin` で終端）かつ canonicalize
  後に data-home base（現在の `XDG_DATA_HOME` または
  `~/.local/share` の canonical path）の配下にある dir に限定
  される。spec decode だけでは任意の `*/skill-graph/opencode-plugin`
  を削除対象にできない（#144 r19）。最終 component が symlink の
  場合は canonicalize した **parent** が managed parent
  （`<canonical base>/skill-graph`）に一致する時のみ link 自体を
  unlink し、target は削除しない — foreign dir の symlink も
  attacker-directed deletion になりうるため同じ base-membership
  gate を通す（#144 r22）。残余: enable
  当時の custom `XDG_DATA_HOME` が unset 等で base 集合から外れた
  payload は orphan として残る（削除を拒否する方向の fail-safe）。
- materialize される OpenCode plugin payload は XDG data dir 配下に
  置かれ、最終 component の symlink は拒否するが、中間 component
  （例: `$XDG_DATA_HOME/skill-graph` 自体の symlink）は user 所有
  領域の自己影響に留まるため祖先 walk の対象外（project config の
  ような他者が書き換えうる経路ではない）。
- plugin の `detect` spawn 予算（session 毎 512 回）は
  SKILL.md prefilter を通過した呼出しにのみ課金する —
  無関係な tool 実行が budget を消費して検出を無効化することはない。
  plugin が spawn する子プロセスの stdout は 4 MiB 上限で打ち切り、
  20 秒 timeout + drain 後 SIGKILL で回収する。

## discovery の保持量境界

- `discover` の byte 会計は pre-read の `metadata().len()` ではなく、
  parse 後の retained フィールド量（body/name/description/keywords/
  metadata key+value/links/path/digest/parse_error/root + 固定
  overhead）と file bytes の大きい方を課金する。
  `from_utf8_lossy` の UTF-8 置換拡大（最大 ~3x）と metadata〜read 間の
  file 成長による過小計上を防ぐ。
- この 512 MiB 境界は **discovery phase の `Skill` データのみ**を
  対象とする。Graph phase（`resolver::build`）で追加される保持量は
  別の global budget（`MAX_RESOLVE_BYTES` = 128 MiB）で bound する:
  link 解決の `resolved_name`/`resolved_path` clone と skill 毎最大
  64 件の derived Mention link は discovery の per-link 課金を
  通らないため、budget 尽きた場合は link を未解決のまま残す
  （backlink/requires の証拠が落ちるだけの graceful degradation）。
  backlinks は name-resolved link を name 毎・path-resolved link を
  target 毎の source 集合として集約し、最終的には target 毎の
  dedup 済み source **件数**のみを保持する（消費側は件数のみ使用）。
  同名 copy への fan-out は発生せず O(links + skills) に留まる
  （#144 r18）。MCP `recommend`/`resolve` は call 毎にこの Graph を
  再構築するため、実メモリは名目値を超えうる既知の residual として
  記録する（`discover` tool は resolver を通らずこの上乗せを持たない）。
- WalkDir は `sort_by_file_name` を使わない: per-directory の全
  dentry を buffer してから yield するため、敵対 corpus の巨大な
  単一 directory で entry cap 到達前に memory を枯渇させうる
  （#144 r19）。代わりに entry を stream し SKILL.md candidate のみ
  bounded に collect、path component 単位の sort で
  `sort_by_file_name` と同一の DFS 順序を再現する。entry cap
  （100,000）超過時の candidate **集合**は filesystem 順序依存に
  なる — cap 超過 corpus は既に truncated 報告される受容 residual。
- discovery の `errors` は収集時点で 1024 件（`MAX_DISCOVER_ERRORS`）に
  制限され、打ち切りは latched notice で通知される。parse 失敗は
  skill/byte cap を消費しないが、error エントリ自体は bounded。
  MCP `discover` は全 error を collect してから truncate せず、
  残り枠分のみを保持する。
- frontmatter `metadata` map は信頼できない入力として bounded:
  128 entry・value 1024 文字・合計 8,192 文字まで。retained byte
  会計には metadata の key+value も含まれる。打ち切りの生存 key は
  `requires-skills`/`requires_skills` を優先し・残りを key sort で
  決定的に選ぶ（HashMap 反復順は process 毎に random なため、
  requires-link の admit 結果が run 間で変わらないようにする）。
- frontmatter に YAML alias（`*name`）を 1 件でも含む document は
  parse_error として reject する。serde_yaml は alias を anchor
  subtree の event 再走査で展開し、その回数制限は jump 数のみを
  bound するため、flat な anchor/alias 対で ~10^10 node の materialize
  （OOM）に至りうる（#144 r17）。anchor 定義のみ・alias 非含有の
  document は影響を受けない。判定は serde_yaml と同一の libyaml
  event stream で行うため parser と乖離しない。同一 scan で
  container nesting depth 128 を超える document も reject する:
  serde_yaml の再帰 deserializer は深い nesting で call stack を
  使い切り（≤1 MiB の `[[[[…` で ~50 万 frame）、他の bound が
  効く前に stack overflow しうる（#144 structural）。
- `parse_skill` の `O_NOFOLLOW` は最終 component のみを守る。
  WalkDir 収集後に skill ディレクトリや祖先 component が symlink
  へ差し替えられる窓は残るが、影響は ≤1 MiB の untrusted parse
  入力に限定され、project config / `.db` open と同じ受容 TOCTOU
  区分として記録する。
- link・requires の target は description と同じ control+Cf strip を
  経てから保持する: target は `Recommendation.reasons` に verbatim
  で載るため、bidi override 等の不可視文字を agent context へ
  到達させない。strip により `requires`照合も正直になる — 不可視
  文字で name 解決を潜らせることはできない（#144 r19）。
- frontmatter `keywords` は sequence を 1 要素ずつ stream して
  32 件・各 128 文字に bound する。全 `serde_yaml::Value` を先に
  materialize すると ~50 万 entry の sequence が bound 適用前に
  全木を allocate する（#144 r19）。`metadata` map は sorted-key
  の決定的 truncation を保つため全 Value を materialize したまま
  — 影響は frontmatter ≤1 MiB に比例する transient に限定される
  受容 residual。
- `bench`（eval::run）は dataset 64 MiB・case 10,000 件に加え、
  graph build の総数 256・recommend cutoff `--max` 64・収集する
  discovery error 合計 4,096 件で bound する。LRU は保持 graph を
  32 に抑えるが総 build work・error 蓄積・case 毎 result 膨張は
  別系統の bound が必要（#144 r19）。CLI `search`/`recommend` の
  `--max` も 64 に clamp する（MCP tool と同じ出力上限）。

## 保証と検証の対応（#225）

各保証対象について「不変条件 → 構造的機構 → 検証」を対応付ける。
検証は性質（正常系だけでなく境界値・敵対的入力・異常系を含む）に
対して行い、修正前実装で失敗することを確認した回帰テストを伴う。

| 保証対象 | 不変条件 | 構造的機構 | 検証 |
|---|---|---|---|
| 入力境界（plugin） | 敵対的・巨大な host payload に対し処理量が O(budget) で有界 | `boundJsonValue`（depth 32 / nodes 8192 / 1 MiB、for..in 遅延走査＋early break、array length は一度だけ読む — Proxy get trap の再読で確保量が検査をすり抜けない）、`resultText`（全ノード計上・64 KiB）、`utf8Slice`（O(budget)）、`detectLoad` の事前 filter | `opencode-plugin.test.ts` の性質テスト: Proxy iterator/descriptor トラップで**走査量そのもの**を観測（旧実装で iters=200,001 / gopd≈50,000 を検出）、2000 件 seeded sweep で safeSessionID 契約、200 件の乱数 result で byte budget |
| session 状態識別 | sanitize 結果が空の session は per-session state を共有しない。runtime・sanitize 済み id で dedup 名前空間を分離。session_id ≤128 chars。sanitize は単射的（literal id が別 id の生成形状と衝突しない — `-`+32hex suffix は reserved） | Rust `state_disabled` + `<runtime>-<session>` state file 名。plugin `attributed` gate で dedup/queue/sessionCtx/context を全 bypass。`emitted` は実際に注入可能な時のみ true | `sanitize_session_id_property_contract`（3000 件 adversarial sweep: ≤128・charset・決定性・非衝突）+ `sanitize_session_id_reserved_shape_cannot_collide`（literal=生成形状の衝突拒否）、TS 側 `reserved transform shape cannot collide` parity、`anonymous session state isolation`（fake CLI + mock ctx で実 hook を駆動、匿名 dedup 非抑制・queue 非漏洩・emitted:false を検証） |
| イベント観測（funnel） | `emitted` は「model が実際に見た注入」のみを表す。recommended[] は注入集合と一致 | plugin は匿名 session に注入経路を持たないため `emitted:false` を記録（Rust は同期 response で匿名注入可能なため true で正しい — 非対称は構造由来） | session 分離テストの `emitted` assertion（匿名 false / attributed true） |
| ファイル所有権・状態安全 | 検査と操作の間に path が差し替えられない。symlink・他人所有・hardlink を拒否。権限は 0700/0600 | state dir を `O_NOFOLLOW|O_DIRECTORY` dirfd で anchor し全操作を openat/fstatat/fchmod 化。plugin `emit()` は O_NOFOLLOW+O_NONBLOCK+uid 検査 | `tests/` の hook/adapter 系 + contracts.md TMPDIR 契約。dirfd 確立前の path 操作は存在しないことをコード構造で保証 |
| clean-legacy 所有権 | v0.3.x 成果物のみ削除。`current` は payload 持ち X.Y.Z dir を指す symlink のみ。release dir は `skill-graph` file AND `adapters/` dir の両 marker 必須 | 二相化（全 entry を削除前 FS で classify→delete） | `tests/adapter.rs` clean_legacy 5 件（lookalike・dangling symlink・bin link・nonlegacy target）。旧実装で `current`/marker 無し dir の誤削除を実証 |
| 配布成果物同一性 | 公開経路の全分岐で「registry が提供する内容」==「この build の pack 内容」。skip も publish 後も同一不変条件。**preflight 完了前に publish 操作は存在しない**（部分公開なし）。package.json `files[]` 宣言ファイル・bin 実行形式（exec bit + `file` 判定）・依存 publish 順序は全 package で publish 前に検証。tarball は gzip magic/integrity・`package/` 単一 root・重複メンバー無し・`..` traversal 無し・regular file/dir のみを archive 段階で要求 | `publish-npm.sh` は二相構造（Phase 1: 全 package の manifest/files[]/bin/依存順序 preflight → Phase 2: skip/publish + registry 同一性検証）→ `verify-npm-registry.sh`（view→curl→pack→比較）→ `verify-npm-tarballs.sh`（archive 形式 gate + package/ root + 全木 diff + mode/type listing）/ `verify-npm-arch.sh`（format+arch predicate）/ `verify-npm-bin.sh`（exec bit + executable format） | `test-release-verify.sh` 33 件（内容不一致・missing/extra・exec-bit・file→dir・corrupt・non-gzip・truncated gzip・duplicate member・symlink/hardlink member・traversal member・root 外ファイル・bin 非実行形式/symlink・wrong arch・unknown target）+ `test-npm-publish.sh` 25 件（stub npm registry で fresh publish・同一 skip・divergent 拒否・post-publish tamper 拒否・**後続 package の preflight 失敗で publish 0 件**・files[] 欠損・bin 非実行形式/symlink/dir・依存順序違反・publish 失敗伝播）。**mutant 実証**: 旧 interleaved publish（部分 publish 7 件 FAIL）・旧 tarball verifier（duplicate/symlink/hardlink 受理 3 件 FAIL）・skip 経路 verify 除去で各 suite が FAIL |
| 性能 | 推薦・discovery・CLI の latency/throughput/memory が spec 範囲内 | オンメモリ engine、LRU graph cache、全走査に上限 | `docs/evidence/` の測定記録（env/SHA/コーパス/再現コマンド）。process latency は 30 samples の p50/p95/max（p95 が max に退化しない件数）、RSS は `/usr/bin/time -v` を各点 20 回計測した最大値で `assets/readme/data/memory.json` に一元化。README/SVG は `perf-500.json`/`bench-eval.json`/`memory.json` から `gen-readme-assets.py` で再生成 |

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
→ 既定 root の順で解決する（hook/plugin と同一規則）。`roots` を
明示した場合は配列であることが必須（非配列は `invalid_params`）。
配列が存在する場合、その配列が探索範囲を定義する: `[]` や全要素が
非文字列・32 件超過で落ちた場合でも既定 root へは拡大せず、
診断付きの空の走査に縮退する（#177 と同一規則）。tool は event
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
- passthrough event の payload 由来文字列フィールドは emit 側
  constructor と同じ上限に正規化される: `session_id` は
  `[A-Za-z0-9._-]`・128 文字（`unknown-<hash>` 正規化と同様に、
  128 文字を超える id は先頭 119 文字 + `-` + hash 8 桁で区別される）、
  `skill_path` 1024、`tool`/`skill_name`/`recommended[].name` 256、
  `reason` 1024、`prompt_hash`/`skill_digest`/`recommended[].digest` 128、
  `timestamp` 64 文字。`recommended` は最大 32 件
  （`MAX_RECOMMENDED`）で打ち切られる。`skill_load.skill_name` は
  `recommended[].name` と同じ valid skill name 規則を要求し、違反行は
  `lines_skipped` に計上される（constructor が emit し得ない名前を
  funnel join キーに流入させない）。charset フィルタで除去される
  文字を含む `session_id` は merge されうる（例: `a:b` と `ab` は
  共に `ab`）— これは sanitize の定義済み動作。`recommended[].name` は
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
  には上限がある: `resultText` は 64 KiB までに切り詰めて保持、
  `tool_input` の JSON parse は 1 MiB 以下のみ（超過は event 化を
  skip）、`skill-graph` への spawn は stdin payload が 1 MiB
  以下の時のみ（fail-open: 上限超過時は注入なしで継続）。
  pending 注入キューは session あたり 64 entry で打ち切る。
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
  比較する。transcript scanner の `finish()` emit は merge 前に
  共有 budget（`MAX_OBSERVE_EVENTS` 件・`MAX_OBSERVE_RETAINED_BYTES`
  推定 byte、両 scanner 合算）で制限され、超過 event は detail を
  捨てて counter map のみ更新する（passthrough pending buffer と
  同じ意味論）。tool_use 1 件あたりの classification 数も
  `MAX_EVENTS_PER_PAYLOAD`（64）で打ち切る（hook の per-payload
  cap と同じ上限）。
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
  対象とする。Graph phase（`resolver::build`）では link 毎に
  `resolved_name`/`resolved_path` が追加される transient を持つ。
  backlinks は name-resolved link を name 毎・path-resolved link を
  target 毎の source 集合として集約し、最終的には target 毎の
  dedup 済み source **件数**のみを保持する（消費側は件数のみ使用）。
  同名 copy への fan-out は発生せず O(links + skills) に留まる
  （#144 r18）。MCP `recommend`/`resolve` は call 毎にこの Graph を
  再構築するため、実メモリは名目値を超えうる既知の residual として
  記録する（`discover` tool は resolver を通らずこの上乗せを持たない）。
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
  event stream で行うため parser と乖離しない。
- `parse_skill` の `O_NOFOLLOW` は最終 component のみを守る。
  WalkDir 収集後に skill ディレクトリや祖先 component が symlink
  へ差し替えられる窓は残るが、影響は ≤1 MiB の untrusted parse
  入力に限定され、project config / `.db` open と同じ受容 TOCTOU
  区分として記録する。

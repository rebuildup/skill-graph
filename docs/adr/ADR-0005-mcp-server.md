# ADR-0005: MCP server の境界（stdio・read-only・観測外）

Date: 2026-11-26
Status: accepted (v1.0.0 spec)

## Context

v1.0.0 spec は MCP を正式な完成対象に含める。skill-graph の価値は
「必要な skill が必要なタイミングで届く」ことであり、MCP は hook を
持たない・または hook を介さずに skill 検索したい client（任意の
MCP 対応 agent）への第4の利用インターフェースになる。

O02（backlog）は「read-only tool surface と権限境界を設計」を
採用条件としていた。v1.0.0 spec §6 で採用が確定したため、境界を
ここで固定する。

## Decision

### Transport: stdio のみ

`skill-graph mcp` は stdio 上の JSON-RPC サーバとして動作する。
Streamable HTTP / SSE は提供しない。理由:

- skill-graph は local filesystem の skill を読むローカルツールであり、
  network 公開の必要性がない。
- stdio は全主要 MCP client（Claude Code / OpenCode / Codex）が
  child process として起動できる最小共通形である。
- HTTP transport は auth・session 管理・bind address の設計を新たに
  必要とし、local ツールの要求を超える。

### Tool surface: read-only 3種

| Tool | 対応 CLI | 副作用 |
|---|---|---|
| `recommend` | `recommend --format json` | なし（graph は in-memory 再構築） |
| `discover` | `discover --format json` | なし |
| `resolve` | `resolve --format json` | なし |

resources / prompts / sampling / elicitation / roots は提供しない。
write 系 tool・file 変更・network 取得・hook 有効化・イベント発行は
実装しない。package install や MCP 登録だけでは何も有効化されず、
各 client の設定記述が唯一の opt-in 経路である。

### 実装: 公式 rmcp SDK

`rmcp`（modelcontextprotocol/rust-sdk、Apache-2.0）を server +
stdio transport feature で利用する。MCP protocol version negotiation・
capability 広告・将来の spec 改訂への追従を自前実装で抱えないため。
同期 engine（CLI/hook/adapter）とは独立に、MCP server のみ tokio
runtime で動く薄い adapter 層とし、推薦ロジックは既存の
`discovery`/`resolver`/`search` をそのまま呼ぶ。

### 観測境界: event log に書かない

MCP tool call は Skill Usage Event を発行しない。

- event contract の `session_id` は hook payload / transcript 由来の
  runtime session を前提としており、stdio MCP には対応する session
  概念がない。プロセスごとの擬似 ID を立てると per-session join
  （conversion 等）が成立せず、指標を歪める。
- `observed_via` に `mcp` を追加するには contract 改訂と emitter
  設計が必要であり、pilot で MCP 経路の利用実態が観測されてから
  判断するのが適切（v1.0.0 spec §1 の境界）。
- read-only・no-persistence は privacy boundary とも整合する。

### roots の解決

`SKILL_GRAPH_ROOTS` → tool 引数 `roots` → CLI 既定 root の順で解決し、
`adapter`/hook と同一の root 規則を共有する。MCP server が独自の
発見規則を持たない。

## Consequences

- `skill-graph mcp` は全配布チャネル（npm/brew/cargo/mise）で追加
  成果物なしに利用可能になる。
- MCP client 登録手順は README に記載する（Claude Code `claude mcp
  add`、OpenCode `mcp` config、Codex `config.toml` の各最小例）。
- tokio が依存に追加されるが、MCP feature を使わない限り実行経路に
  影響しない（`mcp` サブコマンドのみ async runtime を初期化する）。
- 将来 HTTP transport や MCP 経由の観測が必要になった場合は、本
  ADR の境界見直しとして別 ADR で扱う。

# skill-graph adapters

Runtime-specific, opt-in adapters. None of them modify PATH, shell startup
files, or unrelated configuration; all hooks fail open and support a no-save
mode (`SKILL_GRAPH_HOOK_NOSTATE=1`).

| Runtime | Mechanism | Status |
|---|---|---|
| `claude-code/` | Agent Plugins dir (`--plugin-dir`) or settings.json hooks via `install.sh` | Verified live (v0.1.0 S02) |
| `codex/` | `hooks.json` (user `$CODEX_HOME` or trusted `.codex/`) | Live-verified (v0.2.0 #70); skill-load detection via `PostToolUse` matcher `Bash` |
| `opencode/` | TS plugin (`@opencode/plugin`) | Live-verified core path (v0.2.0 #70) |

## Skill Usage Events

All adapters emit the shared contract
(`contracts/v1/skill-usage-event.schema.json`) as JSONL to
`${TMPDIR}/skill-graph-events.jsonl` (`SKILL_GRAPH_EVENT_LOG` overrides).
`skill-graph observe <path>` aggregates hook/plugin events together with
read-only transcript/DB normalization into the same event shape.

- `prompt` mode emits `prompt_submit` + `recommendation` per prompt.
- `skill-load` mode emits `skill_load` (via `skill-graph detect`) and, only
  for load-grade detections (`file_read`/`path_reference`/`native_invocation`/
  `inferred`/`applied`), a dependency `recommendation`. Non-skill tool calls
  are excluded by a jq pre-filter before the binary runs — no corpus analysis
  or context injection happens for them.
- No command lines, tool responses, or prompt bodies are persisted; prompts
  are SHA-256 hashed. `SKILL_GRAPH_HOOK_NOSTATE=1` disables all writes
  (verified: no log file, state dir, or markers are created).
- Event log retention: `${TMPDIR}` is volatile (OS tmp cleaning); the log
  additionally rotates to `<log>.1` once it exceeds
  `SKILL_GRAPH_EVENT_LOG_MAX` bytes (default 64 MiB). Dedup state lives in
  `${TMPDIR}/skill-graph-hook-state` with sanitized per-session marker
  names. Appends are single-line `O_APPEND` writes, safe for concurrent
  hook invocations.

## PATH-free install / uninstall

`install.sh` wires a shell-hook adapter into a runtime's native config using
absolute paths only. Requires `jq` and a `skill-graph` binary
(`SKILL_GRAPH_BIN`, `$SKILL_GRAPH_HOME/bin/skill-graph`, or `PATH`).

```bash
adapters/install.sh claude-code --user      # ~/.claude/settings.json
adapters/install.sh claude-code --project   # ./.claude/settings.json
adapters/install.sh codex --user            # ~/.codex/hooks.json
adapters/install.sh codex --project         # ./.codex/hooks.json
adapters/install.sh opencode --user         # $XDG_CONFIG_HOME/opencode/opencode.json
adapters/install.sh opencode --project      # ./opencode.json
adapters/install.sh <runtime> <scope> --dry-run
adapters/install.sh <runtime> <scope> --uninstall
```

- Merge is additive: existing hooks for the same event are preserved; re-running
  install is idempotent (entries referencing the adapter are replaced, not
  duplicated).
- Uninstall removes only entries whose command references the adapter script
  (or, for OpenCode, the plugin specifier) and prunes emptied groups; a config
  reduced to `{}` is deleted so the pre-install state is fully restored.
- OpenCode's extension point is a TS plugin, not shell hooks: `install.sh
  opencode` merges a `file://<abs path>/adapters/opencode/skill-graph`
  specifier into `opencode.json`'s `plugins` array.
- Codex requires the new hooks to be trusted once via `/hooks` in the Codex
  TUI before they run; this is the normal install path. Bypassing trust is
  for experiments only.
- `test-install-roundtrip.sh` verifies install → reinstall → uninstall
  restores seeded config for every runtime/scope, including the dotfiles
  compose-overlay case (see
  `docs/evidence/2026-11-26-w04-install-roundtrip.md` and
  `docs/evidence/2026-11-26-v030-release-verification.md`).
- On dotfiles-managed machines, `install.sh claude-code --user` hooks are
  imported into `~/.config/dotfiles/claude/settings.local.json` by the next
  `claude-settings compose`; `--uninstall` removes them from both the live
  settings and the overlay so they cannot be regenerated.

## Distribution layout

A release archive should contain:

```
bin/skill-graph                    # CLI binary
adapters/install.sh
adapters/<runtime>/...             # per-runtime adapter payloads
sha256sums.txt
```

Users extract anywhere, set `SKILL_GRAPH_BIN`/`SKILL_GRAPH_HOME` if the binary
is not on `PATH`, and run `install.sh`. Nothing else on the system is touched.

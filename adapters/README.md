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
  install is idempotent. Entries are identified by adapter markers
  (`skill-graph-recommend-hook` / the `/skill-graph` plugin specifier), not by
  embedded paths, so installing from a different version directory replaces
  prior entries instead of duplicating them.
- Uninstall removes only entries carrying those markers and prunes emptied
  groups; a config reduced to `{}` is deleted so the pre-install state is
  fully restored. Uninstall is marker-based and does not require the binary
  or adapter files to still exist.
- OpenCode's extension point is a TS plugin, not shell hooks: `install.sh
  opencode` merges a `file://<abs path>/adapters/opencode/skill-graph`
  specifier into `opencode.json`'s `plugins` array.
- Codex requires the new hooks to be trusted once via `/hooks` in the Codex
  TUI before they run; this is the normal install path. Bypassing trust is
  for experiments only.
- `test-install-roundtrip.sh` verifies install → reinstall → uninstall
  restores seeded config for every runtime/scope, including the dotfiles
  compose-overlay case and versioned-path updates (see
  `docs/evidence/2026-11-26-w04-install-roundtrip.md` and
  `docs/evidence/2026-11-26-v030-release-verification.md`).
- On dotfiles-managed machines, `install.sh claude-code --user` hooks are
  imported into `~/.config/dotfiles/claude/settings.local.json` by the next
  `claude-settings compose`; `--uninstall` removes them from both the live
  settings and the overlay so they cannot be regenerated.

## Runtime lifecycle

Who owns each config, and how update / disable / restore work:

| Runtime | Config owner | Enable | Disable | Restore |
|---|---|---|---|---|
| `claude-code` | `~/.claude/settings.json` (dotfiles machines: composed from `~/.config/dotfiles/claude/` baseline + `settings.local.json` overlay) | `install.sh claude-code --user` then `claude-settings compose` | `install.sh claude-code --user --uninstall` (cleans live file and overlay) | re-run install; compose regenerates from baseline |
| `codex` | `~/.codex/hooks.json` (`$CODEX_HOME` override respected) | `install.sh codex --user`, then trust once via `/hooks` in the Codex TUI | `install.sh codex --user --uninstall`; trusted entries remain listed but inert | re-run install; trust persists while command strings are unchanged |
| `opencode` | `~/.config/opencode/opencode.json` (`$XDG_CONFIG_HOME` respected) | `install.sh opencode --user` | `install.sh opencode --user --uninstall` | re-run install |

### Versioned updates and Codex trust

Codex pins hook trust by hashing the hook definition, including the command
string. To keep trust valid across upgrades, the root `install.sh` installs
payloads under `~/.local/share/skill-graph/<version>/` and points a stable
`current` symlink at the active version; hook commands embed
`current/...` paths, so upgrading flips the symlink without changing any
config entry. The same layout is recommended for manual bundle installs:
reference adapters through a version-independent path.

`adapters/install.sh` marker matching makes updates safe regardless: entries
from any prior version directory are removed before the new ones are added,
so no duplicate hooks accumulate and uninstall removes all generations.

## Distribution layout

GitHub Releases ship per-target `skill-graph-<target>.tar.gz` (the `skill-graph`
binary) plus `skill-graph-adapters.tar.gz` (`adapters/` + `contracts/` +
`plugins/`) and `sha256sums.txt`. The root `install.sh` downloads, verifies,
and lays these out under `~/.local/share/skill-graph/<version>/` with a
`current` symlink — see the repository README for the one-command flow.

For manual installs, extract the binary and adapter bundle anywhere, set
`SKILL_GRAPH_BIN`/`SKILL_GRAPH_HOME` if the binary is not on `PATH`, and run
`adapters/install.sh`. Prefer a version-independent path (symlink) for
`SKILL_GRAPH_HOME` so upgrades do not require re-installing hooks.

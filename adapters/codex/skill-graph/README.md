# skill-graph adapter for Codex CLI

Opt-in adapter that surfaces skill-graph recommendations inside Codex via its
native `hooks.json` lifecycle hooks. It does not modify Codex configuration on
its own; installation is a manual, reviewable merge step.

## Compatibility basis

Verified against the Codex hooks documentation (2026-11):

- `hooks.json` discovery: `$CODEX_HOME/hooks.json` (user), `.codex/hooks.json`
  (trusted project layers), `/etc/codex/hooks.json` (system).
- Events used: `UserPromptSubmit` (input field `prompt`) and `PostToolUse`
  (input `tool_name`, `tool_input`).
- Output contract: `{"hookSpecificOutput": {"hookEventName": ..., "additionalContext": ...}}`
  — identical shape to the Claude Code adapter.
- Commands run with the session `cwd`; plain script + `jq` + the
  `skill-graph` binary. No daemon, no PATH registration required if absolute
  paths are used.

## Layout

- `hooks.json` — template. `${SKILL_GRAPH_HOOK_BIN}` must be replaced with the
  absolute path of `bin/skill-graph-recommend-hook` at install time (Codex does
  not expand plugin-root variables). `PostToolUse` uses matcher `Bash` because
  Codex has no native skill tool — SKILL.md reads surface as Bash commands
  and are classified by `skill-graph detect`.
- `bin/skill-graph-recommend-hook` — the hook entrypoint (same contract as the
  Claude Code adapter; default skill roots include `~/.codex/skills`,
  `~/.agents/skills`, and `$PWD`-relative `.codex`/`.agents`/`skills` dirs).

## Install (manual, per project or per user)

1. Build the CLI: `cargo build --release` in the skill-graph repository.
2. Choose a stable location for this adapter directory (e.g. a clone of the
   repo, or an extracted release archive).
3. Merge the `hooks` object from `hooks.json` into the target hooks file,
   replacing `${SKILL_GRAPH_HOOK_BIN}` with the absolute path to
   `bin/skill-graph-recommend-hook` and setting
   `SKILL_GRAPH_BIN` to the absolute path of the `skill-graph` binary
   (either via the hook command's environment or by exporting it in the shell
   that launches Codex).
4. Restart Codex. Non-managed hooks require review: open `/hooks` in the TUI
   and trust the new hook definitions (they are recorded against their hash;
   edits require re-trust). This `/hooks` trust step is the **normal install
   path** — it is a one-time, per-definition approval recorded by Codex.
   Bypassing or auto-approving hook trust (e.g. config flags that skip
   review) is for experiments only and must not be part of the documented
   install flow.

## Verified live (codex-cli 0.171.0, ChatGPT auth)

- `UserPromptSubmit` fires; `additionalContext` confirmed in rollout JSONL.
- Multi-turn via `exec resume` works; ~33ms/hook.
- `PostToolUse` fires with matcher `Bash` (Codex has no Skill tool — skills
  are read via ordinary file/Bash calls). The hook script classifies the
  command via `skill-graph detect` and emits `skill_load` Skill Usage
  Events; dependency injection on skill load now works through the same
  path (see docs/evidence/2026-11-26-w02-skill-usage-events.md).
- Codex's own skill discovery (`~/.agents/skills`, `.agents/skills`,
  `~/.codex/skills` legacy) overlaps with the hook's default roots; duplicates
  are handled by the engine's name-grouping contract.
- Hook trust is hash-pinned: upgrading the adapter script invalidates trust
  and requires re-approval in `/hooks`.

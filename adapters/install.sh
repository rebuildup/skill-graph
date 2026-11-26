#!/usr/bin/env bash
# skill-graph adapter installer — PATH-free, opt-in, reversible.
#
# Wires adapter hooks into a runtime's native config using ABSOLUTE paths.
# Never touches PATH, shell rc files, or unrelated config keys. Uninstall
# removes exactly the entries whose command references skill-graph.
#
# Usage:
#   install.sh <claude-code|codex> [--user|--project] [--dry-run]
#   install.sh <claude-code|codex> --uninstall [--user|--project]
#
# Env:
#   SKILL_GRAPH_HOME   repo/dist root containing bin/skill-graph and adapters/
#                      (default: parent dir of this script's repo layout,
#                      i.e. two levels up)
set -euo pipefail

usage() { sed -n '2,20p' "$0"; exit "${1:-0}"; }

runtime="" scope="--user" uninstall=0 dry=0
while [ $# -gt 0 ]; do
  case "$1" in
    claude-code|codex) runtime="$1" ;;
    --user|--project) scope="$1" ;;
    --uninstall) uninstall=1 ;;
    --dry-run) dry=1 ;;
    -h|--help) usage 0 ;;
    *) echo "unknown arg: $1" >&2; usage 1 ;;
  esac
  shift
done
[ -n "$runtime" ] || usage 1
command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }

here="$(cd "$(dirname "$0")" && pwd)"
home="${SKILL_GRAPH_HOME:-$(cd "$here/.." && pwd)}"
bin="${SKILL_GRAPH_BIN:-$home/bin/skill-graph}"
[ -x "$bin" ] || bin="$(command -v skill-graph || true)"
[ -n "$bin" ] && [ -x "$bin" ] || { echo "skill-graph binary not found; set SKILL_GRAPH_HOME or PATH" >&2; exit 1; }
bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"

case "$runtime:$scope" in
  claude-code:--user)    hooks_file="$HOME/.claude/settings.json" ;;
  claude-code:--project) hooks_file="$PWD/.claude/settings.json" ;;
  codex:--user)          hooks_file="${CODEX_HOME:-$HOME/.codex}/hooks.json" ;;
  codex:--project)       hooks_file="$PWD/.codex/hooks.json" ;;
esac

adapter_bin="$here/$runtime/skill-graph/bin/skill-graph-recommend-hook"
[ -f "$adapter_bin" ] || { echo "adapter script missing: $adapter_bin" >&2; exit 1; }
adapter_bin="$(cd "$(dirname "$adapter_bin")" && pwd)/$(basename "$adapter_bin")"

mktemp_json() { mktemp "${TMPDIR:-/tmp}/skill-graph-install.XXXXXX.json"; }

if [ "$uninstall" = 1 ]; then
  [ -f "$hooks_file" ] || { echo "no hooks file: $hooks_file"; exit 0; }
  tmp="$(mktemp_json)"
  # Drop hook command objects referencing our adapter, then prune emptied
  # matcher groups and event arrays. Other hooks are untouched.
  jq --arg m "$adapter_bin" '
    .hooks = ((.hooks // {}) | with_entries(.value |=
      map(.hooks = ((.hooks // []) | map(select(.command == null or ((.command | contains($m)) | not))))
        | select((.hooks | length) > 0))
    ) | with_entries(select(.value | length > 0)))
    | if (.hooks | length) == 0 then del(.hooks) else . end
  ' "$hooks_file" > "$tmp" && mv "$tmp" "$hooks_file"
  echo "uninstalled skill-graph hooks from $hooks_file"
  exit 0
fi

# Build the hooks fragment with absolute paths substituted. The adapter
# script is the hook entrypoint; SKILL_GRAPH_BIN points it at the CLI
# binary so nothing needs to be on PATH.
frag="$(mktemp_json)"
case "$runtime" in
  claude-code)
    jq -n --arg h "$adapter_bin" --arg sg "$bin" '{
      hooks: {
        UserPromptSubmit: [{hooks: [{type: "command",
          command: ("SKILL_GRAPH_BIN=" + ($sg | @sh) + " " + ($h | @sh) + " prompt")}]}],
        PostToolUse: [{matcher: "Skill", hooks: [{type: "command",
          command: ("SKILL_GRAPH_BIN=" + ($sg | @sh) + " " + ($h | @sh) + " skill-load")}]}]
      }
    }' > "$frag"
    ;;
  codex)
    jq -n --arg h "$adapter_bin" --arg sg "$bin" '{
      hooks: {
      UserPromptSubmit: [{hooks: [{type: "command",
        command: ("SKILL_GRAPH_BIN=" + ($sg | @sh) + " " + ($h | @sh) + " prompt"),
        statusMessage: "Recommending skills"}]}],
      PostToolUse: [{matcher: "Bash", hooks: [{type: "command",
        command: ("SKILL_GRAPH_BIN=" + ($sg | @sh) + " " + ($h | @sh) + " skill-load"),
        statusMessage: "Resolving skill dependencies"}]}]
      }
    }' > "$frag"
    ;;
esac

merge() {
  local target="$1" fragment="$2"
  if [ "$dry" = 1 ]; then
    echo "--- would merge into $target ---"; cat "$fragment"; return 0
  fi
  mkdir -p "$(dirname "$target")"
  # Append our matcher groups per event; first drop any existing entries that
  # already reference this adapter so re-running install is idempotent and
  # other hooks in the same event are preserved.
  if [ -f "$target" ]; then
    jq --arg m "$adapter_bin" --slurpfile f "$fragment" '
      .hooks = (.hooks // {})
      | reduce (($f[0].hooks // {}) | to_entries[]) as $ev (.;
          .hooks[$ev.key] = (
            [(.hooks[$ev.key] // [])[]
              | select([.hooks[]?.command // "" | contains($m)] | any | not)]
            + $ev.value
          )
        )
    ' "$target" > "$target.new" && mv "$target.new" "$target"
  else
    jq -n --slurpfile f "$fragment" '$f[0]' > "$target"
  fi
}

merge "$hooks_file" "$frag"
rm -f "$frag"
echo "installed skill-graph ($runtime, $scope) -> $hooks_file"
echo "binary: $bin"
[ "$runtime" = codex ] && echo "note: trust the new hooks via /hooks in Codex"

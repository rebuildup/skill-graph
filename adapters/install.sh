#!/usr/bin/env bash
# skill-graph adapter installer — PATH-free, opt-in, reversible.
#
# Wires adapter hooks into a runtime's native config using ABSOLUTE paths.
# Never touches PATH, shell rc files, or unrelated config keys. Uninstall
# removes exactly the entries whose command references skill-graph.
#
# Usage:
#   install.sh <claude-code|codex|opencode> [--user|--project] [--dry-run]
#   install.sh <claude-code|codex|opencode> --uninstall [--user|--project]
#
# Env:
#   SKILL_GRAPH_HOME   repo/dist root containing bin/skill-graph and adapters/
#                      (default: parent dir of this script's repo layout,
#                      i.e. two levels up)
#   XDG_CONFIG_HOME    opencode --user config dir override (default ~/.config)
set -euo pipefail

usage() { sed -n '2,20p' "$0"; exit "${1:-0}"; }

runtime="" scope="--user" uninstall=0 dry=0
while [ $# -gt 0 ]; do
  case "$1" in
    claude-code|codex|opencode) runtime="$1" ;;
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

case "$runtime:$scope" in
  claude-code:--user)    hooks_file="$HOME/.claude/settings.json" ;;
  claude-code:--project) hooks_file="$PWD/.claude/settings.json" ;;
  codex:--user)          hooks_file="${CODEX_HOME:-$HOME/.codex}/hooks.json" ;;
  codex:--project)       hooks_file="$PWD/.codex/hooks.json" ;;
  opencode:--user)       cfg_file="${XDG_CONFIG_HOME:-$HOME/.config}/opencode/opencode.json" ;;
  opencode:--project)    cfg_file="$PWD/opencode.json" ;;
esac

if [ "$runtime" = opencode ]; then
  hooks_file="$cfg_file"
fi

# Identity markers used to find OUR entries in runtime configs. Matching is
# deliberately NOT tied to the absolute install path: the versioned layout
# (~/.local/share/skill-graph/<ver>/ or a `current` symlink) means the same
# logical adapter can live at a different path after an update, and path-based
# matching would leave stale duplicate hooks behind.
hook_marker="skill-graph-recommend-hook"
# Our opencode plugin spec is always a file:// URL whose path ends in the
# plugin directory named `skill-graph`; "/skill-graph" cannot collide with an
# npm package specifier.
plugin_marker="/skill-graph"

mktemp_json() { mktemp "${TMPDIR:-/tmp}/skill-graph-install.XXXXXX.json"; }

if [ "$uninstall" = 1 ]; then
  [ -f "$hooks_file" ] || { echo "no config file: $hooks_file"; exit 0; }
  tmp="$(mktemp_json)"
  # A config reduced to {} carries no information; remove it so uninstall
  # fully restores the pre-install state (no stray empty files).
  finish_uninstall() { # $1 = tmp file, $2 = target config file
    if jq -e 'keys | length == 0' "$1" >/dev/null; then
      rm -f "$2"
    else
      mv "$1" "$2"
    fi
    rm -f "$1"
  }
  if [ "$runtime" = opencode ]; then
    # Drop our plugin specifier (any versioned path), then prune an emptied
    # plugins array.
    jq --arg m "$plugin_marker" '
      .plugins = ((.plugins // []) | map(select((. | contains($m)) | not)))
      | if (.plugins | length) == 0 then del(.plugins) else . end
    ' "$hooks_file" > "$tmp"
    finish_uninstall "$tmp" "$hooks_file"
    echo "uninstalled skill-graph plugin from $hooks_file"
    exit 0
  fi
  # Drop hook command objects referencing our adapter, then prune emptied
  # matcher groups and event arrays. Other hooks are untouched.
  jq --arg m "$hook_marker" '
    .hooks = ((.hooks // {}) | with_entries(.value |=
      map(.hooks = ((.hooks // []) | map(select(.command == null or ((.command | contains($m)) | not))))
        | select((.hooks | length) > 0))
    ) | with_entries(select(.value | length > 0)))
    | if (.hooks | length) == 0 then del(.hooks) else . end
  ' "$hooks_file" > "$tmp"
  finish_uninstall "$tmp" "$hooks_file"
  echo "uninstalled skill-graph hooks from $hooks_file"
  # dotfiles-managed machines compose ~/.claude/settings.json from a
  # machine-local overlay; hooks previously imported there would be
  # regenerated on the next compose, so uninstall must clean it too.
  if [ "$runtime" = claude-code ] && [ "$scope" = "--user" ]; then
    overlay="${XDG_CONFIG_HOME:-$HOME/.config}/dotfiles/claude/settings.local.json"
    if [ -f "$overlay" ]; then
      tmp="$(mktemp_json)"
      if jq --arg m "$hook_marker" '
        .hooks = ((.hooks // {}) | with_entries(.value |=
          map(.hooks = ((.hooks // []) | map(select(.command == null or ((.command | contains($m)) | not))))
            | select((.hooks | length) > 0))
        ) | with_entries(select(.value | length > 0)))
        | if (.hooks | length) == 0 then del(.hooks) else . end
      ' "$overlay" > "$tmp"; then
        mv "$tmp" "$overlay"
      else
        rm -f "$tmp"
      fi
      echo "uninstalled skill-graph hooks from $overlay"
    fi
  fi
  exit 0
fi

# Install path only from here on: resolve the binary, the adapter script, and
# (for opencode) the plugin directory. Uninstall above is marker-based and
# deliberately works even when these files are already gone.
bin="${SKILL_GRAPH_BIN:-$home/bin/skill-graph}"
[ -x "$bin" ] || bin="$(command -v skill-graph || true)"
[ -n "$bin" ] && [ -x "$bin" ] || { echo "skill-graph binary not found; set SKILL_GRAPH_BIN or PATH" >&2; exit 1; }
bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"

if [ "$runtime" = opencode ]; then
  plugin_dir="$here/opencode/skill-graph"
  [ -f "$plugin_dir/index.ts" ] || { echo "opencode plugin missing: $plugin_dir" >&2; exit 1; }
  plugin_dir="$(cd "$plugin_dir" && pwd)"
  plugin_spec="file://$plugin_dir"
fi

if [ "$runtime" != opencode ]; then
  adapter_bin="$here/$runtime/skill-graph/bin/skill-graph-recommend-hook"
  [ -f "$adapter_bin" ] || { echo "adapter script missing: $adapter_bin" >&2; exit 1; }
  adapter_bin="$(cd "$(dirname "$adapter_bin")" && pwd)/$(basename "$adapter_bin")"
fi

# Build the config fragment with absolute paths substituted. The adapter
# script is the hook entrypoint; SKILL_GRAPH_BIN points it at the CLI
# binary so nothing needs to be on PATH. OpenCode has no hooks.json — its
# extension point is a TS plugin listed in opencode.json `plugins`.
frag="$(mktemp_json)"
case "$runtime" in
  opencode)
    jq -n --arg p "$plugin_spec" '{plugins: [$p]}' > "$frag"
    ;;
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
  if [ "$runtime" = opencode ]; then
    # Append our plugin specifier once, preserving other plugins and keys.
    # Drop any prior skill-graph entry first so a version/path change cannot
    # leave duplicates behind.
    if [ -f "$target" ]; then
      jq --arg p "$plugin_spec" --arg m "$plugin_marker" '
        .plugins = (((.plugins // []) | map(select((. | contains($m)) | not))) + [$p])
      ' "$target" > "$target.new" && mv "$target.new" "$target"
    else
      jq -n --arg p "$plugin_spec" '{plugins: [$p]}' > "$target"
    fi
    return 0
  fi
  # Append our matcher groups per event; first drop any existing entries that
  # already reference this adapter so re-running install is idempotent and
  # other hooks in the same event are preserved.
  if [ -f "$target" ]; then
    jq --arg m "$hook_marker" --slurpfile f "$fragment" '
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
if [ "$runtime" = codex ]; then
  echo "note: trust the new hooks via /hooks in Codex"
fi

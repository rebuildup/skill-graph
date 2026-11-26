#!/usr/bin/env bash
# Roundtrip verification for adapters/install.sh (issue #79 / W04).
#
# For each runtime x scope: seeds a config file containing unrelated
# entries, runs install -> install (idempotency) -> uninstall, and asserts
# the result is semantically identical to the seed (jq -S canonical form).
# Runs entirely inside a temporary HOME/project; never touches real config.
#
# Usage: test-install-roundtrip.sh   (from repo root or anywhere)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
home_dir="$(cd "$here/.." && pwd)"
install="$here/install.sh"
bin="${SKILL_GRAPH_BIN:-$home_dir/bin/skill-graph}"
[ -x "$bin" ] || bin="$(command -v skill-graph || true)"
[ -n "$bin" ] && [ -x "$bin" ] || { echo "skill-graph binary not found; set SKILL_GRAPH_BIN" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-roundtrip.XXXXXX")"
trap 'rm -rf "$work"' EXIT
fail=0

canon() { jq -S . "$1"; }
check_eq() { # $1 desc $2 fileA $3 fileB
  if canon "$2" | cmp -s - <(canon "$3"); then
    echo "PASS $1"
  else
    echo "FAIL $1"; diff <(canon "$2") <(canon "$3") | head -20; fail=1
  fi
}

seed_hooks() { # $1 file — unrelated hooks that must survive
  mkdir -p "$(dirname "$1")"
  cat > "$1" <<'JSON'
{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo other-prompt"}]}],
"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo other-bash"}]}]}}
JSON
}
seed_opencode() { # $1 file — unrelated plugin + keys
  mkdir -p "$(dirname "$1")"
  cat > "$1" <<'JSON'
{"plugins":["other-plugin"],"theme":"dark","model":"test/model"}
JSON
}

run_case() { # $1 runtime $2 scope $3 relpath $4 seeder
  local rt="$1" scope="$2" rel="$3" seeder="$4"
  local fakehome="$work/home-$rt-$scope" proj="$work/proj-$rt-$scope"
  mkdir -p "$fakehome" "$proj"
  local target
  case "$rt:$scope" in
    claude-code:--user)    target="$fakehome/.claude/settings.json" ;;
    claude-code:--project) target="$proj/.claude/settings.json" ;;
    codex:--user)          target="$fakehome/.codex/hooks.json" ;;
    codex:--project)       target="$proj/.codex/hooks.json" ;;
    opencode:--user)       target="$fakehome/.config/opencode/opencode.json" ;;
    opencode:--project)    target="$proj/opencode.json" ;;
  esac
  "$seeder" "$target"
  cp "$target" "$work/orig-$rt-$scope.json"

  # install (fresh), install again (idempotent), uninstall
  (cd "$proj" && HOME="$fakehome" SKILL_GRAPH_BIN="$bin" \
     "$install" "$rt" "$scope" >/dev/null)
  cp "$target" "$work/installed-$rt-$scope.json"
  (cd "$proj" && HOME="$fakehome" SKILL_GRAPH_BIN="$bin" \
     "$install" "$rt" "$scope" >/dev/null)
  cp "$target" "$work/reinstalled-$rt-$scope.json"
  (cd "$proj" && HOME="$fakehome" SKILL_GRAPH_BIN="$bin" \
     "$install" "$rt" "$scope" --uninstall >/dev/null)
  cp "$target" "$work/uninstalled-$rt-$scope.json"

  check_eq "$rt $scope: reinstall == install" "$work/installed-$rt-$scope.json" "$work/reinstalled-$rt-$scope.json"
  check_eq "$rt $scope: uninstall restores seed" "$work/orig-$rt-$scope.json" "$work/uninstalled-$rt-$scope.json"
}

for scope in --user --project; do
  run_case claude-code "$scope" x seed_hooks
  run_case codex       "$scope" x seed_hooks
  run_case opencode    "$scope" x seed_opencode
done

# uninstall with no pre-existing config leaves no file behind
d=$(mktemp -d); HOME="$d" SKILL_GRAPH_BIN="$bin" "$install" codex --user --uninstall >/dev/null 2>&1 || true
[ ! -e "$d/.codex/hooks.json" ] && echo "PASS codex uninstall on absent file" || { echo "FAIL codex uninstall on absent file"; fail=1; }
rm -rf "$d"

# dotfiles overlay: uninstall must also clean the compose overlay, or the
# next `claude-settings compose` would regenerate the hooks.
d=$(mktemp -d)
mkdir -p "$d/.config/dotfiles/claude" "$d/.claude"
cat > "$d/.config/dotfiles/claude/settings.local.json" <<'JSON'
{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"echo keep"}]}]}}
JSON
cp "$d/.claude/settings.json" "$work/x" 2>/dev/null || true
HOME="$d" SKILL_GRAPH_BIN="$bin" "$install" claude-code --user >/dev/null
# simulate dotfiles compose having imported our hooks into the overlay:
# copy the live .hooks wholesale (exactly what `extras(shared, live)` yields)
jq --slurpfile live "$d/.claude/settings.json" '.hooks = (.hooks + $live[0].hooks)' \
  "$d/.config/dotfiles/claude/settings.local.json" > "$d/ov.tmp" && mv "$d/ov.tmp" "$d/.config/dotfiles/claude/settings.local.json"
HOME="$d" SKILL_GRAPH_BIN="$bin" "$install" claude-code --user --uninstall >/dev/null
if jq -e '[.. | strings | select(contains("skill-graph-recommend-hook"))] | length == 0' \
     "$d/.config/dotfiles/claude/settings.local.json" >/dev/null \
   && jq -e '.hooks.SessionStart | length == 1' "$d/.config/dotfiles/claude/settings.local.json" >/dev/null; then
  echo "PASS claude-code --user: uninstall cleans dotfiles overlay"
else
  echo "FAIL claude-code --user: uninstall cleans dotfiles overlay"; fail=1
fi
rm -rf "$d"

exit "$fail"

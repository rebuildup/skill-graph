#!/usr/bin/env bash
# skill-graph installer — fetch a GitHub Release, verify checksums, and lay
# down a versioned install that adapters reference through a stable `current`
# symlink. PATH and shell rc files are never modified.
#
# Layout:
#   ${SKILL_GRAPH_HOME:-~/.local/share/skill-graph}/
#     <version>/           binary + adapters/ + contracts/ + plugins/
#     current -> <version> stable path embedded into runtime configs
#   ${SKILL_GRAPH_BIN_DIR:-~/.local/bin}/skill-graph -> current/skill-graph
#
# Because hook/plugin entries reference `current/...` and the bin symlink,
# upgrading is just re-running this script: runtime configs do not change,
# so Codex hook trust and dotfiles compose output survive updates.
#
# Usage:
#   install.sh [--version X.Y.Z] [--runtime claude-code|codex|opencode]...
#              [--bin-dir DIR] [--dry-run]
#   install.sh --uninstall [--runtime ...] [--bin-dir DIR] [--purge]
#
# Env:
#   SKILL_GRAPH_RELEASE_REPO   owner/repo (default: rebuildup/skill-graph)
#   SKILL_GRAPH_RELEASE_DIR    local dir with release assets (testing/offline)
#   SKILL_GRAPH_HOME           install root (default: ~/.local/share/skill-graph)
#   SKILL_GRAPH_BIN_DIR        bin dir for the skill-graph symlink
#                              (default: ~/.local/bin)
#   SKILL_GRAPH_NO_BIN=1       skip the bin symlink entirely
set -euo pipefail

repo="rebuildup/skill-graph"
version=""
uninstall=0
purge=0
dry=0
no_bin="${SKILL_GRAPH_NO_BIN:-0}"
bin_dir="${SKILL_GRAPH_BIN_DIR:-$HOME/.local/bin}"
runtimes=()

usage() { sed -n '2,30p' "$0"; exit "${1:-0}"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="${2:?--version needs a value}"; shift ;;
    --runtime) runtimes+=("${2:?--runtime needs a value}"); shift ;;
    --bin-dir) bin_dir="${2:?--bin-dir needs a value}"; shift ;;
    --uninstall) uninstall=1 ;;
    --purge) purge=1 ;;
    --dry-run) dry=1 ;;
    -h|--help) usage 0 ;;
    *) echo "unknown arg: $1" >&2; usage 1 ;;
  esac
  shift
done

repo="${SKILL_GRAPH_RELEASE_REPO:-$repo}"
home="${SKILL_GRAPH_HOME:-$HOME/.local/share/skill-graph}"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

fetch() { # $1 = asset name, $2 = destination
  if [ -n "${SKILL_GRAPH_RELEASE_DIR:-}" ]; then
    cp "$SKILL_GRAPH_RELEASE_DIR/$1" "$2"
  elif [ -n "$version" ]; then
    curl -fsSL --retry 3 -o "$2" \
      "https://github.com/$repo/releases/download/v$version/$1"
  else
    curl -fsSL --retry 3 -o "$2" \
      "https://github.com/$repo/releases/latest/download/$1"
  fi
}

resolve_version() {
  if [ -n "$version" ]; then return; fi
  if [ -n "${SKILL_GRAPH_RELEASE_DIR:-}" ]; then
    version="$(basename "$(cd "$SKILL_GRAPH_RELEASE_DIR" && pwd)")"
    return
  fi
  local api
  api="$(curl -fsSL --retry 3 \
    "https://api.github.com/repos/$repo/releases/latest")"
  if command -v jq >/dev/null 2>&1; then
    version="$(printf '%s' "$api" | jq -r '.tag_name // empty' | sed 's/^v//')"
  else
    version="$(printf '%s' "$api" \
      | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p' | head -1)"
  fi
  [ -n "$version" ] || { echo "could not resolve latest release for $repo" >&2; exit 1; }
}

verify_sums() { # $1 = dir containing assets + sha256sums.txt
  local dir="$1" asset sum
  for asset in "skill-graph-$target.tar.gz" "skill-graph-adapters.tar.gz"; do
    sum="$(awk -v a="$asset" '$2 == a {print $1}' "$dir/sha256sums.txt")"
    [ -n "$sum" ] || { echo "no checksum entry for $asset" >&2; exit 1; }
    [ "$(sha256_of "$dir/$asset")" = "$sum" ] \
      || { echo "checksum mismatch for $asset" >&2; exit 1; }
  done
}

# --- uninstall -------------------------------------------------------------
uninstall_adapters() {
  local rt
  local -a targets=("${runtimes[@]}")
  [ "${#targets[@]}" -gt 0 ] || targets=(claude-code codex opencode)
  local adapter_sh="$home/current/adapters/install.sh"
  # Uninstall matching is marker-based, so ANY installed bundle works; prefer
  # `current`, else take whichever version dir exists.
  [ -f "$adapter_sh" ] || adapter_sh="$(find "$home" -path '*/adapters/install.sh' -type f 2>/dev/null | head -1 || true)"
  [ -n "$adapter_sh" ] && [ -f "$adapter_sh" ] || { echo "no installed adapter bundle found under $home" >&2; return 1; }
  for rt in "${targets[@]}"; do
    SKILL_GRAPH_HOME="$(cd "$(dirname "$adapter_sh")/.." && pwd)" \
      bash "$adapter_sh" "$rt" --uninstall --user
  done
}

if [ "$uninstall" = 1 ]; then
  uninstall_adapters || true
  if [ "$no_bin" != "1" ] && [ -e "$bin_dir/skill-graph" ]; then
    rm -f "$bin_dir/skill-graph"
    echo "removed $bin_dir/skill-graph"
  fi
  if [ "$purge" = 1 ]; then
    rm -rf "$home"
    echo "removed $home"
  else
    echo "kept $home (use --purge to delete versioned payloads)"
  fi
  exit 0
fi

# --- install ----------------------------------------------------------------
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64|Linux-arm64) target="aarch64-unknown-linux-gnu" ;;
  Darwin-arm64)  target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

resolve_version
version_dir="$home/$version"
assets="$(mktemp -d "${TMPDIR:-/tmp}/skill-graph-install.XXXXXX")"
trap 'rm -rf "$assets"' EXIT

echo "installing skill-graph $version ($target) -> $version_dir"
fetch "sha256sums.txt" "$assets/sha256sums.txt"
fetch "skill-graph-$target.tar.gz" "$assets/skill-graph-$target.tar.gz"
fetch "skill-graph-adapters.tar.gz" "$assets/skill-graph-adapters.tar.gz"
verify_sums "$assets"

if [ "$dry" = 1 ]; then
  echo "dry-run: would extract into $version_dir and point $home/current at it"
  exit 0
fi

mkdir -p "$version_dir"
tar -xzf "$assets/skill-graph-$target.tar.gz" -C "$version_dir"
tar -xzf "$assets/skill-graph-adapters.tar.gz" -C "$version_dir"
chmod +x "$version_dir/skill-graph" "$version_dir"/adapters/install.sh \
  "$version_dir"/adapters/*/skill-graph/bin/skill-graph-recommend-hook 2>/dev/null || true

if [ -e "$home/current" ] && [ ! -L "$home/current" ]; then
  echo "$home/current exists and is not a symlink; refusing to replace it" >&2
  exit 1
fi
rm -f "$home/current"
ln -s "$version" "$home/current"

if [ "$no_bin" != "1" ]; then
  mkdir -p "$bin_dir"
  ln -sfn "$home/current/skill-graph" "$bin_dir/skill-graph"
  echo "binary: $bin_dir/skill-graph -> $home/current/skill-graph"
fi

for rt in "${runtimes[@]}"; do
  case "$rt" in
    claude-code|codex|opencode) ;;
    *) echo "unknown runtime: $rt" >&2; exit 1 ;;
  esac
  SKILL_GRAPH_BIN="$bin_dir/skill-graph" \
  SKILL_GRAPH_HOME="$home/current" \
    bash "$home/current/adapters/install.sh" "$rt" --user
done

echo "done. skill-graph $version installed under $home/current"

#!/usr/bin/env bash
# check-plugin-types.sh — type-check the embedded OpenCode plugin
# against the REAL @opencode/plugin host API types (#225 F05).
#
# The plugin ships as a string asset inside the Rust binary; `bun build`
# only proves it parses. This script installs the pinned host package
# in a scratch dir and runs `tsc --noEmit` so API drift (renamed hooks,
# changed context shapes, removed exports) fails CI instead of
# surfacing as a silent no-op inside a user's OpenCode host.
#
# Versions are pinned exactly — bump them deliberately together with a
# re-verification that the plugin still type-checks.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"

PLUGIN_PKG="@opencode/plugin"
PLUGIN_PKG_VERSION="2.4.2"
BUN_TYPES_VERSION="1.4.5"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-plugin-types.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

cp "$root/src/assets/opencode-plugin.ts" "$tmp/"
cat > "$tmp/package.json" <<EOF
{
  "name": "skill-graph-plugin-typecheck",
  "private": true,
  "type": "module",
  "devDependencies": {
    "$PLUGIN_PKG": "$PLUGIN_PKG_VERSION",
    "bun-types": "$BUN_TYPES_VERSION"
  }
}
EOF
cat > "$tmp/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "strict": true,
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "types": ["bun-types"],
    "skipLibCheck": true,
    "noEmit": true
  },
  "include": ["opencode-plugin.ts"]
}
EOF

cd "$tmp"
bun install --silent
bunx tsc --noEmit -p tsconfig.json

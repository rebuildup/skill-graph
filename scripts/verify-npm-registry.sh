#!/usr/bin/env bash
# verify-npm-registry.sh — prove the artifact the npm registry serves
# for <name>@<version> is byte-for-byte this build's package content.
# Runs AFTER `npm publish` and on the rerun-skip path, so "published"
# always means "verified identical", never merely "present".
#
# This is a separate process with its own `set -euo pipefail`: callers
# may invoke it under `||`/`if` without disabling failure detection
# inside it — the bug class that let a failed diff pass as success in
# the previous inline-function form (#225 re-review).
#
# Usage: verify-npm-registry.sh <name> <version> <package-dir>
set -euo pipefail

name="${1:?usage: verify-npm-registry.sh <name> <version> <package-dir>}"
ver="${2:?usage: verify-npm-registry.sh <name> <version> <package-dir>}"
pkg="${3:?usage: verify-npm-registry.sh <name> <version> <package-dir>}"

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/sg-regverify.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/pack"

url="$(npm view "$name@$ver" dist.tarball)"
[ -n "$url" ] || { echo "$name@$ver has no dist.tarball" >&2; exit 1; }
curl -fsSL "$url" -o "$work/published.tgz"

npm pack "./$pkg" --pack-destination "$work/pack" >/dev/null
local_tgz="$(find "$work/pack" -name '*.tgz' | head -1)"
[ -n "$local_tgz" ] || { echo "npm pack produced no tarball for $pkg" >&2; exit 1; }

"$here/verify-npm-tarballs.sh" "$work/published.tgz" "$local_tgz"

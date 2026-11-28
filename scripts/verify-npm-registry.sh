#!/usr/bin/env bash
# verify-npm-registry.sh — prove the artifact the npm registry serves
# for <name>@<version> is identical to the FIXED packed tarball this
# build produced. Runs AFTER `npm publish` and on the rerun-skip path,
# so "published" always means "verified identical", never merely
# "present".
#
# The expected artifact is the .tgz that publish-npm.sh packed and
# verified BEFORE publishing — never a re-pack of the source dir.
# Re-packing could diverge from what was verified (mtime-dependent
# tar metadata, files[] drift between pack and publish), reopening
# the verify-then-ship gap this script exists to close (#225 r6).
#
# This is a separate process with its own `set -euo pipefail`: callers
# may invoke it under `||`/`if` without disabling failure detection
# inside it — the bug class that let a failed diff pass as success in
# the previous inline-function form (#225 re-review).
#
# Usage: verify-npm-registry.sh <name> <version> <expected.tgz>
set -euo pipefail

name="${1:?usage: verify-npm-registry.sh <name> <version> <expected.tgz>}"
ver="${2:?usage: verify-npm-registry.sh <name> <version> <expected.tgz>}"
tgz="${3:?usage: verify-npm-registry.sh <name> <version> <expected.tgz>}"

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/sg-regverify.XXXXXX")"
trap 'rm -rf "$work"' EXIT

[ -f "$tgz" ] || { echo "expected tarball not found: $tgz" >&2; exit 1; }

url="$(npm view "$name@$ver" dist.tarball)"
[ -n "$url" ] || { echo "$name@$ver has no dist.tarball" >&2; exit 1; }
curl -fsSL "$url" -o "$work/published.tgz"

"$here/verify-npm-tarballs.sh" "$work/published.tgz" "$tgz"

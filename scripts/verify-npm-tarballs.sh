#!/usr/bin/env bash
# verify-npm-tarballs.sh — prove two npm tarballs carry identical
# package content. Used by npm-publish.yml to verify that what the
# registry serves equals what this build packed, on BOTH the
# rerun-skip path and immediately after a fresh `npm publish`.
#
# Every step is checked explicitly: this script is also invoked from
# contexts where errexit may be suspended, so no step may rely on
# `set -e` alone — each fallible command returns non-zero directly
# (#225 re-review).
#
# Usage: verify-npm-tarballs.sh <published.tgz> <local.tgz>
set -euo pipefail

pub="${1:?usage: verify-npm-tarballs.sh <published.tgz> <local.tgz>}"
loc="${2:?usage: verify-npm-tarballs.sh <published.tgz> <local.tgz>}"

[ -f "$pub" ] || { echo "published tarball not found: $pub" >&2; exit 1; }
[ -f "$loc" ] || { echo "local tarball not found: $loc" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-verify.XXXXXX")" || exit 1
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

mkdir -p "$work/pub" "$work/local" || exit 1
# Extraction failure (corrupt gzip, non-tar, hostile path entries) is a
# rejection, not a skip — tar's own checks apply.
tar -xzf "$pub" -C "$work/pub" || { echo "cannot extract published tarball" >&2; exit 1; }
tar -xzf "$loc" -C "$work/local" || { echo "cannot extract local tarball" >&2; exit 1; }

# npm tarballs extract under package/; anything else is malformed.
[ -d "$work/pub/package" ] || { echo "published tarball lacks package/ root" >&2; exit 1; }
[ -d "$work/local/package" ] || { echo "local tarball lacks package/ root" >&2; exit 1; }

# Content identity: diff -r reports missing/extra files and content
# differences. It does NOT compare permission bits — a published
# tarball that lost the exec bit on bin/skill-graph would pass a
# content-only diff while breaking every install (#225 re-review).
diff -r "$work/pub/package" "$work/local/package" || {
  echo "package trees differ" >&2
  exit 1
}

# Mode/type identity: every entry's permission bits, file type, and
# path must match exactly.
( cd "$work/pub/package" && find . -printf '%m %y %p\n' | LC_ALL=C sort ) \
  > "$work/pub.mode" || exit 1
( cd "$work/local/package" && find . -printf '%m %y %p\n' | LC_ALL=C sort ) \
  > "$work/local.mode" || exit 1
diff "$work/pub.mode" "$work/local.mode" || {
  echo "package entry modes/types differ" >&2
  exit 1
}

echo "tarballs identical: $(basename "$loc") == published $(basename "$pub")"

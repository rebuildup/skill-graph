#!/usr/bin/env bash
# verify-npm-tarballs.sh — prove two npm tarballs carry identical
# package content. Used by npm-publish.yml to verify that what the
# registry serves equals the FIXED publish artifact this build
# produced, on BOTH the rerun-skip path and immediately after a fresh
# `npm publish` (#225 r6: the compared artifact is the packed .tgz
# itself, never a re-pack).
#
# Every step is checked explicitly: this script is also invoked from
# contexts where errexit may be suspended, so no step may rely on
# `set -e` alone — each fallible command returns non-zero directly
# (#225 re-review).
#
# Usage: verify-npm-tarballs.sh <published.tgz> <local.tgz>
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

pub="${1:?usage: verify-npm-tarballs.sh <published.tgz> <local.tgz>}"
loc="${2:?usage: verify-npm-tarballs.sh <published.tgz> <local.tgz>}"

[ -f "$pub" ] || { echo "published tarball not found: $pub" >&2; exit 1; }
[ -f "$loc" ] || { echo "local tarball not found: $loc" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-verify.XXXXXX")" || exit 1
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

# Archive-format gate on BOTH tarballs before extraction — gzip
# integrity, canonical package/-rooted member names with no
# normalization collisions or duplicates, and regular-file/dir member
# types only. Shared with the pre-publish artifact check so the two
# verifiers cannot drift apart (#225 r6).
"$here/verify-npm-archive.sh" "$pub" || exit 1
"$here/verify-npm-archive.sh" "$loc" || exit 1

mkdir -p "$work/pub" "$work/local" || exit 1
# Extraction failure (corrupt gzip, non-tar, hostile path entries) is a
# rejection, not a skip — tar's own checks apply.
tar -xzf "$pub" -C "$work/pub" || { echo "cannot extract published tarball" >&2; exit 1; }
tar -xzf "$loc" -C "$work/local" || { echo "cannot extract local tarball" >&2; exit 1; }

# Scope guard: an npm tarball's root must contain EXACTLY `package/`.
# Comparing only the package/ subtrees would silently accept extra
# top-level entries ( stray files, nested dirs, second roots ) that
# ship to every install (#225 re-review).
for side in pub local; do
  entries="$(cd "$work/$side" && find . -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort)"
  [ "$entries" = "package" ] || {
    echo "$side tarball root must contain only package/ — found: ${entries:-<empty>}" >&2
    exit 1
  }
done

# Content identity over the WHOLE extracted tree — not just package/ —
# so nothing outside the compared scope can differ (#225 re-review).
# diff -r reports missing/extra files and content differences. It does
# NOT compare permission bits — a published tarball that lost the exec
# bit on bin/skill-graph would pass a content-only diff while breaking
# every install (#225 re-review).
diff -r "$work/pub" "$work/local" || {
  echo "package trees differ" >&2
  exit 1
}

# Mode/type identity: every entry's permission bits, file type, and
# path must match exactly.
( cd "$work/pub" && find . -printf '%m %y %p\n' | LC_ALL=C sort ) \
  > "$work/pub.mode" || exit 1
( cd "$work/local" && find . -printf '%m %y %p\n' | LC_ALL=C sort ) \
  > "$work/local.mode" || exit 1
diff "$work/pub.mode" "$work/local.mode" || {
  echo "package entry modes/types differ" >&2
  exit 1
}

echo "tarballs identical: $(basename "$loc") == published $(basename "$pub")"

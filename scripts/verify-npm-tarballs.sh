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

# Archive-format gate, applied to BOTH tarballs before extraction
# (#225 r5). `tar -xzf` alone only proves "gzip-compressed tar that
# GNU tar accepts" — it does not prove the archive is a well-formed
# npm tarball: a different compression would fail extraction anyway,
# but duplicate members (last-wins at extract), members outside
# package/, and non-regular types (symlink/hardlink/device) would all
# slip through to the tree diff in their FINAL extracted state.
check_archive() { # $1=file $2=label
  f="$1"; label="$2"
  # gzip magic + integrity: reject zstd/xz/uncompressed-renamed/.tgz
  # and truncated/corrupt streams explicitly rather than relying on
  # tar's error path alone.
  magic="$(od -An -tx1 -N2 "$f" 2>/dev/null | tr -d ' \n')" || exit 1
  [ "$magic" = "1f8b" ] || {
    echo "$label is not gzip-compressed (magic=$magic)" >&2; exit 1; }
  gzip -t "$f" 2>/dev/null || {
    echo "$label fails gzip integrity check" >&2; exit 1; }

  # Member names: every entry must live under package/. This rejects
  # ../ traversal, absolute paths, and stray top-level members in the
  # ARCHIVE itself (not just after extraction). A member name
  # containing a newline splits into extra lines here, and any
  # continuation line fails the prefix check — fail-closed.
  tar -tzf "$f" > "$work/$label.names" 2>/dev/null || {
    echo "$label is not a readable tar archive" >&2; exit 1; }
  [ -s "$work/$label.names" ] || {
    echo "$label is an empty archive" >&2; exit 1; }
  bad="$(grep -v '^package/' "$work/$label.names" || true)"
  [ -z "$bad" ] || {
    echo "$label has members outside package/: $(echo "$bad" | head -3)" >&2
    exit 1; }
  # `package/../x` passes the prefix check but is still a traversal
  # member — GNU tar refuses it at extract, so reject it here where
  # the diagnosis is precise (#225 r5).
  dotdot="$(grep -nE '(^|/)\.\.(/|$)' "$work/$label.names" || true)"
  [ -z "$dotdot" ] || {
    echo "$label has traversal members: $(echo "$dotdot" | head -3)" >&2
    exit 1; }
  # Duplicate member names: extraction is last-wins, so the compared
  # tree would hide an earlier divergent entry entirely.
  dups="$(sort "$work/$label.names" | uniq -d || true)"
  [ -z "$dups" ] || {
    echo "$label has duplicate members: $(echo "$dups" | head -3)" >&2
    exit 1; }

  # Member types: tar -tv's mode field leads with the typeflag —
  # '-' regular file, 'd' directory are the only shapes an npm
  # tarball may contain. 'l'/'h'/'c'/'b'/'p' are rejected outright
  # even when both sides would extract identically: installs must
  # never materialize links or device nodes from a package (#225 r5).
  tar -tvzf "$f" 2>/dev/null | awk '
    substr($1, 1, 1) != "-" && substr($1, 1, 1) != "d" { print; bad = 1 }
    END { exit bad ? 1 : 0 }
  ' > "$work/$label.badtypes" || {
    echo "$label has non-regular members: $(head -3 "$work/$label.badtypes")" >&2
    exit 1; }
}

check_archive "$pub" pub
check_archive "$loc" local

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

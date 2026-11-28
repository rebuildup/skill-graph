#!/usr/bin/env bash
# verify-npm-archive.sh — prove a single .tgz is a well-formed npm
# tarball BEFORE it is published or compared. This is the artifact-
# level gate: what passes here is the fixed publish candidate whose
# bytes are later verified against the registry verbatim (#225 r6).
#
# Checks, all explicit (no reliance on errexit surviving a caller's
# `||`/`if` context — this runs standalone):
#   1. gzip magic bytes + gzip integrity
#   2. member names: canonical form only — every entry under
#      package/, no "//", no "/./", no "."/".." segments, at most one
#      trailing "/" — so names that would NORMALIZE to the same path
#      ("package//a" vs "package/a", "package/./a" vs "package/a",
#      "package/a/" vs "package/a") can never collide silently at
#      extraction (#225 r6)
#   3. no duplicate members under the canonical (slash-stripped) key —
#      extraction is last-wins, so a divergent earlier entry would be
#      hidden from any tree comparison
#   4. member types via tar -tv: only regular files ('-') and
#      directories ('d') — symlinks, hardlinks, devices, fifos are
#      rejected even when both sides of a comparison would extract
#      identically
#   5. --check-bin: extract and run verify-npm-bin.sh on every
#      package/bin/* entry (executable bit AND executable format)
#
# Usage: verify-npm-archive.sh [--check-bin] <file.tgz>
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"

check_bin=0
if [ "${1:-}" = "--check-bin" ]; then
  check_bin=1
  shift
fi
f="${1:?usage: verify-npm-archive.sh [--check-bin] <file.tgz>}"

[ -f "$f" ] || { echo "tarball not found: $f" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-archverify.XXXXXX")" || exit 1
trap 'rm -rf "$work"' EXIT

# 1. gzip magic + integrity: reject zstd/xz/uncompressed-renamed/.tgz
#    and truncated/corrupt streams explicitly.
magic="$(od -An -tx1 -N2 "$f" 2>/dev/null | tr -d ' \n')" || exit 1
[ "$magic" = "1f8b" ] || {
  echo "$f is not gzip-compressed (magic=$magic)" >&2; exit 1; }
gzip -t "$f" 2>/dev/null || {
  echo "$f fails gzip integrity check" >&2; exit 1; }

# 2+3. Member names: canonical form, package/ root, no duplicates.
# tar -tzf emits one name per line; a name containing a newline splits
# into extra lines and any continuation line fails the checks below —
# fail-closed.
tar -tzf "$f" > "$work/names" 2>/dev/null || {
  echo "$f is not a readable tar archive" >&2; exit 1; }
[ -s "$work/names" ] || { echo "$f is an empty archive" >&2; exit 1; }

awk '
  {
    n = $0
    key = n
    sub(/\/$/, "", key)   # dedup key ignores ONE trailing slash
    if (n !~ /^package\//) { print "PREFIX " n > "/dev/stderr"; bad = 1; next }
    m = split(n, seg, "/")
    ok = 1
    for (i = 2; i <= m; i++) {
      s = seg[i]
      if (s == "." || s == "..") { ok = 0; break }
      # Empty segment: allowed ONLY as the final element produced by
      # a single trailing "/" (directory marker). "//" anywhere —
      # interior or doubled at the end — yields an empty non-final
      # segment and is rejected.
      if (s == "" && i != m) { ok = 0; break }
    }
    if (!ok) { print "NONCANON " n > "/dev/stderr"; bad = 1; next }
    if (seen[key]++) { print "DUP " key > "/dev/stderr"; bad = 1 }
  }
  END { exit bad ? 1 : 0 }
' "$work/names" || {
  echo "$f has non-canonical, out-of-root, or duplicate members" >&2
  exit 1
}

# 4. Member types: tar -tv's mode field leads with the typeflag —
#    '-' regular file, 'd' directory are the only shapes an npm
#    tarball may contain.
tar -tvzf "$f" 2>/dev/null | awk '
  substr($1, 1, 1) != "-" && substr($1, 1, 1) != "d" { print; bad = 1 }
  END { exit bad ? 1 : 0 }
' > "$work/badtypes" || {
  echo "$f has non-regular members: $(head -3 "$work/badtypes")" >&2
  exit 1; }

# 5. Optional: verify executable format of every bin/* payload INSIDE
#    the fixed artifact — the bytes that will actually be published,
#    not just the on-disk sources they were packed from.
if [ "$check_bin" -eq 1 ]; then
  mkdir -p "$work/x" || exit 1
  tar -xzf "$f" -C "$work/x" || {
    echo "$f cannot be extracted for bin verification" >&2; exit 1; }
  if [ -d "$work/x/package/bin" ]; then
    while IFS= read -r -d '' b; do
      "$here/verify-npm-bin.sh" "$b" || exit 1
    done < <(find "$work/x/package/bin" -mindepth 1 -type f -print0)
  fi
fi

echo "archive ok: $(basename "$f")"

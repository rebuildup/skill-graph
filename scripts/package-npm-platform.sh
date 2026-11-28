#!/usr/bin/env bash
# package-npm-platform.sh — pack ONE platform npm package around the
# FROZEN release tarball payload and prove the resulting .tgz is a
# well-formed npm artifact carrying the exact verified bytes (#225
# A06: every distribution channel's installable artifact is frozen
# and digest-bound, not rebuilt at publish time).
#
# The payload is extracted from skill-graph-<triple>.tar.gz — the
# same bytes package-target.sh produced, natively verified, and
# digested — never from the build tree. `cp -p` preserves the
# tarball's exec bit exactly; no chmod repair (#225 r10).
#
# Usage: package-npm-platform.sh <npm-target> <frozen-tarball>
#   e.g. package-npm-platform.sh linux-x64 skill-graph-x86_64-unknown-linux-gnu.tar.gz
# Output: ./rebuildup-skill-graph-<npm-target>-<ver>.tgz + .sha256
#         in the cwd.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

npm_target="${1:?usage: package-npm-platform.sh <npm-target> <frozen-tarball>}"
tgz="${2:?usage: package-npm-platform.sh <npm-target> <frozen-tarball>}"
[ -f "$tgz" ] || { echo "frozen tarball not found: $tgz" >&2; exit 1; }

pkg_src="npm/skill-graph-${npm_target}"
[ -d "$pkg_src" ] || { echo "npm package dir missing: $pkg_src" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-npmpkg.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# Stage the package in a scratch dir — the repo tree is never
# mutated by freeze-time packing.
mkdir -p "$work/pkg" "$work/x"
cp -r "$pkg_src/." "$work/pkg/"
mkdir -p "$work/pkg/bin"
tar -xzf "$tgz" -C "$work/x"
cp -p "$work/x/skill-graph" "$work/pkg/bin/skill-graph"
[ -x "$work/pkg/bin/skill-graph" ] || {
  echo "frozen payload lost exec bit" >&2; exit 1; }

# The staged payload must BE the frozen tarball's member bytes —
# compare digests, not just presence.
sha256_stream() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum | awk '{print $1}';
  else shasum -a 256 | awk '{print $1}'; fi
}
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}
inner="$(tar -xzOf "$tgz" skill-graph | sha256_stream)"
got="$(sha256_of "$work/pkg/bin/skill-graph")"
[ "$got" = "$inner" ] || {
  echo "payload digest mismatch: staged bin != tarball member" >&2; exit 1; }

outdir="$work/out"
mkdir -p "$outdir"
packed="$(cd "$work/pkg" && npm pack --silent --pack-destination "$outdir" | tail -1)"
[ -f "$outdir/$packed" ] || { echo "npm pack produced no tarball" >&2; exit 1; }

# Wellformedness + exec-format of bin/* INSIDE the artifact, and the
# declared target must match this npm_target's architecture.
"$here/verify-npm-archive.sh" --check-bin "$outdir/$packed"
"$here/verify-npm-arch.sh" "$npm_target" "$work/pkg/bin/skill-graph"

# The artifact manifest must declare THIS package — a drifted
# package.json would publish under a different name/version.
node -e '
  const { execFileSync } = require("node:child_process");
  const m = JSON.parse(execFileSync("tar", ["-xzOf", process.argv[1], "package/package.json"], {encoding:"utf8"}));
  const want = process.argv[2];
  if (m.name !== "@rebuildup/skill-graph-" + want) {
    console.error(`artifact name ${m.name} != @rebuildup/skill-graph-${want}`); process.exit(1);
  }
' "$outdir/$packed" "$npm_target"

cp "$outdir/$packed" "./$packed"
shasum -a 256 "./$packed" > "./$packed.sha256" 2>/dev/null \
  || sha256sum "./$packed" > "./$packed.sha256"
echo "packaged+verified: $packed"

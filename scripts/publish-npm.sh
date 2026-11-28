#!/usr/bin/env bash
# publish-npm.sh — publish npm packages IN ORDER, proving registry
# content identity on EVERY path: rerun-skip AND fresh publish alike.
# What the registry serves must equal the FIXED packed artifact this
# build produced — presence of a version is never treated as
# identity, and a re-pack is never substituted for the verified
# bytes (#225 r6).
#
# PHASE STRUCTURE (#225 r5/r6):
#   A. Structured manifest preflight (scripts/pkg-preflight.mjs — a
#      real JSON parser, not string extraction): name/version present,
#      files[] payloads exist, dependency order respected.
#   B. bin entries: regular files, executable bit AND executable
#      format (verify-npm-bin.sh).
#   C. `npm pack` EVERY package into fixed .tgz artifacts.
#   D. verify-npm-archive.sh --check-bin on EVERY artifact: gzip
#      integrity, canonical member names (no normalization collisions,
#      no duplicates, package/ root only), regular/dir types only,
#      and executable format of bin/* INSIDE the artifact.
#   E. Only now publish: each verified .tgz is published directly
#      (`npm publish <tgz>` — the exact bytes that passed D), then
#      verify-npm-registry.sh proves the registry serves those same
#      bytes. The skip path compares the registry tarball against
#      the same fixed artifact.
#
# Any failure in A–D means ZERO publishes; a failure in E leaves only
# already-verified predecessors published, each proven identical to
# the registry afterwards.
#
# Extracted from npm-publish.yml so the entire publish decision path
# is exercised by scripts/test-npm-publish.sh against a stub npm —
# the workflow itself cannot run without NPM_TOKEN (#225 re-review).
# Requires: bash, node (npm itself is a Node program, so node is
# always present where npm is), npm, curl, file, tar, awk, find.
#
# Every step is a separate process or an explicit `|| exit`: nothing
# relies on errexit surviving a caller's `||`/`if` context.
#
# Usage: publish-npm.sh <package-dir> [<package-dir>...]
# Auth: npm's normal resolution (NODE_AUTH_TOKEN / .npmrc).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
[ "$#" -gt 0 ] || { echo "usage: publish-npm.sh <package-dir>..." >&2; exit 1; }

# ------------------------------------------------------------------
# Phase A: structured manifest preflight for EVERY package. Real
# JSON parsing via node — sed/awk extraction mis-parses nested or
# unusually formatted manifests (#225 r6). No pack/publish may happen
# until all manifests validate.
# ------------------------------------------------------------------
meta="$(node "$here/pkg-preflight.mjs" "$@")" || exit 1

declare -a pkg_names=() pkg_vers=() pkg_files=()
while IFS=$'\t' read -r n v f; do
  [ -n "$n" ] && [ -n "$v" ] && [ -n "$f" ] || {
    echo "pkg-preflight emitted malformed record: '$n'$'\t''$v'" >&2; exit 1; }
  pkg_names+=("$n")
  pkg_vers+=("$v")
  pkg_files+=("$f")
done <<< "$meta"
[ "${#pkg_names[@]}" -eq "$#" ] || {
  echo "pkg-preflight record count mismatch" >&2; exit 1; }

# ------------------------------------------------------------------
# Phase B: bin entries on disk — regular files that are BOTH
# mode-executable and executable-format. Cheap early rejection before
# packing; the authoritative check re-runs INSIDE each artifact in
# Phase D (#225 r5/r6).
# ------------------------------------------------------------------
for pkg in "$@"; do
  if [ -d "$pkg/bin" ]; then
    while IFS= read -r -d '' f; do
      [ -f "$f" ] || { echo "bin entry is not a regular file: $f" >&2; exit 1; }
      "$here/verify-npm-bin.sh" "$f" || exit 1
    done < <(find "$pkg/bin" -mindepth 1 -print0)
  fi
done

# ------------------------------------------------------------------
# Phase C: pack EVERY package into fixed artifacts BEFORE any publish.
# Phase D: verify each artifact itself. Both complete for ALL packages
# before Phase E publishes anything — a late artifact failure must
# not leave a partial release (#225 r5).
# ------------------------------------------------------------------
work="$(mktemp -d "${TMPDIR:-/tmp}/sg-publish.XXXXXX")" || exit 1
trap 'rm -rf "$work"' EXIT

declare -a tgzs=()
i=0
for pkg in "$@"; do
  # npm pack resolves its argument as a package SPEC, not a path:
  # a bare "npm/foo" is interpreted as the GitHub repository
  # github:npm/foo (verified against real npm — it runs
  # `git ls-remote ssh://git@github.com/npm/foo.git`), so a missing
  # or renamed directory would silently pack someone else's repo.
  # An absolute path is unambiguous: it can only be a local folder.
  pkg_abs="$(cd "$pkg" 2>/dev/null && pwd)" || {
    echo "package directory does not exist: $pkg" >&2; exit 1; }
  [ -f "$pkg_abs/package.json" ] || {
    echo "package directory has no package.json: $pkg" >&2; exit 1; }
  pdir="$work/pack-$i"
  mkdir -p "$pdir" || exit 1
  npm pack "$pkg_abs" --pack-destination "$pdir" >/dev/null || {
    echo "npm pack failed for $pkg" >&2; exit 1; }
  tgz="$(find "$pdir" -name '*.tgz' -print -quit)"
  [ -n "$tgz" ] || { echo "npm pack produced no tarball for $pkg" >&2; exit 1; }
  # Exactly one artifact per pack dir — a second .tgz would make the
  # publish target ambiguous.
  [ "$(find "$pdir" -name '*.tgz' | wc -l)" -eq 1 ] || {
    echo "npm pack produced multiple tarballs for $pkg" >&2; exit 1; }

  # Phase D: the FIXED artifact must itself be a well-formed npm
  # tarball with executable-format bin payloads — verified bytes are
  # the bytes that get published.
  "$here/verify-npm-archive.sh" --check-bin "$tgz" || exit 1

  # The artifact must BE the package preflight validated: identity of
  # name/version inside package/package.json, and every declared
  # files[] payload present as an archive member. npm could otherwise
  # pack a different directory or silently drop payloads (#225 r7).
  FILES_JSON="${pkg_files[$i]}" WANT_NAME="${pkg_names[$i]}" \
    WANT_VER="${pkg_vers[$i]}" TGZ="$tgz" node -e '
      const { execFileSync } = require("node:child_process");
      const tar = execFileSync("tar", ["-xzOf", process.env.TGZ, "package/package.json"], { encoding: "utf8" });
      const m = JSON.parse(tar);
      if (m.name !== process.env.WANT_NAME || m.version !== process.env.WANT_VER) {
        console.error(`artifact identity mismatch: packed ${m.name}@${m.version}, expected ${process.env.WANT_NAME}@${process.env.WANT_VER}`);
        process.exit(1);
      }
      const members = execFileSync("tar", ["-tzf", process.env.TGZ], { encoding: "utf8" }).split("\n");
      const want = JSON.parse(process.env.FILES_JSON);
      for (const f of want) {
        const prefix = "package/" + f.replace(/\/+$/, "");
        const hit = members.some((mem) => mem === prefix || mem === prefix + "/" || mem.startsWith(prefix + "/"));
        if (!hit) {
          console.error(`required files[] payload missing from artifact: ${f}`);
          process.exit(1);
        }
      }
    ' || exit 1
  tgzs+=("$tgz")
  i=$((i + 1))
done

# ------------------------------------------------------------------
# Phase E1: reconcile the registry for EVERY package BEFORE any
# publish. An already-published version must be proven identical to
# this build's fixed artifact — discovering a divergent published
# version for a LATER package after earlier ones shipped would leave
# a partial, inconsistent release (#225 r7).
# ------------------------------------------------------------------
declare -a missing=()
i=0
for tgz in "${tgzs[@]}"; do
  name="${pkg_names[$i]}"
  ver="${pkg_vers[$i]}"
  if npm view "$name@$ver" version >/dev/null 2>&1; then
    echo "$name@$ver already published — verifying content identity"
    "$here/verify-npm-registry.sh" "$name" "$ver" "$tgz" || {
      echo "published $name@$ver differs from this build — refusing to publish anything" >&2
      exit 1
    }
  else
    missing+=("$i")
  fi
  i=$((i + 1))
done

# ------------------------------------------------------------------
# Phase E2: publish only the missing artifacts, in argument order.
# ------------------------------------------------------------------
for i in "${missing[@]}"; do
  name="${pkg_names[$i]}"
  ver="${pkg_vers[$i]}"
  npm publish "${tgzs[$i]}" --access public --provenance || {
    echo "npm publish failed for $name@$ver" >&2; exit 1; }
done

# ------------------------------------------------------------------
# Phase E3: prove the registry now serves THIS build's fixed bytes
# for EVERY package — freshly published and previously skipped alike.
# ------------------------------------------------------------------
i=0
for tgz in "${tgzs[@]}"; do
  name="${pkg_names[$i]}"
  ver="${pkg_vers[$i]}"
  "$here/verify-npm-registry.sh" "$name" "$ver" "$tgz" || {
    echo "registry does not serve this build's $name@$ver" >&2
    exit 1
  }
  i=$((i + 1))
done

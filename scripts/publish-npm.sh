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

declare -a pkg_names=() pkg_vers=()
while IFS=$'\t' read -r n v; do
  [ -n "$n" ] && [ -n "$v" ] || {
    echo "pkg-preflight emitted malformed record: '$n'$'\t''$v'" >&2; exit 1; }
  pkg_names+=("$n")
  pkg_vers+=("$v")
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
  pdir="$work/pack-$i"
  mkdir -p "$pdir" || exit 1
  npm pack "$pkg" --pack-destination "$pdir" >/dev/null || {
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
  tgzs+=("$tgz")
  i=$((i + 1))
done

# ------------------------------------------------------------------
# Phase E: publish the verified artifacts in argument order (already
# validated against the dependency graph in Phase A) and prove the
# registry serves the SAME fixed bytes afterwards.
# ------------------------------------------------------------------
i=0
for tgz in "${tgzs[@]}"; do
  name="${pkg_names[$i]}"
  ver="${pkg_vers[$i]}"

  if npm view "$name@$ver" version >/dev/null 2>&1; then
    # Version exists: presence is not identity — a rerun must prove
    # the published tarball carries THIS build's fixed artifact, not
    # silently skip a divergent artifact (#225).
    echo "$name@$ver already published — verifying content identity"
    "$here/verify-npm-registry.sh" "$name" "$ver" "$tgz" || {
      echo "published $name@$ver differs from this build — refusing to skip" >&2
      exit 1
    }
    i=$((i + 1))
    continue
  fi

  # Publish the packed artifact itself: `npm publish <tgz>` ships the
  # exact bytes Phase D verified — no re-pack gap (#225 r6).
  npm publish "$tgz" --access public --provenance || {
    echo "npm publish failed for $name@$ver" >&2; exit 1; }
  # Post-publish verification: the registry must serve THIS artifact.
  "$here/verify-npm-registry.sh" "$name" "$ver" "$tgz" || {
    echo "published $name@$ver does not match this build" >&2
    exit 1
  }
  i=$((i + 1))
done

#!/usr/bin/env bash
# publish-npm.sh — publish npm packages IN ORDER, proving registry
# content identity on EVERY path: rerun-skip AND fresh publish alike.
# What the registry serves must equal this build's packed tree —
# presence of a version is never treated as identity (#225).
#
# Extracted from npm-publish.yml so the entire publish decision path
# is exercised by scripts/test-npm-publish.sh against a stub npm —
# the workflow itself cannot run without NPM_TOKEN (#225 re-review).
# Pure shell + the verify-* scripts: no node/python dependency, so it
# runs in any CI job that has bash, npm, curl, and tar.
#
# Every step is a separate process or an explicit `|| exit`: nothing
# relies on errexit surviving a caller's `||`/`if` context.
#
# Usage: publish-npm.sh <package-dir> [<package-dir>...]
# Auth: npm's normal resolution (NODE_AUTH_TOKEN / .npmrc).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
[ "$#" -gt 0 ] || { echo "usage: publish-npm.sh <package-dir>..." >&2; exit 1; }

# First "key": "value" string pair in a repo-controlled package.json.
# These manifests are machine-maintained with name/version at top
# level and exactly one occurrence each (CI asserts version agreement
# separately); a missing or duplicated field fails closed below.
pkg_field() { # $1=pkg-dir $2=field
  sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" \
    "$1/package.json" | head -1
}

for pkg in "$@"; do
  [ -f "$pkg/package.json" ] || { echo "missing $pkg/package.json" >&2; exit 1; }

  # Every shipped bin entry must be executable BEFORE publish — a
  # package whose bin lost its exec bit fails every install, not CI.
  # Platform packages ship bin/skill-graph; the metapackage ships
  # bin/skill-graph.js. bin/ holds executables only by convention.
  if [ -d "$pkg/bin" ]; then
    while IFS= read -r -d '' f; do
      [ -x "$f" ] || { echo "bin not executable: $f" >&2; exit 1; }
    done < <(find "$pkg/bin" -mindepth 1 -maxdepth 1 ! -type d -print0)
  fi

  name="$(pkg_field "$pkg" name)"
  ver="$(pkg_field "$pkg" version)"
  [ -n "$name" ] && [ -n "$ver" ] || {
    echo "cannot read name/version from $pkg/package.json" >&2
    exit 1
  }

  if npm view "$name@$ver" version >/dev/null 2>&1; then
    # Version exists: presence is not identity — a rerun must prove
    # the published tarball carries THIS build's content, not silently
    # skip a divergent artifact (#225).
    echo "$name@$ver already published — verifying content identity"
    "$here/verify-npm-registry.sh" "$name" "$ver" "$pkg" || {
      echo "published $name@$ver differs from this build — refusing to skip" >&2
      exit 1
    }
    continue
  fi

  npm publish "./$pkg" --access public --provenance
  # Post-publish verification: the registry must serve THIS build's
  # content — publish exit status alone does not prove what landed.
  "$here/verify-npm-registry.sh" "$name" "$ver" "$pkg" || {
    echo "published $name@$ver does not match this build" >&2
    exit 1
  }
done

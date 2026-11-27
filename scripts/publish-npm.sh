#!/usr/bin/env bash
# publish-npm.sh — publish npm packages IN ORDER, proving registry
# content identity on EVERY path: rerun-skip AND fresh publish alike.
# What the registry serves must equal this build's packed tree —
# presence of a version is never treated as identity (#225).
#
# TWO-PHASE structure (#225 r5): EVERY package is fully preflighted —
# manifest readable, declared files[] present, bin entries executable
# AND executable-format, dependency order respected — BEFORE the first
# `npm publish` runs. The previous interleaved loop could publish
# package A and then fail package B's checks, leaving a partial
# release; now any preflight failure means zero publishes.
#
# Extracted from npm-publish.yml so the entire publish decision path
# is exercised by scripts/test-npm-publish.sh against a stub npm —
# the workflow itself cannot run without NPM_TOKEN (#225 re-review).
# Pure shell + the verify-* scripts: no node/python dependency, so it
# runs in any CI job that has bash, npm, curl, file, and tar.
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

# Quoted entries of the "files" array — every declared payload path
# must exist before publish. awk state machine, NOT a sed line range:
# a sed range cannot terminate on its start line, so a single-line
# files array would leak into the following fields (#225 r5 — the
# publish-path tests caught "optionalDependencies" being checked as
# a required file).
pkg_files() { # $1=pkg-dir
  awk '
    /"files"[[:space:]]*:[[:space:]]*\[/ { inf = 1 }
    inf {
      l = $0
      if (l ~ /\]/) { inf = 0; sub(/\].*/, "", l) }
      while (match(l, /"[^"]*"/)) {
        print substr(l, RSTART + 1, RLENGTH - 2)
        l = substr(l, RSTART + RLENGTH)
      }
    }
  ' "$1/package.json" | grep -vx 'files' || true
}

# Package names under "dependencies"/"optionalDependencies" — for the
# publish-order invariant (dependents must not precede their deps).
# Same awk rationale as pkg_files; the block key itself is filtered.
pkg_deps() { # $1=pkg-dir
  awk '
    /"(optional)?[dD]ependencies"[[:space:]]*:[[:space:]]*\{/ { indep = 1 }
    indep {
      l = $0
      if (l ~ /\}/) { indep = 0; sub(/\}.*/, "", l) }
      while (match(l, /"[^"]*"[[:space:]]*:/)) {
        t = substr(l, RSTART, RLENGTH)
        gsub(/[": \t]/, "", t)
        print t
        l = substr(l, RSTART + RLENGTH)
      }
    }
  ' "$1/package.json" \
    | grep -vx -e dependencies -e optionalDependencies || true
}

# ------------------------------------------------------------------
# Phase 1: preflight EVERY package. No publish may happen until all
# checks pass — a late failure must not leave a partial release.
# ------------------------------------------------------------------
declare -a pkg_names=()
for pkg in "$@"; do
  [ -f "$pkg/package.json" ] || { echo "missing $pkg/package.json" >&2; exit 1; }

  name="$(pkg_field "$pkg" name)"
  ver="$(pkg_field "$pkg" version)"
  [ -n "$name" ] && [ -n "$ver" ] || {
    echo "cannot read name/version from $pkg/package.json" >&2
    exit 1
  }
  pkg_names+=("$name")

  # Required distribution files: every package.json "files" entry must
  # exist (and directories must be non-empty) BEFORE publish — a
  # package missing bin/skill-graph or plugin/ would otherwise ship
  # broken and only fail at install time (#225 r5).
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if [ -d "$pkg/$f" ]; then
      [ -n "$(find "$pkg/$f" -mindepth 1 -print -quit)" ] || {
        echo "required directory is empty: $pkg/$f" >&2; exit 1; }
    else
      [ -e "$pkg/$f" ] || { echo "required file missing: $pkg/$f" >&2; exit 1; }
    fi
  done < <(pkg_files "$pkg")

  # bin entries must be regular files that are BOTH mode-executable and
  # executable-format — chmod +x on a text file or a corrupt binary
  # passes a permission check but fails every install (#225 r5).
  if [ -d "$pkg/bin" ]; then
    while IFS= read -r -d '' f; do
      [ -f "$f" ] || { echo "bin entry is not a regular file: $f" >&2; exit 1; }
      "$here/verify-npm-bin.sh" "$f" || exit 1
    done < <(find "$pkg/bin" -mindepth 1 -print0)
  fi
done

# Dependency order: when two packages in this invocation are related
# by optionalDependencies, the dependency must be published FIRST —
# a metapackage published before its platform packages would resolve
# to missing/broken optional deps for early adopters (#225 r5).
idx=0
for pkg in "$@"; do
  for dep in $(pkg_deps "$pkg"); do
    j=0
    for other in "$@"; do
      if [ "$j" -gt "$idx" ] && [ "${pkg_names[$j]}" = "$dep" ]; then
        echo "publish order violation: $pkg depends on $dep but $other comes later" >&2
        exit 1
      fi
      j=$((j + 1))
    done
  done
  idx=$((idx + 1))
done

# ------------------------------------------------------------------
# Phase 2: publish + verify each package (order = argument order,
# already validated above).
# ------------------------------------------------------------------
for pkg in "$@"; do
  name="$(pkg_field "$pkg" name)"
  ver="$(pkg_field "$pkg" version)"

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

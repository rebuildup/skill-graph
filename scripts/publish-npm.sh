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
#   E1. Reconcile the registry for EVERY package: an
#       already-published version must be proven identical to this
#       build's fixed artifact, and `npm view` failures are
#       classified — only an explicit not-found (E404) permits the
#       publish path; any other registry error aborts before publish.
#   E2. Publish each missing artifact in argument order and verify
#       it against the registry IMMEDIATELY, so a dependent package
#       is never published while its dependency's registry content
#       is unverified. The skip path compares the registry tarball
#       against the same fixed artifact.
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
# Usage: publish-npm.sh <package-dir|file.tgz> [<package-dir|file.tgz>...]
#
# Arguments may be package DIRECTORIES (packed here in Phase C) or
# pre-packed .tgz artifacts — the frozen npm channel artifacts the
# approved freeze manifest digests (#225 A06/A11). For a .tgz the
# archive itself is the fixed publish candidate: it is extracted
# only for manifest/files/bin preflight, never re-packed, so the
# published bytes are byte-identical to the verified frozen bytes.
# Auth: npm's normal resolution (NODE_AUTH_TOKEN / .npmrc).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
[ "$#" -gt 0 ] || { echo "usage: publish-npm.sh <package-dir|file.tgz>..." >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-publish.XXXXXX")" || exit 1
trap 'rm -rf "$work"' EXIT

# Normalize arguments: directories pass through; .tgz files are
# extracted into $work/src-N and the EXTRACTED tree stands in for the
# package dir in Phases A/B (manifest, files[], bin). The .tgz itself
# remains the publish candidate — Phase C skips re-packing it.
declare -a src_dirs=() src_tgzs=()
i=0
for arg in "$@"; do
  case "$arg" in
    *.tgz)
      [ -f "$arg" ] || { echo "tarball not found: $arg" >&2; exit 1; }
      # Archive wellformedness BEFORE extraction: canonical
      # package/-rooted members, no ".."/absolute paths, regular
      # files and dirs only — extraction of an unverified archive is
      # itself an attack surface.
      "$here/verify-npm-archive.sh" "$arg" || exit 1
      dest="$work/src-$i"
      mkdir -p "$dest" || exit 1
      tar -xzf "$arg" -C "$dest" || {
        echo "cannot extract $arg for preflight" >&2; exit 1; }
      [ -f "$dest/package/package.json" ] || {
        echo "$arg has no package/package.json" >&2; exit 1; }
      # Preflight/bin checks operate on package/ contents — point
      # them at the extracted package root.
      src_dirs+=("$dest/package")
      src_tgzs+=("$arg")
      ;;
    *)
      src_dirs+=("$arg")
      src_tgzs+=("")
      ;;
  esac
  i=$((i + 1))
done

# ------------------------------------------------------------------
# Phase A: structured manifest preflight for EVERY package. Real
# JSON parsing via node — sed/awk extraction mis-parses nested or
# unusually formatted manifests (#225 r6). No pack/publish may happen
# until all manifests validate.
# ------------------------------------------------------------------
meta="$(node "$here/pkg-preflight.mjs" "${src_dirs[@]}")" || exit 1

declare -a pkg_names=() pkg_vers=() pkg_files=() pkg_bins=() pkg_manifests=()
while IFS=$'\t' read -r n v f b x; do
  [ -n "$n" ] && [ -n "$v" ] && [ -n "$f" ] && [ -n "$b" ] && [ -n "$x" ] || {
    echo "pkg-preflight emitted malformed record" >&2; exit 1; }
  pkg_names+=("$n")
  pkg_vers+=("$v")
  pkg_files+=("$f")
  pkg_bins+=("$b")
  pkg_manifests+=("$x")
done <<< "$meta"
[ "${#pkg_names[@]}" -eq "$#" ] || {
  echo "pkg-preflight record count mismatch" >&2; exit 1; }

# ------------------------------------------------------------------
# Phase B: bin entries on disk — regular files that are BOTH
# mode-executable and executable-format. Cheap early rejection before
# packing; the authoritative check re-runs INSIDE each artifact in
# Phase D (#225 r5/r6).
# ------------------------------------------------------------------
for pkg in "${src_dirs[@]}"; do
  if [ -d "$pkg/bin" ]; then
    while IFS= read -r -d '' f; do
      [ -f "$f" ] || { echo "bin entry is not a regular file: $f" >&2; exit 1; }
      "$here/verify-npm-bin.sh" "$f" || exit 1
    done < <(find "$pkg/bin" -mindepth 1 -print0)
  fi
done

# Manifest-declared bin targets are REQUIRED payloads — preflight
# proved they exist under bin/ with the exec bit; the format/shebang
# check runs here on the exact on-disk bytes (#225 r8). Targets are
# constrained to bin/ by preflight, so --check-bin covers them inside
# the artifact too.
i=0
for pkg in "${src_dirs[@]}"; do
  while IFS= read -r t; do
    [ -n "$t" ] || continue
    "$here/verify-npm-bin.sh" "$pkg/$t" || exit 1
  done < <(BIN_JSON="${pkg_bins[$i]}" node -e '
    for (const t of JSON.parse(process.env.BIN_JSON)) console.log(t);
  ')
  i=$((i + 1))
done

# ------------------------------------------------------------------
# Phase C: produce the FIXED artifact for EVERY package BEFORE any
# publish. Directory arguments are packed here; .tgz arguments ARE
# the fixed artifact already (frozen channel bytes — never re-packed,
# #225 A06). Phase D verifies each artifact itself. Both complete
# for ALL packages before Phase E publishes anything — a late
# artifact failure must not leave a partial release (#225 r5).
# ------------------------------------------------------------------

declare -a tgzs=()
i=0
for pkg in "${src_dirs[@]}"; do
  frozen_tgz="${src_tgzs[$i]}"
  if [ -n "$frozen_tgz" ]; then
    # Pre-packed artifact: the bytes are already fixed. Resolve to
    # an absolute path — publish must not depend on cwd.
    tgz="$(cd "$(dirname "$frozen_tgz")" && pwd)/$(basename "$frozen_tgz")"
  else
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
  fi

  # Phase D: the FIXED artifact must itself be a well-formed npm
  # tarball with executable-format bin payloads — verified bytes are
  # the bytes that get published.
  "$here/verify-npm-archive.sh" --check-bin "$tgz" || exit 1

  # The artifact must BE the package preflight validated: the WHOLE
  # manifest deep-equal, and every declared files[]/bin payload
  # present as an archive member. npm could otherwise pack a
  # different directory or silently drop payloads (#225 r7/r9).
  FILES_JSON="${pkg_files[$i]}" BIN_JSON="${pkg_bins[$i]}" \
    EXPECTED_JSON="${pkg_manifests[$i]}" \
    WANT_NAME="${pkg_names[$i]}" WANT_VER="${pkg_vers[$i]}" TGZ="$tgz" node -e '
      const { execFileSync } = require("node:child_process");
      const tar = execFileSync("tar", ["-xzOf", process.env.TGZ, "package/package.json"], { encoding: "utf8" });
      const m = JSON.parse(tar);
      // The artifact manifest must BE the manifest preflight
      // validated — deep equality over the WHOLE object, not just
      // name/version: a pack that drifted bin/files/deps/anything
      // else would otherwise publish a package whose install
      // contract differs from what was verified (#225 r9).
      const sortKeys = (v) => Array.isArray(v) ? v.map(sortKeys)
        : (v && typeof v === "object"
            ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, sortKeys(v[k])]))
            : v);
      const expected = JSON.parse(process.env.EXPECTED_JSON);
      if (JSON.stringify(sortKeys(m)) !== JSON.stringify(sortKeys(expected))) {
        console.error(`artifact manifest differs from validated manifest for ${process.env.WANT_NAME}@${process.env.WANT_VER}`);
        process.exit(1);
      }
      const members = execFileSync("tar", ["-tzf", process.env.TGZ], { encoding: "utf8" }).split("\n");
      // Required member set = files[] payloads UNION manifest bin
      // targets. npm does not guarantee bin targets are packed via
      // files[] — a declared executable missing from the artifact is
      // a broken install that must fail HERE, before publish (#225 r8).
      const want = [...JSON.parse(process.env.FILES_JSON), ...JSON.parse(process.env.BIN_JSON)];
      for (const f of want) {
        const prefix = "package/" + f.replace(/\/+$/, "");
        const hit = members.some((mem) => mem === prefix || mem === prefix + "/" || mem.startsWith(prefix + "/"));
        if (!hit) {
          console.error(`required payload missing from artifact: ${f}`);
          process.exit(1);
        }
      }
    ' || exit 1

  # Platform packages (skill-graph-<os>-<arch>) must ship a NATIVE
  # binary for their declared target — a wrong-arch or script payload
  # passes --check-bin yet fails every install on the target platform
  # (#225 r8). verify-npm-arch.sh owns the target→file-output mapping.
  base="${pkg_names[$i]##*/}"   # strip npm scope if present
  case "$base" in
    skill-graph-linux-x64|skill-graph-linux-arm64|skill-graph-darwin-x64|skill-graph-darwin-arm64)
      target="${base#skill-graph-}"
      archdir="$work/arch-$i"
      mkdir -p "$archdir" || exit 1
      tar -xzf "$tgz" -C "$archdir" || exit 1
      while IFS= read -r -d '' b; do
        "$here/verify-npm-arch.sh" "$target" "$b" || exit 1
      done < <(find "$archdir/package/bin" -mindepth 1 -type f -print0 2>/dev/null)
      ;;
  esac
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
  # `npm view` fails for TWO different reasons that must not be
  # conflated (#225 r8): the version genuinely does not exist (E404 /
  # "No match found" — safe to publish) versus the registry being
  # unreachable or erroring (network/auth/5xx — publishing on top of
  # an unverified registry state is unsafe). Only an explicit
  # not-found signature permits the publish path.
  view_out="$(npm view "$name@$ver" version 2>&1)" && view_rc=0 || view_rc=$?
  if [ "$view_rc" -eq 0 ]; then
    echo "$name@$ver already published — verifying content identity"
    "$here/verify-npm-registry.sh" "$name" "$ver" "$tgz" || {
      echo "published $name@$ver differs from this build — refusing to publish anything" >&2
      exit 1
    }
  elif printf '%s' "$view_out" | grep -qiE 'E404|No match found|404 Not Found|is not in this registry'; then
    missing+=("$i")
  else
    echo "registry query failed for $name@$ver — cannot distinguish unpublished from unreachable; refusing to publish" >&2
    printf '%s\n' "$view_out" >&2
    exit 1
  fi
  i=$((i + 1))
done

# ------------------------------------------------------------------
# Phase E2: publish missing artifacts IN ARGUMENT ORDER (dependencies
# first, enforced in Phase A), and prove EACH published artifact
# against the registry IMMEDIATELY — a dependent package is never
# published while its dependency's registry content is still
# unverified (#225 r8). Skipped packages were already proven
# identical in E1, so every package is verified exactly once before
# the script exits.
# ------------------------------------------------------------------
for i in "${missing[@]}"; do
  name="${pkg_names[$i]}"
  ver="${pkg_vers[$i]}"
  npm publish "${tgzs[$i]}" --access public --provenance || {
    echo "npm publish failed for $name@$ver" >&2; exit 1; }
  "$here/verify-npm-registry.sh" "$name" "$ver" "${tgzs[$i]}" || {
    echo "registry does not serve this build's $name@$ver after publish" >&2
    exit 1
  }
done

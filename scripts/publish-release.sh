#!/usr/bin/env bash
# publish-release.sh — the GitHub Release publish/reconcile state
# machine for skill-graph (#225 F01/A09/A10).
#
# Extracted from release.yml so the publish/no-publish DECISIONS are
# executable and testable outside Actions: scripts/test-publish-release.sh
# drives every branch — fresh create, identical no-op, additive recovery,
# and every refuse path — against a stubbed `gh`.
#
# Contract:
#   * A release that does not exist is created with dist assets +
#     sha256sums.txt + freeze-manifest.json (--verify-tag).
#   * A release that exists is IMMUTABLE: published assets are never
#     clobbered. The published sha256sums.txt defines the expected set;
#     this build's manifest must equal it byte-for-byte (sorted), every
#     published tarball must match its manifest digest, missing manifest
#     entries are uploaded individually WITHOUT --clobber, and any
#     divergence (checksum diff, asset mismatch, unlisted asset,
#     manifest-record diff, tarballs-without-sums) is a hard refusal.
#   * freeze-manifest.json is the approval record produced by the
#     pinned freeze run — if already published it must be
#     byte-identical; if absent it is uploaded additively.
#
# Usage: publish-release.sh <tag> <dist-dir> [notes-file]
#   dist-dir must contain *.tar.gz, sha256sums.txt, freeze-manifest.json.
#   Env: GH_TOKEN (gh auth), GITHUB_REPOSITORY.
set -euo pipefail

tag="${1:?usage: publish-release.sh <tag> <dist-dir> [notes-file]}"
dist="${2:?usage: publish-release.sh <tag> <dist-dir> [notes-file]}"
notes="${3:-}"
# Resolve notes to an absolute path BEFORE cd'ing into dist — callers
# pass repo-relative paths.
[ -n "$notes" ] && notes="$(cd "$(dirname "$notes")" && pwd)/$(basename "$notes")"

[[ "$tag" =~ ^v[0-9A-Za-z._-]+$ ]] || { echo "invalid tag: $tag" >&2; exit 1; }
[ -d "$dist" ] || { echo "dist dir not found: $dist" >&2; exit 1; }
cd "$dist"
shopt -s nullglob
tarballs=(*.tar.gz)
[ "${#tarballs[@]}" -gt 0 ] || { echo "no tarballs in $dist" >&2; exit 1; }
[ -f sha256sums.txt ] || { echo "missing sha256sums.txt in $dist" >&2; exit 1; }
[ -f freeze-manifest.json ] || { echo "missing freeze-manifest.json in $dist" >&2; exit 1; }
shopt -u nullglob
# The manifest's artifact identity was already proven upstream by
# verify-freeze-manifest.py against the pinned freeze run; this script
# only requires the release inputs to be present and self-consistent.

if ! gh release view "$tag" >/dev/null 2>&1; then
  args=(release create "$tag" ./*.tar.gz sha256sums.txt freeze-manifest.json
        --title "skill-graph $tag" --verify-tag)
  [ -n "$notes" ] && [ -f "$notes" ] && args+=(--notes-file "$notes")
  gh "${args[@]}"
  exit 0
fi

# ---- Release exists: reconcile without ever clobbering -------------
published="$(gh release view "$tag" --json assets -q '.assets[].name' | sort)"

if ! grep -qx 'sha256sums.txt' <<< "$published"; then
  # No checksum manifest published. Completing the asset set is only
  # safe when NO tarball assets exist yet (fresh partial release); if
  # tarballs are already published we cannot verify them — refuse.
  if grep -q '^skill-graph-.*\.tar\.gz$' <<< "$published"; then
    echo "release $tag has assets but no sha256sums.txt;" >&2
    echo "cannot verify published bytes — refusing to overwrite." >&2
    exit 1
  fi
  # Additive recovery (#225 A10): upload only what is missing; a
  # pre-existing freeze-manifest.json must be byte-identical.
  gh release upload "$tag" ./*.tar.gz sha256sums.txt
  if grep -qx 'freeze-manifest.json' <<< "$published"; then
    work0="$(mktemp -d)"
    gh release download "$tag" --pattern 'freeze-manifest.json' \
      --dir "$work0" --clobber
    cmp "$work0/freeze-manifest.json" freeze-manifest.json || {
      echo "published freeze-manifest.json differs from the" >&2
      echo "approved manifest — refusing to continue." >&2
      exit 1; }
  else
    gh release upload "$tag" freeze-manifest.json
  fi
  exit 0
fi

work="$(mktemp -d)"
# The manifest must download; any failure aborts here instead of
# masquerading as "no manifest" (#221).
gh release download "$tag" --pattern 'sha256sums.txt' \
  --dir "$work" --clobber
published_sums="$work/sha256sums.txt"

# The published manifest defines the expected asset set — no hardcoded
# copy of the build matrix to drift (#221).
awk '{print $NF}' "$published_sums" | sort > "$work/expected.txt"
comm -23 "$work/expected.txt" \
  <(printf '%s\n' "$published" | sed '/^$/d' | sort) \
  > "$work/missing.txt" || true
missing="$(cat "$work/missing.txt")"

if ! diff <(sort "$published_sums") <(sort sha256sums.txt) > /dev/null; then
  echo "release $tag published checksums differ from this build." >&2
  echo "Published artifacts are immutable; publish a new tag instead." >&2
  diff <(sort "$published_sums") <(sort sha256sums.txt) || true
  exit 1
fi

# Manifest identical: this build reproduces the published bytes. Now
# verify the published ASSETS themselves against the manifest — a
# truncated, replaced, or unlisted asset must be detected, not
# overwritten (#221). Missing assets are uploaded below.
work_assets="$work/assets"
mkdir -p "$work_assets"
gh release download "$tag" --pattern '*.tar.gz' \
  --dir "$work_assets" --clobber
shopt -s nullglob
tarballs_p=("$work_assets"/*.tar.gz)
[ "${#tarballs_p[@]}" -gt 0 ] || {
  echo "no published tarballs found to verify"; exit 1; }
for f in "${tarballs_p[@]}"; do
  name="$(basename "$f")"
  want="$(awk -v n="$name" '$NF==n{print $1}' "$published_sums")"
  [ -n "$want" ] || {
    echo "published asset $name is not in sha256sums.txt" >&2
    echo "asset set does not match the manifest — refusing to continue." >&2
    exit 1; }
  got="$(sha256sum "$f" | awk '{print $1}')"
  [ "$want" = "$got" ] || {
    echo "checksum mismatch for published asset $name" >&2
    echo "published artifacts are immutable — refusing to overwrite." >&2
    exit 1; }
done
shopt -u nullglob

# freeze-manifest.json is the published APPROVAL RECORD — byte
# identity is required when present; absent => additive upload.
if grep -qx 'freeze-manifest.json' <<< "$published"; then
  gh release download "$tag" --pattern 'freeze-manifest.json' \
    --dir "$work" --clobber
  cmp "$work/freeze-manifest.json" freeze-manifest.json || {
    echo "published freeze-manifest.json differs from the" >&2
    echo "approved manifest — refusing to continue." >&2
    exit 1; }
else
  gh release upload "$tag" freeze-manifest.json
fi

if [ -z "$missing" ]; then
  echo "release $tag assets verified against published checksums; no-op"
else
  echo "release $tag missing assets:"; cat "$work/missing.txt"
  echo "uploading only the missing assets (no clobber)"
  # Upload each missing asset individually without --clobber: a
  # concurrent upload fails loudly instead of overwriting.
  while read -r asset; do
    [ -n "$asset" ] || continue
    gh release upload "$tag" "$asset"
  done < "$work/missing.txt"
fi

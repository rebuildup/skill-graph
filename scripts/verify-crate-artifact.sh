#!/usr/bin/env bash
# verify-crate-artifact.sh — PRE-PUBLISH crate identity check (#225
# A07/A08).
#
# crates.io accepts exactly one upload per version and never allows
# re-publishing the same version, so byte identity must be proven
# BEFORE `cargo publish` runs — a post-publish mismatch cannot be
# fixed by republishing. `cargo package` is byte-deterministic
# (normalized tar ordering/mtimes; verified empirically), so the
# .crate produced here is the same file `cargo publish` will upload
# from this clean checkout.
#
# Usage: verify-crate-artifact.sh <expected-sha256>
#   expected-sha256 is the crates-channel digest recorded in the
#   approved freeze-manifest.json for this release.
#
# Run from a CLEAN checkout of the tagged commit, immediately before
# `cargo publish`.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

want="${1:?usage: verify-crate-artifact.sh <expected-sha256>}"
[[ "$want" =~ ^[0-9a-f]{64}$ ]] || { echo "bad sha256: $want" >&2; exit 1; }

name="$(grep -m1 '^name' Cargo.toml | sed -E 's/^name = "([^"]+)".*/\1/')"
vers="$(grep -m1 '^version' Cargo.toml | sed -E 's/^version = "([^"]+)".*/\1/')"
[ -n "$name" ] && [ -n "$vers" ] || { echo "cannot resolve crate name/version" >&2; exit 1; }

# The packaged bytes must correspond to the committed tree: cargo
# package ships WORKING TREE content under the Cargo.toml `include`
# whitelist, so reject tracked modifications and untracked files
# under packaged paths — other untracked files cannot enter the
# .crate.
tracked_dirty="$(git status --porcelain | grep -v '^??' || true)"
untracked_pkg="$(git ls-files --others --exclude-standard -- src contracts Cargo.toml LICENSE README.md || true)"
if [ -n "$tracked_dirty" ] || [ -n "$untracked_pkg" ]; then
  echo "working tree differs from HEAD under packaged paths —" >&2
  echo "the .crate would not match the release commit." >&2
  printf '%s\n' "$tracked_dirty" "$untracked_pkg" >&2
  exit 1
fi

cargo package --locked >/dev/null
crate="target/package/${name}-${vers}.crate"
[ -f "$crate" ] || { echo "missing $crate" >&2; exit 1; }

got="$(sha256sum "$crate" 2>/dev/null | awk '{print $1}' \
  || shasum -a 256 "$crate" | awk '{print $1}')"
if [ "$got" != "$want" ]; then
  echo "CRATE DIGEST MISMATCH before publish" >&2
  echo "  local cargo package : $got" >&2
  echo "  approved manifest   : $want" >&2
  echo "the bytes cargo publish would upload differ from the" >&2
  echo "freeze-verified artifact — DO NOT PUBLISH." >&2
  exit 1
fi
echo "PASS: local crate == approved freeze digest ($got)"
echo "safe to run: cargo publish --locked"

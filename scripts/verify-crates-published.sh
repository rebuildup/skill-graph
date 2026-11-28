#!/usr/bin/env bash
# verify-crates-published.sh — post-publish byte-identity check for
# the crates.io channel (#225 A07/A08).
#
# crates.io versions are immutable and a version can never be
# re-published, so this check is a VERIFICATION, not a recovery
# mechanism: the approved freeze-manifest.json digest is the
# authority. Run scripts/verify-crate-artifact.sh BEFORE
# `cargo publish` to prove the bytes about to be uploaded equal the
# freeze-verified artifact; run THIS script AFTER publish to prove
# crates.io actually serves those same bytes.
#
# Usage: verify-crates-published.sh <expected-sha256> [version]
#   expected-sha256: crates-channel digest from the approved
#   freeze-manifest.json (REQUIRED — there is no re-pack fallback).
#   version: defaults to Cargo.toml's version.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

want="${1:?usage: verify-crates-published.sh <expected-sha256> [version]}"
vers="${2:-$(grep -m1 '^version' Cargo.toml | sed -E 's/^version = "([^"]+)".*/\1/')}"
name="$(grep -m1 '^name' Cargo.toml | sed -E 's/^name = "([^"]+)".*/\1/')"
[[ "$want" =~ ^[0-9a-f]{64}$ ]] || { echo "bad sha256: $want" >&2; exit 1; }
[ -n "$name" ] && [ -n "$vers" ] || { echo "cannot resolve crate name/version" >&2; exit 1; }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-crates-verify.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
remote="$tmp/${name}-${vers}.crate"

# static.crates.io serves the immutable uploaded bytes directly —
# no index propagation wait needed for the file itself.
if ! curl -fsSL --retry 5 --retry-delay 10 \
     -o "$remote" "https://static.crates.io/crates/${name}/${name}-${vers}.crate"; then
  echo "cannot download published crate ${name}-${vers}" >&2
  echo "(publish first, or the version does not exist on crates.io)" >&2
  exit 1
fi

got="$(sha256sum "$remote" 2>/dev/null | awk '{print $1}' \
  || shasum -a 256 "$remote" | awk '{print $1}')"
if [ "$want" != "$got" ]; then
  echo "CRATE BYTE MISMATCH for ${name}-${vers}" >&2
  echo "  published (crates.io) : $got" >&2
  echo "  approved manifest     : $want" >&2
  echo "published bytes differ from the freeze-verified artifact." >&2
  echo "crates.io versions are immutable: yank ${vers} and investigate" >&2
  echo "before any republish — do NOT assume a same-version retry works." >&2
  exit 1
fi
echo "PASS: crates.io ${name}-${vers} bytes == approved freeze digest ($want)"

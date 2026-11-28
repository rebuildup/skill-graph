#!/usr/bin/env bash
# verify-crates-published.sh — post-publish byte-identity check for
# the crates.io channel (#225 r11).
#
# `cargo publish` re-packs the .crate from the working tree at publish
# time, so the bytes crates.io stores are only guaranteed identical to
# the verified artifact if `cargo package` is deterministic — which it
# is (normalized tar: fixed ordering and mtimes; verified empirically,
# see contracts.md). This script closes the loop AFTER a manual
# `cargo publish`: it re-packs locally, downloads the .crate crates.io
# actually serves, and requires byte equality. crates.io versions are
# immutable, so a mismatch means the published bytes differ from what
# was verified — yank and republish, do not ignore.
#
# Usage: scripts/verify-crates-published.sh [version]
#   version defaults to Cargo.toml's version. Run AFTER
#   `cargo publish` has completed and the index has propagated
#   (static.crates.io serves the file as soon as the upload lands).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

name="$(grep -m1 '^name' Cargo.toml | sed -E 's/^name = "([^"]+)".*/\1/')"
vers="${1:-$(grep -m1 '^version' Cargo.toml | sed -E 's/^version = "([^"]+)".*/\1/')}"
[ -n "$name" ] && [ -n "$vers" ] || { echo "cannot resolve crate name/version" >&2; exit 1; }

# Re-pack locally: this is byte-identical to what `cargo publish`
# uploaded IFF the working tree equals the published commit — check
# that first so a dirty tree cannot masquerade as the release.
if [ -n "$(git status --porcelain -- Cargo.toml src/ Cargo.lock 2>/dev/null)" ]; then
  echo "warning: working tree differs from HEAD under packaged paths;" >&2
  echo "         the local .crate may not represent the published commit." >&2
fi

cargo package --allow-dirty --locked >/dev/null 2>&1 || {
  echo "cargo package failed" >&2; exit 1; }
local_crate="target/package/${name}-${vers}.crate"
[ -f "$local_crate" ] || { echo "missing $local_crate" >&2; exit 1; }
want="$(sha256sum "$local_crate" | awk '{print $1}')"

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

got="$(sha256sum "$remote" | awk '{print $1}')"
if [ "$want" != "$got" ]; then
  echo "CRATE BYTE MISMATCH for ${name}-${vers}" >&2
  echo "  local  (cargo package): $want" >&2
  echo "  remote (crates.io)    : $got" >&2
  echo "published bytes differ from the verified artifact." >&2
  echo "crates.io versions are immutable: yank ${vers} and investigate" >&2
  echo "before republishing." >&2
  exit 1
fi
echo "PASS: crates.io ${name}-${vers} bytes == cargo package artifact ($want)"

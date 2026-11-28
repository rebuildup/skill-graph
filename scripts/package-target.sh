#!/usr/bin/env bash
# package-target.sh — build ONE release tarball for a Rust target
# triple and PROVE the packaged artifact on the machine that built it:
# extract -> exec bit -> executable format -> native --version run ->
# arch match. This is the single artifact-construction path shared by
# release.yml (which publishes the tarball) and artifact-verify.yml
# (which runs it on every PR), so the bytes verified pre-merge are
# byte-identical to the bytes released — the archive is deterministic
# (normalized mtime, gzip -n) (#225 r9).
#
# Usage: package-target.sh <rust-target-triple> <npm-target-name>
# Runs from the repo root after `cargo build --release --target`.
# Output: ./skill-graph-<triple>.tar.gz in the cwd.
set -euo pipefail

triple="${1:?usage: package-target.sh <rust-target> <npm-target>}"
npm_target="${2:?usage: package-target.sh <rust-target> <npm-target>}"
here="$(cd "$(dirname "$0")" && pwd)"

# `cargo build --target <triple>` lands under target/<triple>/; a
# plain host `cargo build --release` lands under target/release/.
bin=""
for cand in "target/${triple}/release/skill-graph" "target/release/skill-graph"; do
  [ -f "$cand" ] && bin="$cand" && break
done
[ -n "$bin" ] || { echo "built binary not found for $triple" >&2; exit 1; }

strip "$bin" 2>/dev/null || true
mkdir -p pkg
cp "$bin" pkg/skill-graph
# Deterministic archive, portable across GNU tar (Linux) and bsdtar
# (macOS runners): normalize the single payload's mtime, archive it,
# and strip gzip's header timestamp. A rerun on the same
# commit/toolchain must reproduce byte-identical checksums —
# published assets are immutable and reruns verify instead of
# overwriting (#130).
touch -t 202001010000.00 pkg/skill-graph
COPYFILE_DISABLE=1 tar --no-xattrs -cf - -C pkg skill-graph \
  | gzip -n > "skill-graph-${triple}.tar.gz"
rm -rf pkg

# Verify the ARTIFACT, not the build tree: extract the exact tarball
# that will be published and prove it natively on this runner — the
# same OS/arch the binary targets (#225 r9).
verify_dir="$(mktemp -d "${TMPDIR:-/tmp}/sg-pkgverify.XXXXXX")"
trap 'rm -rf "$verify_dir"' EXIT
tar -xzf "skill-graph-${triple}.tar.gz" -C "$verify_dir"
# NO repair: the tarball must carry the exec bit itself. tar
# preserves modes, so a missing +x fails verify-npm-bin below —
# exactly the defect we are verifying against (#225 r10).
"$here/verify-npm-bin.sh" "$verify_dir/skill-graph"
"$verify_dir/skill-graph" --version
"$here/verify-npm-arch.sh" "$npm_target" "$verify_dir/skill-graph"

# Freeze record: the digest of the exact verified bytes travels with
# the artifact so a later promotion step can prove it is promoting
# THESE bytes, not a rebuild (#225 r10).
shasum -a 256 "skill-graph-${triple}.tar.gz" > "skill-graph-${triple}.tar.gz.sha256" 2>/dev/null \
  || sha256sum "skill-graph-${triple}.tar.gz" > "skill-graph-${triple}.tar.gz.sha256"

echo "packaged+verified: skill-graph-${triple}.tar.gz"

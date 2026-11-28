#!/usr/bin/env bash
# test-publish-release.sh — integration test for the release
# publish/reconcile state machine (#225 F01).
#
# Drives scripts/publish-release.sh through EVERY decision branch with
# a stubbed `gh` backed by a filesystem "release store": fresh create,
# identical no-op, additive recovery (missing manifest / missing
# assets), and every refuse path (checksum diff, asset mismatch,
# tarballs-without-sums, unlisted asset, divergent approval record).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
script="$root/scripts/publish-release.sh"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-pubrel.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

bin="$tmp/bin"; store="$tmp/store"
mkdir -p "$bin" "$store"

# ---- gh stub: emulates `gh release view/download/upload/create` ----
cat > "$bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
store="${GH_STUB_STORE:?}"
cmd="${1:?}"; shift
[ "$cmd" = "release" ] || { echo "stub gh: unsupported $cmd" >&2; exit 2; }
sub="${1:?}"; shift
case "$sub" in
  view)
    tag="${1:?}"; shift || true
    [ -d "$store/$tag" ] || exit 1
    if [ "${1:-}" = "--json" ]; then
      # --json assets -q '.assets[].name'
      find "$store/$tag" -maxdepth 1 -type f -printf '%f\n' | sort
      exit 0
    fi
    exit 0
    ;;
  download)
    tag="${1:?}"; shift
    pattern=""; dir=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --pattern) pattern="$2"; shift 2 ;;
        --dir) dir="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    [ -d "$store/$tag" ] || { echo "stub gh: no release $tag" >&2; exit 1; }
    mkdir -p "$dir"
    for f in "$store/$tag"/*; do
      [ -f "$f" ] || continue
      b="$(basename "$f")"
      case "$pattern" in
        '*.tar.gz') [[ "$b" == *.tar.gz ]] || continue ;;
        *) [ "$b" = "$pattern" ] || continue ;;
      esac
      cp "$f" "$dir/$b"
    done
    ;;
  upload)
    tag="${1:?}"; shift
    [ -d "$store/$tag" ] || { echo "stub gh: no release $tag" >&2; exit 1; }
    for f in "$@"; do
      case "$f" in --*) continue ;; esac
      [ -f "$f" ] || continue
      b="$(basename "$f")"
      if [ -e "$store/$tag/$b" ]; then
        echo "stub gh: asset $b already exists (no --clobber)" >&2
        exit 1
      fi
      cp "$f" "$store/$tag/$b"
    done
    ;;
  create)
    tag="${1:?}"; shift
    mkdir -p "$store/$tag"
    for f in "$@"; do
      case "$f" in --*) continue ;; esac
      [ -f "$f" ] || continue
      cp "$f" "$store/$tag/$(basename "$f")"
    done
    ;;
  *) echo "stub gh: unsupported release $sub" >&2; exit 2 ;;
esac
STUB
chmod +x "$bin/gh"
export PATH="$bin:$PATH"
export GH_STUB_STORE="$store"
export GH_TOKEN=dummy GITHUB_REPOSITORY=rebuildup/skill-graph

fails=0
ok()  { echo "ok: $1"; }
bad() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

# ---- fixture dist -------------------------------------------------
mkdist() { # $1 = dir
  mkdir -p "$1"
  printf 'binary-a' > "$1/skill-graph-x86_64-unknown-linux-gnu.tar.gz"
  printf 'binary-b' > "$1/skill-graph-aarch64-apple-darwin.tar.gz"
  (cd "$1" && sha256sum *.tar.gz > sha256sums.txt)
  printf '{"schema":"skill-graph/freeze-manifest/1","run_id":1}\n' > "$1/freeze-manifest.json"
}

TAG=v9.9.9

# 1. Fresh release -> create with the full asset set.
d1="$tmp/d1"; mkdist "$d1"
"$script" "$TAG" "$d1" >"$tmp/out1" 2>&1 || bad "fresh create exited nonzero: $(cat "$tmp/out1")"
[ -f "$store/$TAG/sha256sums.txt" ] && [ -f "$store/$TAG/freeze-manifest.json" ] \
  && [ "$(ls "$store/$TAG" | grep -c 'tar.gz$')" = "2" ] \
  && ok "fresh release created with all assets" \
  || bad "fresh release asset set wrong: $(ls "$store/$TAG" 2>/dev/null)"

# 2. Identical re-run -> verified no-op, nothing uploaded/clobbered.
"$script" "$TAG" "$d1" >"$tmp/out2" 2>&1 || bad "identical re-run failed: $(cat "$tmp/out2")"
grep -q "no-op" "$tmp/out2" && ok "identical re-run is a verified no-op" \
  || bad "identical re-run did not report no-op: $(cat "$tmp/out2")"

# 3. Corrupt a published tarball -> checksum mismatch refusal.
printf 'tampered' > "$store/$TAG/skill-graph-x86_64-unknown-linux-gnu.tar.gz"
if "$script" "$TAG" "$d1" >"$tmp/out3" 2>&1; then
  bad "corrupt published asset was accepted"
else
  grep -q "checksum mismatch" "$tmp/out3" && ok "corrupt published asset refused" \
    || bad "wrong refusal reason: $(cat "$tmp/out3")"
fi
# restore
printf 'binary-a' > "$store/$TAG/skill-graph-x86_64-unknown-linux-gnu.tar.gz"

# 4. Local manifest differs from published checksums -> refuse.
d4="$tmp/d4"; mkdist "$d4"
printf 'binary-c' > "$d4/skill-graph-aarch64-apple-darwin.tar.gz"
(cd "$d4" && sha256sum *.tar.gz > sha256sums.txt)
if "$script" "$TAG" "$d4" >"$tmp/out4" 2>&1; then
  bad "divergent local manifest was accepted"
else
  grep -q "published checksums differ" "$tmp/out4" && ok "divergent manifest refused" \
    || bad "wrong refusal reason: $(cat "$tmp/out4")"
fi

# 5. Release with tarballs but NO sha256sums.txt -> refuse (cannot verify).
TAG2=v9.9.10
mkdir -p "$store/$TAG2"
printf 'x' > "$store/$TAG2/skill-graph-x86_64-unknown-linux-gnu.tar.gz"
d5="$tmp/d5"; mkdist "$d5"
if "$script" "$TAG2" "$d5" >"$tmp/out5" 2>&1; then
  bad "tarballs-without-sums release was accepted"
else
  grep -q "no sha256sums.txt" "$tmp/out5" && ok "tarballs-without-sums refused" \
    || bad "wrong refusal reason: $(cat "$tmp/out5")"
fi

# 6. Empty partial release (no assets) -> additive recovery uploads all.
TAG3=v9.9.11
mkdir -p "$store/$TAG3"
d6="$tmp/d6"; mkdist "$d6"
"$script" "$TAG3" "$d6" >"$tmp/out6" 2>&1 || bad "partial recovery failed: $(cat "$tmp/out6")"
[ "$(ls "$store/$TAG3" | wc -l)" = "4" ] && ok "empty partial release recovered additively" \
  || bad "partial recovery asset count wrong: $(ls "$store/$TAG3")"

# 7. Missing freeze-manifest.json on published release -> additive upload only.
rm "$store/$TAG3/freeze-manifest.json"
"$script" "$TAG3" "$d6" >"$tmp/out7" 2>&1 || bad "manifest recovery failed: $(cat "$tmp/out7")"
[ -f "$store/$TAG3/freeze-manifest.json" ] && ok "missing approval record uploaded additively" \
  || bad "freeze-manifest.json not recovered"

# 8. Divergent published freeze-manifest.json -> refuse.
printf '{"different":true}\n' > "$store/$TAG3/freeze-manifest.json"
if "$script" "$TAG3" "$d6" >"$tmp/out8" 2>&1; then
  bad "divergent approval record was accepted"
else
  grep -q "differs from the" "$tmp/out8" && ok "divergent approval record refused" \
    || bad "wrong refusal reason: $(cat "$tmp/out8")"
fi
printf '{"schema":"skill-graph/freeze-manifest/1","run_id":1}\n' > "$store/$TAG3/freeze-manifest.json"

# 8b. Checksums-only partial publish (sums present, ZERO tarballs)
#     -> additive recovery uploads every manifest asset (#225 A10).
TAG4=v9.9.12
mkdir -p "$store/$TAG4"
cp "$d6/sha256sums.txt" "$store/$TAG4/sha256sums.txt"
d8b="$tmp/d8b"; mkdist "$d8b"
"$script" "$TAG4" "$d8b" >"$tmp/out8b" 2>&1 \
  || bad "checksums-only recovery failed: $(cat "$tmp/out8b")"
[ "$(ls "$store/$TAG4" | grep -c 'tar.gz$')" = "2" ] \
  && [ -f "$store/$TAG4/freeze-manifest.json" ] \
  && ok "checksums-only partial release recovered additively" \
  || bad "checksums-only recovery wrong: $(ls "$store/$TAG4")"

# 8c. No-sums release with a DIVERGENT published manifest -> refuse
#     AND no side effects: tarballs/checksums must NOT have been
#     uploaded before the refusal (#225 A09 read-all-then-mutate).
TAG5=v9.9.13
mkdir -p "$store/$TAG5"
printf '{"different":true}\n' > "$store/$TAG5/freeze-manifest.json"
d8c="$tmp/d8c"; mkdist "$d8c"
if "$script" "$TAG5" "$d8c" >"$tmp/out8c" 2>&1; then
  bad "no-sums divergent manifest was accepted"
else
  grep -q "differs from the" "$tmp/out8c" && ok "no-sums divergent manifest refused" \
    || bad "wrong refusal reason: $(cat "$tmp/out8c")"
fi
[ "$(ls "$store/$TAG5" | wc -l)" = "1" ] \
  && ok "refusal left zero side effects (no tarballs/checksums uploaded)" \
  || bad "refusal had side effects: $(ls "$store/$TAG5")"

# 9. Published asset missing from release (in sums, not uploaded) ->
#    additive upload of exactly the missing asset.
rm "$store/$TAG3/skill-graph-aarch64-apple-darwin.tar.gz"
"$script" "$TAG3" "$d6" >"$tmp/out9" 2>&1 || bad "missing-asset recovery failed: $(cat "$tmp/out9")"
[ -f "$store/$TAG3/skill-graph-aarch64-apple-darwin.tar.gz" ] \
  && ok "missing published asset uploaded without clobber" \
  || bad "missing asset not recovered"

# 10. Extra published asset not in sha256sums -> refuse.
printf 'rogue' > "$store/$TAG3/skill-graph-rogue.tar.gz"
# recompute nothing: rogue is not in published sums -> the asset-set
# check compares expected(manifest) vs published; extra asset shows up
# in `published` but not expected -> comm -23 expected-published is
# empty (missing empty) — wait: extra asset makes published superset;
# the per-tarball loop checks each PUBLISHED tarball is in sums ->
# rogue.tar.gz is not -> refuse.
if "$script" "$TAG3" "$d6" >"$tmp/out10" 2>&1; then
  bad "unlisted published asset was accepted"
else
  grep -q "not in sha256sums.txt" "$tmp/out10" && ok "unlisted published asset refused" \
    || bad "wrong refusal reason: $(cat "$tmp/out10")"
fi
rm "$store/$TAG3/skill-graph-rogue.tar.gz"

[ "$fails" -eq 0 ] && echo "publish-release tests: all cases pass" \
  || { echo "publish-release tests: $fails FAILURES" >&2; exit 1; }

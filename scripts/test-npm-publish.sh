#!/usr/bin/env bash
# test-npm-publish.sh — end-to-end tests for scripts/publish-npm.sh
# using a stub `npm` that simulates a registry on the filesystem.
# Proves the publish decision path REJECTS what it must: divergent
# already-published content, post-publish registry mismatch, missing
# executables, failed publishes — and ACCEPTS identical skip/publish
# flows (#225 re-review). Runs without NPM_TOKEN or network: the stub
# serves file:// tarballs to the real verify-npm-registry.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
PUB="$(pwd)/scripts/publish-npm.sh"

fails=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; fails=$((fails + 1)); }
ok() { say "ok: $*"; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-publish-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# --- stub npm ----------------------------------------------------------
# Registry state: $FAKE_NPM_ROOT/{published/<key>,tarballs/<key>.tgz}
# key = name@version with / and @ flattened. `npm publish` packs the
# dir exactly like npm (single package/ root) unless
# FAKE_NPM_TAMPER=1, which ships a tampered tarball to exercise the
# post-publish identity check.
mkdir -p "$work/bin" "$work/binfail" "$work/registry"
cat > "$work/bin/npm" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
root="${FAKE_NPM_ROOT:?}"
field() { sed -n "s/^[[:space:]]*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$1/package.json" | head -1; }
key_of() { printf '%s' "$1" | tr '/@' '__'; }

pack_dir() { # $1=pkgdir $2=out.tgz
  local tmp
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-npmpack.XXXXXX")"
  mkdir -p "$tmp/package"
  cp -r "$1"/. "$tmp/package/"
  tar -czf "$2" -C "$tmp" package
  rm -rf "$tmp"
}

case "${1:-}" in
  view)
    spec="$2"; want="$3"
    # spec = name@version; name may itself start with @scope/
    ver="${spec##*@}"
    name="${spec%@$ver}"
    key="$(key_of "$name@$ver")"
    if [ "$want" = "version" ]; then
      [ -f "$root/published/$key" ] && { echo "$ver"; exit 0; }
      exit 1
    fi
    if [ "$want" = "dist.tarball" ]; then
      [ -f "$root/tarballs/$key.tgz" ] && { echo "file://$root/tarballs/$key.tgz"; exit 0; }
      exit 1
    fi
    exit 1
    ;;
  publish)
    dir="$2"
    name="$(field "$dir" name)"; ver="$(field "$dir" version)"
    key="$(key_of "$name@$ver")"
    mkdir -p "$root/published" "$root/tarballs"
    if [ "${FAKE_NPM_TAMPER:-0}" = "1" ]; then
      # Ship content that differs from the local package: the
      # post-publish identity check MUST catch this.
      tamper_tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-tamper.XXXXXX")"
      mkdir -p "$tamper_tmp/package"
      cp -r "$dir"/. "$tamper_tmp/package/"
      printf 'tampered\n' > "$tamper_tmp/package/tampered.txt"
      tar -czf "$root/tarballs/$key.tgz" -C "$tamper_tmp" package
      rm -rf "$tamper_tmp"
    else
      pack_dir "$dir" "$root/tarballs/$key.tgz"
    fi
    echo "$ver" > "$root/published/$key"
    echo "PUBLISH $name@$ver" >> "$root/publish.log"
    ;;
  pack)
    dir="$2"; dest=""
    while [ $# -gt 0 ]; do [ "$1" = "--pack-destination" ] && { dest="$2"; break; }; shift; done
    [ -n "$dest" ] || { echo "stub npm pack: no --pack-destination" >&2; exit 2; }
    name="$(field "$dir" name)"; ver="$(field "$dir" version)"
    out="$dest/$(key_of "$name")-$ver.tgz"
    pack_dir "$dir" "$out"
    echo "$out"
    ;;
  *) echo "stub npm: unhandled args: $*" >&2; exit 2 ;;
esac
SH
chmod +x "$work/bin/npm"
# Variant stub whose publish always fails.
sed 's|^  publish)|  publish) exit 1 ;;|' "$work/bin/npm" > "$work/binfail/npm"
chmod +x "$work/binfail/npm"
export FAKE_NPM_ROOT="$work/registry"

# --- fixture packages (relative dirs; publish-npm.sh runs from $work) -
mkpkg() { # $1=dir $2=name $3=payload
  mkdir -p "$1/bin"
  printf '{\n  "name": "%s",\n  "version": "1.0.0",\n  "files": ["bin/tool","payload.txt"]\n}\n' "$2" > "$1/package.json"
  printf '#!/bin/sh\necho tool\n' > "$1/bin/tool"
  chmod +x "$1/bin/tool"
  printf '%s\n' "$3" > "$1/payload.txt"
}
(cd "$work" && mkpkg pkgA "@test/pkg-a" "payload-A-v1" && mkpkg pkgB "@test/pkg-b" "payload-B-v1")

cd "$work"
export PATH="$work/bin:$PATH"
reset_registry() {
  rm -rf "$FAKE_NPM_ROOT/published" "$FAKE_NPM_ROOT/tarballs" "$FAKE_NPM_ROOT/publish.log"
  mkdir -p "$FAKE_NPM_ROOT/published" "$FAKE_NPM_ROOT/tarballs"
}

# --- cases ---------------------------------------------------------------

echo "== fresh publish: both packages publish then verify =="
reset_registry
if PATH="$work/bin:$PATH" bash "$PUB" pkgA pkgB >"$work/out1" 2>&1; then
  ok "fresh publish accepted"
else
  fail "fresh publish rejected: $(cat "$work/out1")"
fi
[ "$(wc -l < "$FAKE_NPM_ROOT/publish.log" 2>/dev/null || echo 0)" -eq 2 ] \
  && ok "both packages published" \
  || fail "publish log wrong: $(cat "$FAKE_NPM_ROOT/publish.log" 2>/dev/null)"
grep -q 'PUBLISH @test/pkg-a@1.0.0' "$FAKE_NPM_ROOT/publish.log" \
  && grep -q 'PUBLISH @test/pkg-b@1.0.0' "$FAKE_NPM_ROOT/publish.log" \
  && ok "publish order preserved" || fail "publish order wrong"

echo "== rerun: identical published content skips without republish =="
: > "$FAKE_NPM_ROOT/publish.log"
if PATH="$work/bin:$PATH" bash "$PUB" pkgA pkgB >"$work/out2" 2>&1; then
  ok "identical rerun accepted"
else
  fail "identical rerun rejected: $(cat "$work/out2")"
fi
[ ! -s "$FAKE_NPM_ROOT/publish.log" ] \
  && ok "no republish on identical rerun" \
  || fail "republished identical content: $(cat "$FAKE_NPM_ROOT/publish.log")"

echo "== rerun: divergent local content refuses to skip =="
printf 'payload-A-v2\n' > pkgA/payload.txt
if PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out3" 2>&1; then
  fail "divergent published content was accepted"
else
  ok "divergent published content rejected"
  grep -q "refusing to skip" "$work/out3" \
    && ok "rejection names the skip path" || fail "rejection message unclear"
fi
printf 'payload-A-v1\n' > pkgA/payload.txt

echo "== post-publish: registry serving different content is rejected =="
reset_registry
if FAKE_NPM_TAMPER=1 PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out4" 2>&1; then
  fail "tampered post-publish content was accepted"
else
  ok "tampered post-publish content rejected"
fi

echo "== published marker without tarball is rejected =="
reset_registry
key="$(printf '%s' '@test/pkg-a@1.0.0' | tr '/@' '__')"
echo "1.0.0" > "$FAKE_NPM_ROOT/published/$key"
if PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out5" 2>&1; then
  fail "missing published tarball was accepted"
else
  ok "missing published tarball rejected"
fi

echo "== non-executable bin entry rejected before publish =="
reset_registry
chmod -x pkgA/bin/tool
if PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out6" 2>&1; then
  fail "non-executable bin was published"
else
  ok "non-executable bin rejected"
  grep -q "not executable" "$work/out6" && ok "rejection names the bin" || fail "unclear rejection"
fi
chmod +x pkgA/bin/tool
[ ! -e "$FAKE_NPM_ROOT/publish.log" ] \
  && ok "rejected package never reached npm publish" \
  || fail "publish attempted despite bin failure"

echo "== missing package.json rejected =="
mkdir -p empty
if PATH="$work/bin:$PATH" bash "$PUB" empty >"$work/out7" 2>&1; then
  fail "missing package.json accepted"
else
  ok "missing package.json rejected"
fi

echo "== npm publish failure propagates =="
reset_registry
if PATH="$work/binfail:$work/bin:$PATH" bash "$PUB" pkgA >"$work/out8" 2>&1; then
  fail "failed npm publish was accepted"
else
  ok "failed npm publish propagates non-zero"
fi

if [ "$fails" -gt 0 ]; then
  say "$fails publish-path test(s) FAILED"
  exit 1
fi
say "publish-path tests: all cases pass"

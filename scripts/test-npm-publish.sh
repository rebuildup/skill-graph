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
PUB="${PUB:-$(pwd)/scripts/publish-npm.sh}"

fails=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; fails=$((fails + 1)); }
ok() { say "ok: $*"; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-publish-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# --- stub npm ----------------------------------------------------------
# Registry state: $FAKE_NPM_ROOT/{published/<key>,tarballs/<key>.tgz}
# key = name@version with / and @ flattened. `npm publish`/`npm view`
# are stubbed (no NPM_TOKEN, no network), but `npm pack` DELEGATES TO
# REAL NPM: the artifact under test must be what npm actually produces,
# not a stub fabrication — the r7 review found the previous stub masked
# a real defect (bare "npm/foo" pack args resolve as GitHub repos).
mkdir -p "$work/bin" "$work/binfail" "$work/registry"
REAL_NPM="$(command -v npm)"
[ -n "$REAL_NPM" ] || { echo "real npm not found — required for pack fidelity" >&2; exit 2; }
export REAL_NPM
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
  if [ "${FAKE_NPM_BADPACK:-0}" = "1" ]; then
    # Rename one member to a non-canonical form: "package//evil"
    # normalizes to package/evil at extraction. The archive gate MUST
    # reject the artifact before it can be published (#225 r6).
    printf 'evil\n' > "$tmp/package/evil"
    tar --transform 's|^package/evil$|package//evil|' -czf "$2" -C "$tmp" package
  else
    tar -czf "$2" -C "$tmp" package
  fi
  rm -rf "$tmp"
}

# Extract name/version from a packed artifact's manifest — the publish
# path now ships the fixed .tgz itself, so the stub reads
# package/package.json out of the tarball.
artifact_field() { # $1=tgz $2=field — node is guaranteed wherever npm runs
  tar -xzOf "$1" package/package.json 2>/dev/null | FIELD="$2" node -e '
    let s = "";
    process.stdin.on("data", (d) => (s += d)).on("end", () => {
      try {
        const v = JSON.parse(s)[process.env.FIELD];
        if (typeof v === "string") console.log(v);
      } catch {}
    });
  '
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
    artifact="$2"
    case "$artifact" in
      *.tgz) name="$(artifact_field "$artifact" name)"; ver="$(artifact_field "$artifact" version)" ;;
      *) name="$(field "$artifact" name)"; ver="$(field "$artifact" version)" ;;
    esac
    [ -n "$name" ] && [ -n "$ver" ] || { echo "stub npm publish: cannot read manifest" >&2; exit 2; }
    key="$(key_of "$name@$ver")"
    mkdir -p "$root/published" "$root/tarballs"
    if [ "${FAKE_NPM_TAMPER:-0}" = "1" ]; then
      # Ship content that differs from the fixed artifact: the
      # post-publish identity check MUST catch this.
      tamper_tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-tamper.XXXXXX")"
      tar -xzf "$artifact" -C "$tamper_tmp" 2>/dev/null || mkdir -p "$tamper_tmp/package"
      printf 'tampered\n' > "$tamper_tmp/package/tampered.txt"
      tar -czf "$root/tarballs/$key.tgz" -C "$tamper_tmp" package
      rm -rf "$tamper_tmp"
    elif [ "${FAKE_NPM_BADPACK:-0}" = "1" ] && [[ "$artifact" == *.tgz ]]; then
      # Repack the artifact dir with a normalization-colliding member
      # — simulating a corrupted publish payload.
      rep_tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-repack.XXXXXX")"
      tar -xzf "$artifact" -C "$rep_tmp"
      printf 'evil\n' > "$rep_tmp/package/evil"
      tar --transform 's|^package/evil$|package//evil|' -czf "$root/tarballs/$key.tgz" -C "$rep_tmp" package
      rm -rf "$rep_tmp"
    elif [[ "$artifact" == *.tgz ]]; then
      # The fixed artifact ships byte-identical — what Phase D
      # verified is what the registry serves.
      cp "$artifact" "$root/tarballs/$key.tgz"
    else
      pack_dir "$artifact" "$root/tarballs/$key.tgz"
    fi
    echo "$ver" > "$root/published/$key"
    echo "PUBLISH $name@$ver" >> "$root/publish.log"
    ;;
  pack)
    # FAKE_NPM_BADPACK fabricates a normalization-colliding member that
    # real npm cannot be made to emit; FAKE_NPM_DROP_MEMBER repacks a
    # REAL npm artifact minus one files[] payload to exercise the
    # artifact member-set check. Everything else uses REAL npm pack.
    if [ "${FAKE_NPM_BADPACK:-0}" = "1" ]; then
      dir="$2"; dest=""
      while [ $# -gt 0 ]; do [ "$1" = "--pack-destination" ] && { dest="$2"; break; }; shift; done
      [ -n "$dest" ] || { echo "stub npm pack: no --pack-destination" >&2; exit 2; }
      name="$(field "$dir" name)"; ver="$(field "$dir" version)"
      out="$dest/$(key_of "$name")-$ver.tgz"
      FAKE_NPM_BADPACK=0 pack_dir "$dir" "$out"
      # inject the colliding member into the fabricated tarball
      tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-badpack.XXXXXX")"
      tar -xzf "$out" -C "$tmp"
      printf 'evil\n' > "$tmp/package/evil"
      tar --transform 's|^package/evil$|package//evil|' -czf "$out" -C "$tmp" package
      rm -rf "$tmp"
      echo "$out"
      exit 0
    fi
    if [ "${FAKE_NPM_DROP_MEMBER:-0}" = "1" ]; then
      # Real pack, then strip one payload member and repack — simulates
      # an artifact missing a declared files[] entry.
      dest=""
      prev=""
      for a in "$@"; do [ "$prev" = "--pack-destination" ] && dest="$a"; prev="$a"; done
      [ -n "$dest" ] || { echo "stub npm pack: no --pack-destination" >&2; exit 2; }
      "$REAL_NPM" "$@" >/dev/null
      tgz="$(find "$dest" -name '*.tgz' -print -quit)"
      tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-drop.XXXXXX")"
      tar -xzf "$tgz" -C "$tmp"
      rm -f "$tmp/package/payload.txt"
      tar -czf "$tgz" -C "$tmp" package
      rm -rf "$tmp"
      echo "$tgz"
      exit 0
    fi
    exec "$REAL_NPM" "$@"
    ;;
  *) echo "stub npm: unhandled args: $*" >&2; exit 2 ;;
esac
SH
chmod +x "$work/bin/npm"
# Variant stub whose `npm publish` always fails while pack/view still
# work — isolates Phase E2 failure propagation (the previous sed
# variant broke the case grammar and failed at pack instead).
cat > "$work/binfail/npm" <<SH
#!/usr/bin/env bash
if [ "\${1:-}" = "publish" ]; then
  echo "stub npm publish: forced failure" >&2
  exit 1
fi
exec "$work/bin/npm" "\$@"
SH
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
  grep -q "refusing to publish anything" "$work/out3" \
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

# --- round-5 guarantees: two-phase preflight, required files, bin
# format, dependency order (#225 r5) ---------------------------------

echo "== preflight failure on LAST package means ZERO publishes =="
reset_registry
# pkgBad declares files[] entry that does not exist on disk.
mkdir -p pkgBad/bin
cat > pkgBad/package.json <<'JSON'
{
  "name": "@test/pkg-bad",
  "version": "1.0.0",
  "files": ["bin/tool", "missing-payload.txt"]
}
JSON
printf '#!/bin/sh\necho t\n' > pkgBad/bin/tool
chmod +x pkgBad/bin/tool
if PATH="$work/bin:$PATH" bash "$PUB" pkgA pkgBad >"$work/out9" 2>&1; then
  fail "package with missing required file was published"
else
  ok "missing required file rejected"
  grep -q "required file missing" "$work/out9" \
    && ok "rejection names the missing file" || fail "unclear rejection"
fi
[ ! -e "$FAKE_NPM_ROOT/publish.log" ] \
  && ok "NO package published when a later package fails preflight" \
  || fail "partial publish occurred: $(cat "$FAKE_NPM_ROOT/publish.log")"

echo "== +x but non-executable-format bin rejected =="
reset_registry
printf 'just some text, no shebang\n' > pkgA/bin/tool
chmod +x pkgA/bin/tool
if PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out10" 2>&1; then
  fail "text-file bin with exec bit was published"
else
  ok "non-executable-format bin rejected"
fi
printf '#!/bin/sh\necho tool\n' > pkgA/bin/tool
chmod +x pkgA/bin/tool

echo "== symlink bin entry rejected =="
reset_registry
rm pkgA/bin/tool
ln -s /bin/sh pkgA/bin/tool
if PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out11" 2>&1; then
  fail "symlink bin was published"
else
  ok "symlink bin rejected"
fi
rm -f pkgA/bin/tool
printf '#!/bin/sh\necho tool\n' > pkgA/bin/tool
chmod +x pkgA/bin/tool

echo "== subdirectory inside bin/ rejected =="
reset_registry
mkdir -p pkgA/bin/nested
if PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out12" 2>&1; then
  fail "bin subdirectory was published"
else
  ok "bin subdirectory rejected"
fi
rmdir pkgA/bin/nested

echo "== dependency order: dependent before dependency rejected =="
reset_registry
# pkgMeta depends on @test/pkg-a via optionalDependencies.
mkdir -p pkgMeta/bin
cat > pkgMeta/package.json <<'JSON'
{
  "name": "@test/pkg-meta",
  "version": "1.0.0",
  "files": ["bin/tool"],
  "optionalDependencies": {
    "@test/pkg-a": "1.0.0"
  }
}
JSON
printf '#!/bin/sh\necho m\n' > pkgMeta/bin/tool
chmod +x pkgMeta/bin/tool
if PATH="$work/bin:$PATH" bash "$PUB" pkgMeta pkgA >"$work/out13" 2>&1; then
  fail "dependent-before-dependency order was published"
else
  ok "publish order violation rejected"
  grep -q "order violation" "$work/out13" && ok "rejection names the violation" || fail "unclear rejection"
fi
[ ! -e "$FAKE_NPM_ROOT/publish.log" ] \
  && ok "order violation prevented ALL publishes" \
  || fail "partial publish on order violation"

echo "== non-canonical packed artifact rejected before ANY publish =="
reset_registry
# Stub pack emits a tarball with member "package//evil" — a name that
# normalizes to package/evil at extraction. Phase D must reject the
# fixed artifact, so nothing may reach the registry (#225 r6).
if FAKE_NPM_BADPACK=1 PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out15" 2>&1; then
  fail "non-canonical packed artifact was published"
else
  ok "non-canonical packed artifact rejected"
  grep -q "non-canonical" "$work/out15" \
    && ok "rejection names the archive gate" || fail "unclear rejection: $(cat "$work/out15")"
fi
[ ! -e "$FAKE_NPM_ROOT/publish.log" ] \
  && ok "artifact failure prevented ALL publishes" \
  || fail "partial publish on artifact failure"

echo "== pkg-preflight: malformed JSON rejected =="
reset_registry
mkdir -p pkgBadJson/bin
printf '{ "name": "@test/bad-json", "version": }\n' > pkgBadJson/package.json
printf '#!/bin/sh\necho t\n' > pkgBadJson/bin/tool
chmod +x pkgBadJson/bin/tool
if PATH="$work/bin:$PATH" bash "$PUB" pkgBadJson >"$work/out16" 2>&1; then
  fail "malformed package.json accepted"
else
  ok "malformed package.json rejected"
fi

echo "== pkg-preflight: files[] traversal entry rejected =="
reset_registry
mkdir -p pkgEscape/bin
cat > pkgEscape/package.json <<'JSON'
{
  "name": "@test/pkg-escape",
  "version": "1.0.0",
  "files": ["bin/tool", "../outside.txt"]
}
JSON
printf '#!/bin/sh\necho t\n' > pkgEscape/bin/tool
chmod +x pkgEscape/bin/tool
if PATH="$work/bin:$PATH" bash "$PUB" pkgEscape >"$work/out17" 2>&1; then
  fail "files[] traversal entry accepted"
else
  ok "files[] traversal entry rejected"
  grep -q "invalid files" "$work/out17" \
    && ok "rejection names the bad entry" || fail "unclear rejection"
fi

echo "== pkg-preflight: non-array files rejected =="
reset_registry
mkdir -p pkgFilesStr/bin
cat > pkgFilesStr/package.json <<'JSON'
{
  "name": "@test/pkg-files-str",
  "version": "1.0.0",
  "files": "bin/tool"
}
JSON
printf '#!/bin/sh\necho t\n' > pkgFilesStr/bin/tool
chmod +x pkgFilesStr/bin/tool
if PATH="$work/bin:$PATH" bash "$PUB" pkgFilesStr >"$work/out18" 2>&1; then
  fail "non-array files accepted"
else
  ok "non-array files rejected"
fi

echo "== dependency order: dependency first is accepted =="
reset_registry
if PATH="$work/bin:$PATH" bash "$PUB" pkgA pkgMeta >"$work/out14" 2>&1; then
  ok "dependency-first order accepted"
else
  fail "dependency-first order rejected: $(cat "$work/out14")"
fi
grep -q 'PUBLISH @test/pkg-a@1.0.0' "$FAKE_NPM_ROOT/publish.log" \
  && grep -q 'PUBLISH @test/pkg-meta@1.0.0' "$FAKE_NPM_ROOT/publish.log" \
  && ok "publish order preserved (dep then dependent)" || fail "order wrong"

# --- round-7 guarantees: real npm pack, slash-relative args, pre-publish
# registry reconciliation, artifact member-set/identity (#225 r7) ------

echo "== slash-relative package dir packs the LOCAL directory =="
# Real npm resolves a bare "dir/sub" argument as a GitHub repository
# spec (github:dir/sub), NOT a filesystem path — verified against npm
# 11: it runs `git ls-remote ssh://git@github.com/dir/sub.git`. The
# publish path must therefore never hand npm a non-absolute path.
reset_registry
mkdir -p pkgs
(cd pkgs && mkpkg pkgC "@test/pkg-c" "payload-C-v1")
if PATH="$work/bin:$PATH" bash "$PUB" pkgs/pkgC >"$work/out19" 2>&1; then
  ok "slash-relative package dir accepted"
else
  fail "slash-relative package dir rejected: $(cat "$work/out19")"
fi
grep -q 'PUBLISH @test/pkg-c@1.0.0' "$FAKE_NPM_ROOT/publish.log" \
  && ok "local package published, not a GitHub fetch" \
  || fail "publish log wrong: $(cat "$FAKE_NPM_ROOT/publish.log" 2>/dev/null)"
# The registry tarball must carry THIS fixture's payload — proof the
# packed artifact came from pkgs/pkgC, not some other source.
key="$(printf '%s' '@test/pkg-c@1.0.0' | tr '/@' '__')"
tar -xzOf "$FAKE_NPM_ROOT/tarballs/$key.tgz" package/payload.txt 2>/dev/null \
  | grep -q 'payload-C-v1' \
  && ok "registry artifact contains local payload" \
  || fail "registry artifact content wrong"

echo "== divergent ALREADY-PUBLISHED later package blocks ALL publishes =="
# pkgB@1.0.0 exists in the registry with DIFFERENT content. The old
# interleaved Phase E would publish fresh pkgA first and only discover
# the divergence afterwards — a partial release. E1 reconciliation
# must reject before a single publish.
reset_registry
# Seed a divergent published artifact for pkgB (real pack shape).
divtmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-div.XXXXXX")"
mkdir -p "$divtmp/package"
cp -r pkgB/. "$divtmp/package/"
printf 'payload-B-DIVERGENT\n' > "$divtmp/package/payload.txt"
tar -czf "$work/divergent.tgz" -C "$divtmp" package
rm -rf "$divtmp"
bkey="$(printf '%s' '@test/pkg-b@1.0.0' | tr '/@' '__')"
cp "$work/divergent.tgz" "$FAKE_NPM_ROOT/tarballs/$bkey.tgz"
echo "1.0.0" > "$FAKE_NPM_ROOT/published/$bkey"
if PATH="$work/bin:$PATH" bash "$PUB" pkgA pkgB >"$work/out20" 2>&1; then
  fail "divergent pre-published package was accepted"
else
  ok "divergent pre-published package rejected"
  grep -q "refusing to publish anything" "$work/out20" \
    && ok "rejection names pre-publish reconciliation" || fail "unclear rejection"
fi
[ ! -e "$FAKE_NPM_ROOT/publish.log" ] \
  && ok "ZERO publishes when a later package diverges" \
  || fail "partial publish occurred: $(cat "$FAKE_NPM_ROOT/publish.log")"

echo "== identical pre-published later package skips, fresh one publishes =="
reset_registry
# Seed an IDENTICAL artifact for pkgB by publishing it once, then
# rerun pkgA(fresh)+pkgB(published): only pkgA may be published.
if PATH="$work/bin:$PATH" bash "$PUB" pkgB >"$work/out21" 2>&1; then
  ok "seed publish of pkgB succeeded"
else
  fail "seed publish failed: $(cat "$work/out21")"
fi
: > "$FAKE_NPM_ROOT/publish.log"
if PATH="$work/bin:$PATH" bash "$PUB" pkgA pkgB >"$work/out22" 2>&1; then
  ok "mixed fresh/published run accepted"
else
  fail "mixed run rejected: $(cat "$work/out22")"
fi
grep -q 'PUBLISH @test/pkg-a@1.0.0' "$FAKE_NPM_ROOT/publish.log" \
  && ! grep -q 'PUBLISH @test/pkg-b@1.0.0' "$FAKE_NPM_ROOT/publish.log" \
  && ok "only the missing package was published" \
  || fail "publish set wrong: $(cat "$FAKE_NPM_ROOT/publish.log")"

echo "== artifact missing a declared files[] member is rejected =="
reset_registry
if FAKE_NPM_DROP_MEMBER=1 PATH="$work/bin:$PATH" bash "$PUB" pkgA >"$work/out23" 2>&1; then
  fail "artifact missing files[] payload was published"
else
  ok "member-set-deficient artifact rejected"
  grep -q "missing from artifact" "$work/out23" \
    && ok "rejection names the missing payload" || fail "unclear rejection: $(cat "$work/out23")"
fi
[ ! -e "$FAKE_NPM_ROOT/publish.log" ] \
  && ok "member-set failure prevented ALL publishes" \
  || fail "partial publish on member-set failure"

if [ "$fails" -gt 0 ]; then
  say "$fails publish-path test(s) FAILED"
  exit 1
fi
say "publish-path tests: all cases pass"

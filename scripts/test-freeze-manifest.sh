#!/usr/bin/env bash
# test-freeze-manifest.sh — fixture tests for the approved-manifest
# pipeline (#225 A01-A06): verify-freeze-manifest.py must reject
# identity substitution and missing/duplicate/corrupt artifacts, and
# freeze-manifest.sh must reject foreign freeze-meta bindings.
set -euo pipefail
cd "$(dirname "$0")/.."

VERIFY=scripts/verify-freeze-manifest.py
PRODUCER=scripts/freeze-manifest.sh
fails=0
say() { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; fails=$((fails + 1)); }
ok() { say "ok: $*"; }

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-fman.XXXXXX")"
trap 'rm -rf "$work"' EXIT

RUN_ID=777000111
RUN_ATTEMPT=3
HEAD_SHA="$(git rev-parse HEAD)"
export RUN_ID RUN_ATTEMPT HEAD_SHA

verify() { # extra args appended
  python3 "$VERIFY" "$MAN" --run-id "$RUN_ID" --run-attempt "$RUN_ATTEMPT" \
    --head-sha "$HEAD_SHA" --event push --artifact-dir "$art" "$@"
}
expect_ok() { local d="$1"; shift; if verify "$@" >/dev/null 2>&1; then ok "$d"; else fail "$d (unexpected rejection)"; fi; }
expect_reject() { local d="$1"; shift; if verify "$@" >/dev/null 2>&1; then fail "$d (unexpectedly accepted)"; else ok "$d"; fi; }
sha() { sha256sum "$1" 2>/dev/null | awk '{print $1}' || shasum -a 256 "$1" | awk '{print $1}'; }

# --- synthetic manifest + artifact tree -------------------------------
mkdir -p "$work/arts/frozen-linux-x64" "$work/arts/approved-manifest"
art="$work/arts"
export art
printf 'tarball-bytes\n' > "$art/frozen-linux-x64/skill-graph-x86_64-unknown-linux-gnu.tar.gz"
printf 'tgz-bytes\n'    > "$art/frozen-linux-x64/rebuildup-skill-graph-linux-x64-0.5.0.tgz"
printf 'meta-tgz\n'    > "$art/approved-manifest/rebuildup-skill-graph-0.5.0.tgz"
printf 'crate-bytes\n' > "$art/approved-manifest/skill-graph-0.5.0.crate"

MAN="$art/approved-manifest/freeze-manifest.json"
export MAN

python3 - "$art" "$HEAD_SHA" "$RUN_ID" "$RUN_ATTEMPT" <<'PY'
import hashlib, json, os, sys
art, sha_, rid, att = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
def h(p):
    return hashlib.sha256(open(p,'rb').read()).hexdigest()
arts = {
  "skill-graph-x86_64-unknown-linux-gnu.tar.gz": ("release","x86_64-unknown-linux-gnu","frozen-linux-x64/"),
  "rebuildup-skill-graph-linux-x64-0.5.0.tgz": ("npm","linux-x64","frozen-linux-x64/"),
  "rebuildup-skill-graph-0.5.0.tgz": ("npm-meta",None,"approved-manifest/"),
  "skill-graph-0.5.0.crate": ("crates",None,"approved-manifest/"),
}
out = {}
for name,(ch,tgt,sub) in arts.items():
    e = {"sha256": h(os.path.join(art,sub,name)), "channel": ch}
    if tgt: e["target"] = tgt
    out[name] = e
m = {"schema":"skill-graph/freeze-manifest/1","workflow":"artifact-verify.yml",
     "event":"push","ref":"refs/heads/release-0-5-0","head_sha":sha_,
     "run_id":rid,"run_attempt":att,"artifacts":out}
open(os.path.join(art,"approved-manifest","freeze-manifest.json"),"w").write(json.dumps(m,indent=2,sort_keys=True)+"\n")
PY

expect_ok "valid manifest+artifacts accepted" \
  --require-channel release --require-channel npm --require-channel npm-meta --require-channel crates
# wrong run_id / attempt / sha / event need custom invocations:
if python3 "$VERIFY" "$MAN" --run-id 1 --run-attempt "$RUN_ATTEMPT" --head-sha "$HEAD_SHA" --event push --artifact-dir "$art" >/dev/null 2>&1; then
  fail "wrong run_id accepted"; else ok "wrong run_id rejected"; fi
if python3 "$VERIFY" "$MAN" --run-id "$RUN_ID" --run-attempt 9 --head-sha "$HEAD_SHA" --event push --artifact-dir "$art" >/dev/null 2>&1; then
  fail "wrong attempt accepted"; else ok "wrong attempt rejected"; fi
if python3 "$VERIFY" "$MAN" --run-id "$RUN_ID" --run-attempt "$RUN_ATTEMPT" --head-sha deadbeef --event push --artifact-dir "$art" >/dev/null 2>&1; then
  fail "wrong sha accepted"; else ok "wrong sha rejected"; fi
if python3 "$VERIFY" "$MAN" --run-id "$RUN_ID" --run-attempt "$RUN_ATTEMPT" --head-sha "$HEAD_SHA" --event pull_request --artifact-dir "$art" >/dev/null 2>&1; then
  fail "wrong event accepted"; else ok "wrong event rejected"; fi
expect_reject "absent required channel rejected" --require-channel bogus

# corrupt one artifact
printf 'tampered\n' > "$art/approved-manifest/skill-graph-0.5.0.crate"
expect_reject "corrupt artifact digest rejected"
printf 'crate-bytes\n' > "$art/approved-manifest/skill-graph-0.5.0.crate"

# remove an artifact entirely
mv "$art/frozen-linux-x64/rebuildup-skill-graph-linux-x64-0.5.0.tgz" "$work/gone.tgz"
expect_reject "missing artifact rejected"
mv "$work/gone.tgz" "$art/frozen-linux-x64/rebuildup-skill-graph-linux-x64-0.5.0.tgz"

# duplicate artifact name in a second dir
mkdir -p "$art/dup"
cp "$art/approved-manifest/skill-graph-0.5.0.crate" "$art/dup/"
if python3 "$VERIFY" "$MAN" --run-id "$RUN_ID" --run-attempt "$RUN_ATTEMPT" \
     --head-sha "$HEAD_SHA" --event push --artifact-dir "$art" --artifact-dir "$art/dup" >/dev/null 2>&1; then
  fail "duplicate artifact accepted"; else ok "duplicate artifact rejected"; fi

# --- producer negative: foreign freeze-meta must be rejected ---------
fix="$work/fix"
mkdir -p "$fix/frozen-x"
printf 'x\n' > "$fix/frozen-x/skill-graph-aarch64-apple-darwin.tar.gz"
dsum="$(sha "$fix/frozen-x/skill-graph-aarch64-apple-darwin.tar.gz")"
printf '%s  skill-graph-aarch64-apple-darwin.tar.gz\n' "$dsum" > "$fix/frozen-x/skill-graph-aarch64-apple-darwin.tar.gz.sha256"
python3 - "$fix/frozen-x" <<'PY'
import json,sys,hashlib
d=sys.argv[1]
sha=hashlib.sha256(open(d+"/skill-graph-aarch64-apple-darwin.tar.gz",'rb').read()).hexdigest()
m={"workflow":"artifact-verify.yml","head_sha":"0"*40,"run_id":1,"run_attempt":1,
   "target":"aarch64-apple-darwin","npm_target":"darwin-arm64",
   "asset":"skill-graph-aarch64-apple-darwin.tar.gz","sha256":sha}
open(d+"/freeze-meta.json","w").write(json.dumps(m))
PY
if env GITHUB_RUN_ID="$RUN_ID" GITHUB_RUN_ATTEMPT="$RUN_ATTEMPT" GITHUB_SHA="$HEAD_SHA" \
     GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/x \
     bash "$PRODUCER" "$fix" "$work/out" >/dev/null 2>&1; then
  fail "producer accepted foreign freeze-meta"; else ok "producer rejects foreign freeze-meta"; fi

if [ "$fails" -gt 0 ]; then say "$fails freeze-manifest test(s) FAILED"; exit 1; fi
say "freeze-manifest tests: all cases pass"

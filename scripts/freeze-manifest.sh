#!/usr/bin/env bash
# freeze-manifest.sh — aggregate EVERY frozen distribution artifact
# into the single APPROVED manifest that release/npm/crates
# promotion consumes (#225 A01-A06).
#
# Runs as the `freeze-manifest` job of artifact-verify.yml AFTER the
# per-target matrix. It does NOT rebuild anything it did not verify:
#
#   * every frozen-*/ dir's freeze-meta.json must bind THIS run
#     (run_id, run_attempt, head_sha) — a stale or foreign artifact
#     set is rejected;
#   * every tarball/.tgz/.crate digest is recomputed from bytes;
#   * the platform-independent artifacts (metapackage .tgz, .crate)
#     are produced HERE, once, from the same checkout, verified, and
#     digested — npm/crates channels then publish THESE bytes.
#
# Output (in <out-dir>):
#   freeze-manifest.json  — the approval record: identity (workflow,
#     event, ref, head_sha, run_id, run_attempt) + every artifact's
#     sha256/channel/target
#   <metapackage>.tgz, <crate>.crate + .sha256 sidecars — the bytes
#     downstream channels must publish
#
# Usage: freeze-manifest.sh <frozen-parent-dir> <out-dir>
# Env:    GITHUB_RUN_ID GITHUB_RUN_ATTEMPT GITHUB_SHA GITHUB_EVENT_NAME GITHUB_REF
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"

frozen_parent="${1:?usage: freeze-manifest.sh <frozen-parent> <out-dir>}"
out_dir="${2:?usage: freeze-manifest.sh <frozen-parent> <out-dir>}"
: "${GITHUB_RUN_ID:?}" "${GITHUB_RUN_ATTEMPT:?}" "${GITHUB_SHA:?}"
: "${GITHUB_EVENT_NAME:?}" "${GITHUB_REF:?}"

mkdir -p "$out_dir"
work="$(mktemp -d "${TMPDIR:-/tmp}/sg-freeze.XXXXXX")"
trap 'rm -rf "$work"' EXIT

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

# ------------------------------------------------------------------
# 1. Collect + verify every per-target frozen artifact.
# ------------------------------------------------------------------
entries_tsv="$work/entries.tsv"   # channel \t target-or-empty \t filename \t sha256
: > "$entries_tsv"
tarball_count=0 npm_tgz_count=0
shopt -s nullglob
for d in "$frozen_parent"/frozen-*/; do
  [ -d "$d" ] || continue
  meta="$d/freeze-meta.json"
  [ -f "$meta" ] || { echo "missing freeze-meta.json in $d" >&2; exit 1; }
  # The meta must bind THIS run exactly — run, attempt, commit,
  # workflow. Anything else means the artifact set did not come from
  # the run being promoted.
  python3 - "$meta" <<'PY'
import json, os, sys
m = json.load(open(sys.argv[1]))
checks = {
    "workflow": "artifact-verify.yml",
    "run_id": int(os.environ["GITHUB_RUN_ID"]),
    "run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]),
    "head_sha": os.environ["GITHUB_SHA"],
}
for k, want in checks.items():
    assert m.get(k) == want, f"freeze-meta {k}={m.get(k)!r} != {want!r}"
assert isinstance(m.get("target"), str) and m["target"], m
assert isinstance(m.get("npm_target"), str) and m["npm_target"], m
assert m["asset"].startswith("skill-graph-") and m["asset"].endswith(".tar.gz"), m
assert m["asset"] == f"skill-graph-{m['target']}.tar.gz", m
assert len(m.get("sha256", "")) == 64, m
PY
  target="$(python3 -c "import json;print(json.load(open('$meta'))['target'])")"
  npm_target="$(python3 -c "import json;print(json.load(open('$meta'))['npm_target'])")"
  asset="$(python3 -c "import json;print(json.load(open('$meta'))['asset'])")"
  [ -f "$d/$asset" ] || { echo "missing $asset in $d" >&2; exit 1; }
  # Digest: bytes vs sidecar vs meta — all three must agree.
  got="$(sha256_of "$d/$asset")"
  side="$(awk '{print $1}' "$d/$asset.sha256")"
  meta_d="$(python3 -c "import json;print(json.load(open('$meta'))['sha256'])")"
  [ "$got" = "$side" ] && [ "$got" = "$meta_d" ] || {
    echo "digest mismatch for $asset in $d" >&2; exit 1; }
  printf 'release\t%s\t%s\t%s\n' "$target" "$asset" "$got" >> "$entries_tsv"
  tarball_count=$((tarball_count + 1))

  # Platform npm tgz frozen alongside (package-npm-platform.sh).
  plat_tgz="$(find "$d" -maxdepth 1 -name 'rebuildup-skill-graph-*.tgz' -print -quit)"
  [ -n "$plat_tgz" ] || { echo "missing platform npm tgz in $d" >&2; exit 1; }
  [ "$(find "$d" -maxdepth 1 -name '*.tgz' | wc -l)" -eq 1 ] || {
    echo "expected exactly one npm tgz in $d" >&2; exit 1; }
  plat_name="$(basename "$plat_tgz")"
  psha="$(sha256_of "$plat_tgz")"
  pside="$(awk '{print $1}' "$plat_tgz.sha256")"
  [ "$psha" = "$pside" ] || { echo "npm tgz digest mismatch in $d" >&2; exit 1; }
  # The tgz must declare the npm_target this matrix leg built for.
  node -e '
    const { execFileSync } = require("node:child_process");
    const m = JSON.parse(execFileSync("tar", ["-xzOf", process.argv[1], "package/package.json"], {encoding:"utf8"}));
    const want = "@rebuildup/skill-graph-" + process.argv[2];
    if (m.name !== want) { console.error(`tgz name ${m.name} != ${want}`); process.exit(1); }
  ' "$plat_tgz" "$npm_target"
  printf 'npm\t%s\t%s\t%s\n' "$npm_target" "$plat_name" "$psha" >> "$entries_tsv"
  npm_tgz_count=$((npm_tgz_count + 1))
done
shopt -u nullglob
[ "$tarball_count" -eq 4 ] || {
  echo "expected 4 frozen targets, found $tarball_count" >&2; exit 1; }
[ "$npm_tgz_count" -eq 4 ] || {
  echo "expected 4 platform npm tgz, found $npm_tgz_count" >&2; exit 1; }

# ------------------------------------------------------------------
# 2. Platform-independent artifacts: metapackage tgz + crate.
#    Built once here from THIS checkout; their digests enter the
#    manifest so downstream channels publish exactly these bytes.
# ------------------------------------------------------------------
version="$(grep -m1 '^version' Cargo.toml | sed -E 's/^version = "([^"]+)".*/\1/')"
[ -n "$version" ] || { echo "cannot resolve version" >&2; exit 1; }

# 2a. Metapackage: npm/skill-graph + plugin + contracts (same bundle
#     verify-install-local.sh exercises).
meta_stage="$work/meta-pkg"
mkdir -p "$meta_stage"
cp -r npm/skill-graph/. "$meta_stage/"
cp -r plugins/skill-graph "$meta_stage/plugin"
cp -r contracts "$meta_stage/contracts"
meta_out="$work/meta-out"
mkdir -p "$meta_out"
meta_tgz_name="$(cd "$meta_stage" && npm pack --silent --pack-destination "$meta_out" | tail -1)"
[ -f "$meta_out/$meta_tgz_name" ] || { echo "meta npm pack failed" >&2; exit 1; }
"$here/verify-npm-archive.sh" --check-bin "$meta_out/$meta_tgz_name"
meta_sha="$(sha256_of "$meta_out/$meta_tgz_name")"
node -e '
  const { execFileSync } = require("node:child_process");
  const m = JSON.parse(execFileSync("tar", ["-xzOf", process.argv[1], "package/package.json"], {encoding:"utf8"}));
  if (m.name !== "@rebuildup/skill-graph") { console.error(`meta tgz name ${m.name}`); process.exit(1); }
' "$meta_out/$meta_tgz_name"
printf 'npm-meta\t\t%s\t%s\n' "$meta_tgz_name" "$meta_sha" >> "$entries_tsv"

# 2b. Crate: `cargo package` of THIS checkout — the exact file set
#     crates.io receives. Cargo.toml `include` whitelists the crate
#     contents, so only changes under packaged paths can alter the
#     bytes: reject tracked modifications anywhere (a dirty lockfile
#     or manifest changes packaging) and any untracked file under the
#     packaged roots. Untracked files elsewhere (e.g. the downloaded
#     frozen/ artifact dirs this job itself created) cannot enter
#     the .crate and are permitted.
tracked_dirty="$(git status --porcelain | grep -v '^??' || true)"
untracked_pkg="$(git ls-files --others --exclude-standard -- src contracts Cargo.toml LICENSE README.md || true)"
if [ -n "$tracked_dirty" ] || [ -n "$untracked_pkg" ]; then
  echo "working tree differs from $GITHUB_SHA under packaged paths —" >&2
  echo "cannot freeze crate bytes" >&2
  printf '%s\n' "$tracked_dirty" "$untracked_pkg" >&2
  exit 1
fi
cargo package --locked >/dev/null
crate="$(ls -t target/package/skill-graph-*.crate 2>/dev/null | head -1)"
[ -n "$crate" ] && [ -f "$crate" ] || { echo "cargo package produced no .crate" >&2; exit 1; }
crate_name="$(basename "$crate")"
crate_sha="$(sha256_of "$crate")"
printf 'crates\t\t%s\t%s\n' "$crate_name" "$crate_sha" >> "$entries_tsv"

# ------------------------------------------------------------------
# 3. Emit the approved manifest + copy publishable bytes to out-dir.
# ------------------------------------------------------------------
cp "$meta_out/$meta_tgz_name" "$out_dir/$meta_tgz_name"
sha256_of "$out_dir/$meta_tgz_name" > "$out_dir/$meta_tgz_name.sha256"
cp "$crate" "$out_dir/$crate_name"
sha256_of "$out_dir/$crate_name" > "$out_dir/$crate_name.sha256"

# Platform tgz bytes travel inside their frozen-* artifacts; the
# manifest records digests only — consumers locate files by name.
python3 - "$entries_tsv" "$out_dir/freeze-manifest.json" <<'PY'
import json, os, sys
entries = []
for line in open(sys.argv[1]):
    channel, target, name, sha = line.rstrip("\n").split("\t")
    entries.append({
        "name": name,
        "sha256": sha,
        "channel": channel,
        "target": target or None,
    })
names = [e["name"] for e in entries]
assert len(names) == len(set(names)), f"duplicate artifact names: {names}"
manifest = {
    "schema": "skill-graph/freeze-manifest/1",
    "workflow": "artifact-verify.yml",
    "event": os.environ["GITHUB_EVENT_NAME"],
    "ref": os.environ["GITHUB_REF"],
    "head_sha": os.environ["GITHUB_SHA"],
    "run_id": int(os.environ["GITHUB_RUN_ID"]),
    "run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]),
    "artifacts": {e["name"]: {"sha256": e["sha256"], "channel": e["channel"],
                              **({"target": e["target"]} if e["target"] else {})}
                  for e in entries},
}
with open(sys.argv[2], "w") as f:
    json.dump(manifest, f, indent=2, sort_keys=True)
    f.write("\n")
print(f"freeze-manifest: {len(entries)} artifacts "
      f"({sum(1 for e in entries if e['channel']=='release')} release, "
      f"{sum(1 for e in entries if e['channel']=='npm')} npm-platform, "
      f"1 npm-meta, 1 crates)")
PY

echo "freeze manifest + publishable bytes written to $out_dir"
ls -l "$out_dir"

#!/usr/bin/env python3
"""verify-freeze-manifest.py — validate a freeze-manifest.json against
the EXACT run/SHA/attempt being promoted, then prove every listed
artifact exists with its recorded digest (#225 A01-A05, A09).

The manifest is the APPROVAL RECORD produced by artifact-verify.yml's
freeze-manifest job. Promotion paths (release.yml, npm-publish.yml,
crates verification) must:

  1. bind it to the pinned freeze run — run_id, run_attempt,
     head_sha, event, workflow — rejecting substitution of any other
     run's output (A01/A03);
  2. locate every artifact it names across the supplied artifact
     directories, exactly once, with a matching sha256 — missing,
     duplicated, or mismatched bytes are rejected (A05);
  3. optionally restrict which channels must be present (e.g. the
     release job requires channel=release; npm requires channel=npm*).

Usage:
  verify-freeze-manifest.py MANIFEST \
      --run-id N --run-attempt N --head-sha SHA --event push \
      --artifact-dir DIR [--artifact-dir DIR ...] \
      [--require-channel release] [--require-channel npm] ...

Exit 0 prints nothing on success; diagnostics go to stderr.
"""
import argparse
import hashlib
import json
import os
import re
import sys

SCHEMA = "skill-graph/freeze-manifest/1"
WORKFLOW = "artifact-verify.yml"
CHANNELS = {"release", "npm", "npm-meta", "crates"}
NAME_RE = re.compile(r"^[A-Za-z0-9@._/+~-]+$")
SHA_RE = re.compile(r"^[0-9a-f]{64}$")

# The approved artifact SET is fixed (#225 A03): exactly these
# channels/counts/targets, with name<->channel<->target correspondence.
# Anything extra, missing, or mislabeled is rejected — a manifest that
# merely "contains" the right digests somewhere is not the approval
# record.
EXPECTED_COUNTS = {"release": 4, "npm": 4, "npm-meta": 1, "crates": 1}
RELEASE_TARGETS = {
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
}
RELEASE_NAME_RE = re.compile(r"^skill-graph-(.+)\.tar\.gz$")
NPM_NAME_RE = re.compile(
    r"^rebuildup-skill-graph-(linux-x64|linux-arm64|darwin-x64|darwin-arm64)-[^/]+\.tgz$"
)
NPM_META_NAME_RE = re.compile(r"^rebuildup-skill-graph-[0-9][^/]*\.tgz$")
CRATE_NAME_RE = re.compile(r"^skill-graph-[0-9][^/]*\.crate$")


def fail(msg):
    print(f"freeze-manifest: {msg}", file=sys.stderr)
    sys.exit(1)


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("manifest")
    ap.add_argument("--run-id", type=int, required=True)
    ap.add_argument("--run-attempt", type=int, required=True)
    ap.add_argument("--head-sha", required=True)
    ap.add_argument("--event", required=True)
    ap.add_argument("--artifact-dir", action="append", default=[])
    ap.add_argument("--require-channel", action="append", default=[])
    # Exact git ref(s) the manifest's `ref` must equal — derived by the
    # caller from the PINNED run's head_branch (refs/heads/<b> or
    # refs/tags/<t>). A bare "starts with refs/" check let a manifest
    # claim any ref (#225 A04).
    ap.add_argument("--expect-ref", action="append", default=[])
    args = ap.parse_args()

    try:
        m = json.load(open(args.manifest))
    except Exception as e:
        fail(f"cannot parse manifest: {e}")
    if not isinstance(m, dict):
        fail("manifest is not a JSON object")

    # ---- identity binding (A01/A03/A09) ----
    if m.get("schema") != SCHEMA:
        fail(f"schema {m.get('schema')!r} != {SCHEMA!r}")
    if m.get("workflow") != WORKFLOW:
        fail(f"workflow {m.get('workflow')!r} != {WORKFLOW!r}")
    for key, want in (
        ("run_id", args.run_id),
        ("run_attempt", args.run_attempt),
        ("head_sha", args.head_sha),
        ("event", args.event),
    ):
        if m.get(key) != want:
            fail(f"{key} {m.get(key)!r} != expected {want!r}")
    ref = m.get("ref")
    if args.expect_ref:
        if ref not in args.expect_ref:
            fail(f"ref {ref!r} not in expected {args.expect_ref}")
    elif not isinstance(ref, str) or not ref.startswith("refs/"):
        fail(f"ref {ref!r} is not a git ref")

    arts = m.get("artifacts")
    if not isinstance(arts, dict) or not arts:
        fail("manifest has no artifacts map")

    # ---- strict artifact set (A03): exactly 10 entries, fixed
    # channel counts, and name<->channel<->target correspondence ----
    counts = {}
    for name, meta in arts.items():
        if not NAME_RE.match(name):
            fail(f"artifact name {name!r} is not a safe filename")
        if not isinstance(meta, dict):
            fail(f"artifact {name}: entry is not an object")
        sha = meta.get("sha256")
        if not isinstance(sha, str) or not SHA_RE.match(sha):
            fail(f"artifact {name}: bad sha256 {sha!r}")
        ch = meta.get("channel")
        if ch not in CHANNELS:
            fail(f"artifact {name}: unknown channel {ch!r}")
        counts[ch] = counts.get(ch, 0) + 1
        target = meta.get("target")
        if ch == "release":
            mm = RELEASE_NAME_RE.match(name)
            if not mm or mm.group(1) not in RELEASE_TARGETS:
                fail(f"artifact {name}: release name/target mismatch")
            if target != mm.group(1):
                fail(f"artifact {name}: target {target!r} != name target {mm.group(1)!r}")
        elif ch == "npm":
            mm = NPM_NAME_RE.match(name)
            if not mm:
                fail(f"artifact {name}: npm platform name mismatch")
            if target != mm.group(1):
                fail(f"artifact {name}: target {target!r} != name target {mm.group(1)!r}")
        elif ch == "npm-meta":
            if target is not None or not NPM_META_NAME_RE.match(name):
                fail(f"artifact {name}: npm-meta name/target mismatch")
        elif ch == "crates":
            if target is not None or not CRATE_NAME_RE.match(name):
                fail(f"artifact {name}: crates name/target mismatch")
    if len(arts) != sum(EXPECTED_COUNTS.values()):
        fail(f"artifact count {len(arts)} != {sum(EXPECTED_COUNTS.values())}")
    for ch, want in EXPECTED_COUNTS.items():
        if counts.get(ch, 0) != want:
            fail(f"channel {ch}: {counts.get(ch, 0)} artifacts != {want}")

    channels = set(counts)
    for req in args.require_channel:
        if req not in channels:
            fail(f"required channel {req!r} absent from manifest")

    # ---- artifact presence + digest (A05) ----
    if args.artifact_dir:
        # Index every regular file under the dirs by basename.
        found = {}
        for d in args.artifact_dir:
            if not os.path.isdir(d):
                fail(f"artifact dir missing: {d}")
            for base, _dirs, files in os.walk(d):
                for fn in files:
                    p = os.path.join(base, fn)
                    found.setdefault(fn, []).append(p)
        for name, meta in arts.items():
            paths = found.get(name, [])
            if len(paths) == 0:
                fail(f"artifact {name} not found under {args.artifact_dir}")
            if len(paths) > 1:
                fail(f"artifact {name} found {len(paths)} times — ambiguous")
            got = sha256_of(paths[0])
            if got != meta["sha256"]:
                fail(f"artifact {name} digest mismatch: {got} != {meta['sha256']}")

    print(f"freeze-manifest OK: {len(arts)} artifacts, "
          f"channels={sorted(channels)}, run={args.run_id} "
          f"attempt={args.run_attempt} sha={args.head_sha[:12]}")


if __name__ == "__main__":
    main()

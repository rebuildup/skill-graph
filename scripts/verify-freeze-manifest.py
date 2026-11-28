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
    if not isinstance(ref, str) or not ref.startswith("refs/"):
        fail(f"ref {ref!r} is not a git ref")

    arts = m.get("artifacts")
    if not isinstance(arts, dict) or not arts:
        fail("manifest has no artifacts map")
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
        if ch == "release" and not (
            name.startswith("skill-graph-") and name.endswith(".tar.gz")
        ):
            fail(f"artifact {name}: release channel name mismatch")
        if "target" in meta and not isinstance(meta["target"], str):
            fail(f"artifact {name}: target is not a string")

    channels = {a["channel"] for a in arts.values()}
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

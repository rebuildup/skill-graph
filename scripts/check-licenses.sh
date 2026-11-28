#!/usr/bin/env bash
# Dependency license audit (#225 F11): every crates.io dependency in
# Cargo.lock must offer at least one license option from the
# permissive allowlist. Expressions are evaluated per OR-branch —
# e.g. "MIT OR Apache-2.0 OR LGPL-2.1-or-later" passes because the
# user may choose MIT; an expression whose EVERY branch contains a
# restricted license fails. AND-branches require all members
# allowlisted.
set -euo pipefail
cd "$(dirname "$0")/.."

meta_file="$(mktemp "${TMPDIR:-/tmp}/sg-meta.XXXXXX")"
trap 'rm -f "$meta_file"' EXIT
cargo metadata --format-version 1 --locked --offline > "$meta_file"
python3 - "$meta_file" <<'PY'
import json, re, sys

ALLOW = {
    "MIT", "MIT-0", "Apache-2.0", "BSD-1-Clause", "BSD-2-Clause",
    "BSD-3-Clause", "ISC", "Zlib", "Unlicense", "CC0-1.0",
    "Unicode-3.0", "Unicode-DFS-2016", "CDLA-Permissive-2.0",
    "BSL-1.0", "OpenSSL", "MPL-2.0",
}
RESTRICTED = re.compile(r"GPL|LGPL|AGPL|SSPL|Commons-Clause|EUPL|OSL|CPL", re.I)
ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9.\-+]*")

def branch_ok(branch: str) -> bool:
    # strip WITH exceptions (LLVM-exception etc.) — they don't change
    # the license family for our purposes
    ids = [t for t in ID.findall(branch) if t not in {"AND", "OR", "WITH"}]
    ids = [i for i in ids if not i.lower().endswith("exception")]
    return bool(ids) and all(i in ALLOW for i in ids)

def top_level_or_branches(expr: str):
    # Split on ' OR ' only at paren depth 0 — OR inside parentheses
    # binds to its group, not the whole expression.
    branches, depth, cur = [], 0, ""
    i = 0
    while i < len(expr):
        if expr.startswith(" OR ", i) and depth == 0:
            branches.append(cur)
            cur = ""
            i += 4
            continue
        ch = expr[i]
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        cur += ch
        i += 1
    branches.append(cur)
    return branches


def expr_ok(expr: str) -> bool:
    # An expression passes when SOME top-level OR branch is fully
    # allowlisted; AND members must all be allowlisted (branch_ok).
    return any(branch_ok(b) for b in top_level_or_branches(expr))

meta = json.load(open(sys.argv[1]))
bad = []
for p in meta["packages"]:
    if p["name"] == "skill-graph":
        continue
    lic = p.get("license")
    if not lic:
        bad.append((p["name"], p["version"], "<none>"))
        continue
    if not expr_ok(lic):
        bad.append((p["name"], p["version"], lic))

for name, ver, lic in sorted(bad):
    print(f"RESTRICTED {name} {ver}: {lic}", file=sys.stderr)
if bad:
    sys.exit(f"{len(bad)} dependencies lack a permissive license option")
print(f"license audit OK: {sum(1 for p in meta['packages'] if p['name'] != 'skill-graph')} dependencies, all permissive")
PY

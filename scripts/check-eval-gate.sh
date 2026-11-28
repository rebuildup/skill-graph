#!/usr/bin/env bash
# Eval gate (#225 G04): run the release eval dataset through the
# packaged binary and enforce the thresholds declared in
# docs/specs/evaluation.md. A regression FAILS — the freeze path
# (artifact-verify.yml freeze-manifest job) and CI both call this, so
# an eval regression blocks promotion rather than being reported
# after the fact.
#
# Thresholds are deliberately the "must not regress" subset:
#   - required_recall == 1.0      (every required skill is recommended)
#   - required_recall_at3 == 1.0  (within the top-3 budget)
#   - forbidden_hits == 0         (no forbidden skill ever surfaces)
#   - cases == dataset size       (the whole dataset actually ran —
#                                  a truncated/empty run cannot pass)
# Precision/recall@5/elapsed are informational and intentionally NOT
# gated: they may legitimately move with ranking changes.
set -euo pipefail

bin="${1:-./target/release/skill-graph}"
dataset="${2:-evals/dataset/cases.jsonl}"
cd "$(dirname "$0")/.."

[ -x "$bin" ] || { echo "eval-gate: binary not executable: $bin" >&2; exit 1; }
[ -f "$dataset" ] || { echo "eval-gate: dataset missing: $dataset" >&2; exit 1; }

expected_cases="$(grep -c . "$dataset")"

summary="$("$bin" bench "$dataset" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["summary"]))')"

printf '%s' "$summary" | python3 -c '
import json,sys
s=json.loads(sys.argv[1])
expected=int(sys.argv[2])
checks=[
    ("cases",            s["cases"]==expected,          s["cases"]),
    ("required_recall",  s["required_recall"]==1.0,     s["required_recall"]),
    ("required_recall_at3", s["required_recall_at3"]==1.0, s["required_recall_at3"]),
    ("forbidden_hits",   s["forbidden_hits"]==0,        s["forbidden_hits"]),
]
failed=False
for name,ok,value in checks:
    print(("PASS" if ok else "FAIL"), f"{name}={value}")
    failed |= not ok
sys.exit(1 if failed else 0)
' "$summary" "$expected_cases"

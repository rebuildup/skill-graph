# Evidence — C03 synthetic corpus performance benchmark (2026-11-25)

Issue: #9
Branch: `9` → `release-0-1-0`

## Method

`src/perf.rs` generates a synthetic skill corpus in the OS temp dir, runs discovery + resolver build once per size, times `recommend` warm iterations on the built graph, and times cold `skill-graph recommend` process spawns as a Hook-startup proxy. Corpus cleanup is verified (`cleanup_ok: true`).

Command:

```bash
cargo build --release
cargo run --release --quiet -- perf
```

Sizes: 50 / 100 / 200 / 500 skills. Warm recommend iterations: 30 per size. Process samples: 5 per size. Query: `rust cli implementation cargo checks`; context: `module boundaries verification harness`; max recommendations: 5.

## Results (release build)

| Skills | Cold discover ms | Cold build ms | Cold total ms | Warm recommend p50 ms | Warm recommend p95 ms | Process p50 ms | Process p95 ms | Recommended |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 50 | 0.657 | 0.179 | 0.836 | 0.050 | 0.073 | 2.650 | 2.868 | 2 |
| 100 | 1.070 | 0.414 | 1.484 | 0.101 | 0.145 | 3.480 | 4.472 | 2 |
| 200 | 3.376 | 0.847 | 4.223 | 0.232 | 0.326 | 4.949 | 5.430 | 2 |
| 500 | 5.599 | 2.080 | 7.680 | 0.580 | 0.679 | 9.657 | 13.053 | 2 |

Debug-build comparison (`cargo run --quiet -- perf`) was roughly an order of magnitude slower (500 skills: cold total 35.85 ms, warm recommend p95 13.30 ms, process p95 46.41 ms) but still below the 250 ms Hook candidate budget.

## Interpretation

- At 500 synthetic skills, cold discover+resolve+recommend is ~7.7 ms and the full CLI process is ~13 ms p95 in release. Warm in-memory recommendation is sub-millisecond p95.
- There is no evidence that a persistent cache is required for the MVP target range (50–500 skills). Revisit O01 only if real corpora show larger parse costs or if Hook budgets tighten.
- Process startup dominates warm recommendation but remains far below the 250 ms Hook candidate and 100 ms recommendation p95 reference values.

## Limits

- Synthetic corpus uses uniform small `SKILL.md` files; real skill bodies, links, and `requires` density may differ.
- Process samples are 5 per size; treat p95 as indicative, not a tight confidence interval.
- Measurements are single-machine Linux x86_64; cross-platform and signed-binary distribution cost is out of scope for C03.

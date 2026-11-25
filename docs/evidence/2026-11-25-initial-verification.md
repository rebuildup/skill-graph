# Evidence — 2026-11-25 initial verification

Applied project-init revision: `285b139b01c199d3d4135d90f3cc3f6a932eff3b`

## Commands

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --quiet -- bench evals/dataset/cases.jsonl
```

## Results

- `cargo clippy --all-targets -- -D warnings`: passed
- `cargo test`: 3 passed, 0 failed
- `bench evals/dataset/cases.jsonl`: completed; `required_recall = 1.0`, `forbidden_hits = 1`, `empty_recommendations = 1`

## Observations

- Fixture discovery, Markdown link resolution, `requires-skills`, backlinks, and reasoned recommendation are covered by tests.
- The preliminary bench shows a known ranking limitation: `release-notes` recommends `alpha-skill` because alpha's body references release notes and graph boost propagates it. This is recorded as a hypothesis for C02/E02, not as a final quality claim.
- No persistent cache, graph DB, MCP server, or runtime hook was added.

## Not verified yet

- Real 190-session log access and label quality.
- Claude Code / Codex / OpenCode hook payload and context injection behavior.
- 50–500 skill cold/warm benchmark.
- OS signing, notarization, Gatekeeper, SmartScreen.

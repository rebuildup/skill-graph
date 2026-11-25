# Evidence — C01 in-memory Discovery / Parser / Resolver (2026-11-25)

Issue: #7
Branch: `7` → `release-0-1-0`

## Scope

C01 verifies the on-memory core against the S03 v1 contracts and S04 security boundaries: standard `SKILL.md` frontmatter, Markdown links, `requires-skills`, backlinks, content digest, and ambiguous names. No persistent cache or graph DB is introduced.

## Coverage

- `src/discovery.rs`: canonicalized skill roots, `follow_links(false)`, symlink rejection, parse errors collected without aborting discovery.
- `src/parser.rs`: frontmatter `name`/`description`/`metadata`, `requires-skills` / `requires_skills`, Markdown vs external links, SHA-256 content digest, parse-error capture.
- `src/resolver.rs`: Markdown path resolution confined to discovered skills, `requires` resolution only for unique names, mention extraction, backlinks, and `ambiguous_names`.
- Contracts: `contracts/v1/*` and `tests/contract.rs`; security boundaries: `tests/security.rs`.

## Added verification

`tests/core.rs` now also asserts:

- digest is stable for identical bytes, 64 hex chars, and changes when content changes;
- duplicate skill names populate `graph.ambiguous_names` and `by_name` without being uniquified.

Existing fixture tests already cover `requires-skills` → `LinkKind::Requires`, Markdown link resolution, and backlinks.

## Verification

```bash
cargo fmt --all
mise run check
```

`mise run check` passed: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

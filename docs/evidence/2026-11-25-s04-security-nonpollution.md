# Evidence — S04 security and non-pollution contract (2026-11-25)

Issue: #6
Branch: `6` → `release-0-1-0`

## Scope

S04 fixes the trust boundary for untrusted skill documents before C01 builds the discovery/parser/resolver on top of it. The contract is normative in `docs/specs/core-contract.md`; this change adds adversarial regression tests and records the verified behavior.

## Verified guarantees

`tests/security.rs` covers the rejection conditions that the current in-memory implementation can enforce:

- **Symlink traversal**: discovery skips symlinked `SKILL.md` files and does not descend into symlinked skill directories (`follow_links(false)` plus explicit symlink rejection). A temporary skill root is unchanged after discovery.
- **Path traversal**: Markdown links such as `../../outside/secret/SKILL.md` are parsed as references but do not resolve to skills outside the discovered roots.
- **External links**: `https://` and `mailto:` targets are classified as external and are never fetched or resolved as local skills.
- **Parse errors**: malformed frontmatter is surfaced as `parse_error`, kept out of recommendation document-frequency statistics, and skipped by `recommend` even when the fallback name would match the query.

`docs/specs/core-contract.md` now states the symlink and link-resolution boundary explicitly.

## Verification

```bash
cargo fmt --all
mise run check
```

`mise run check` passed: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` (7 passed across `tests/core.rs` and `tests/security.rs`).

## Limits / next

- The tests prove the current filesystem and parser boundaries; they do not prove behavior under a hostile process that can modify files during discovery.
- Session-scoped or cached modes are still out of scope; when introduced they must preserve the same no-write and boundary guarantees.
- Runtime hook trust boundaries remain S02/X03 work.

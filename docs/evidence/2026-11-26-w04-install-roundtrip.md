# W04 install / update / uninstall roundtrip verification (#79)

Date: 2026-11-26. Branch: `79`. OS: Linux (bash, jq, GNU coreutils).
macOS was not available in this environment; the script uses only POSIX-ish
bash + `jq` + `mktemp`, so the same procedure applies, but macOS-specific
runtimes config paths (`~/Library/...`) are not used by any adapter — all
targets are `$HOME`-relative or `$PWD`-relative and are OS-independent.

## Method

`adapters/test-install-roundtrip.sh` runs entirely inside a temporary
`HOME` and project dir. For each runtime × scope it:

1. seeds the target config with unrelated entries (other hooks / other
   plugins / other keys),
2. snapshots it,
3. runs `install.sh <rt> <scope>` → snapshot,
4. runs install again → asserts byte-semantic equality with step 3
   (idempotent update path),
5. runs `--uninstall` → asserts `jq -S` canonical equality with the seed.

## Results (Linux)

```
PASS claude-code --user: reinstall == install
PASS claude-code --user: uninstall restores seed
PASS codex --user: reinstall == install
PASS codex --user: uninstall restores seed
PASS opencode --user: reinstall == install
PASS opencode --user: uninstall restores seed
PASS claude-code --project: reinstall == install
PASS claude-code --project: uninstall restores seed
PASS codex --project: reinstall == install
PASS codex --project: uninstall restores seed
PASS opencode --project: reinstall == install
PASS opencode --project: uninstall restores seed
PASS codex uninstall on absent file
```

Fresh install → uninstall on an absent config removes the created
`hooks.json`/`opencode.json` entirely (config reduced to `{}` is deleted),
so no stray files remain.

## Bugs found and fixed during verification

- `install.sh` exited 1 for non-codex runtimes: trailing
  `[ "$runtime" = codex ] && echo ...` returned false as the last command.
- Uninstall left `{"hooks":{}}` / `{"plugins" removed}` residue; now a
  config reduced to `{}` is removed.
- `install.sh` gained an `opencode` runtime: merges
  `file://<abs>/adapters/opencode/skill-graph` into `opencode.json`
  `plugins` (user: `$XDG_CONFIG_HOME/opencode/opencode.json`, project:
  `./opencode.json`), preserving other plugins and keys; uninstall removes
  only that specifier.

## Boundaries

- Verification used throwaway `HOME`/project dirs; real user configs were
  not touched.
- OpenCode plugin *loading* inside a live OpenCode server remains covered
  by the v0.2.0 live-verification evidence; this ticket covers the config
  merge/unmerge path only.
- Codex `/hooks` trust is documented as the normal post-install step in
  `adapters/codex/skill-graph/README.md`; bypass flags are experiment-only.

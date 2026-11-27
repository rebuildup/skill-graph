#!/usr/bin/env bash
# test-release-verify.sh — fixture tests for the release-verification
# scripts. Proves the verifiers REJECT the failure modes they exist
# for, not just accept the happy path (#225 re-review): content
# mismatch, missing/extra files, permission-bit drift, corrupt
# archives, and wrong architectures.
set -euo pipefail
cd "$(dirname "$0")/.."

TAR_VERIFY=scripts/verify-npm-tarballs.sh
ARCH_VERIFY=scripts/verify-npm-arch.sh
fails=0

say() { printf '%s\n' "$*"; }
fail() { say "FAIL: $*"; fails=$((fails + 1)); }

expect_ok() { # $1=desc, rest=cmd
  local desc="$1"; shift
  if "$@" >/dev/null 2>&1; then say "ok: $desc"; else fail "$desc (unexpected rejection)"; fi
}
expect_reject() { # $1=desc, rest=cmd
  local desc="$1"; shift
  if "$@" >/dev/null 2>&1; then fail "$desc (unexpectedly accepted)"; else say "ok: $desc"; fi
}

work="$(mktemp -d "${TMPDIR:-/tmp}/sg-verify-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# --- tarball fixtures -------------------------------------------------
# npm tarballs extract under package/; build two identical trees then
# pack each with tar (same layout npm produces).
mkpkg() { # $1=dest-dir $2=variant
  local d="$1" v="$2"
  rm -rf "$d"
  mkdir -p "$d/package/bin" "$d/package/plugin"
  printf '#!/bin/sh\necho skill-graph 0.5.0\n' > "$d/package/bin/skill-graph"
  chmod +x "$d/package/bin/skill-graph"
  printf '{"name":"@rebuildup/skill-graph-linux-x64","version":"0.5.0"}\n' \
    > "$d/package/package.json"
  printf 'plugin payload %s\n' "$v" > "$d/package/plugin/index.js"
}

pack() { # $1=pkgdir $2=out.tgz
  tar -czf "$2" -C "$1" package
}

mkdir -p "$work/a" "$work/b"
mkpkg "$work/a" v1
mkpkg "$work/b" v1
pack "$work/a" "$work/a.tgz"
pack "$work/b" "$work/b.tgz"

expect_ok "identical tarballs accepted" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/b.tgz"

# content mismatch
mkpkg "$work/b" v2
pack "$work/b" "$work/b.tgz"
expect_reject "content mismatch rejected" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/b.tgz"

# missing file
rm "$work/b/package/plugin/index.js"
pack "$work/b" "$work/b.tgz"
expect_reject "missing file rejected" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/b.tgz"
mkpkg "$work/b" v1

# extra file
printf 'extra\n' > "$work/b/package/extra.txt"
pack "$work/b" "$work/b.tgz"
expect_reject "extra file rejected" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/b.tgz"
rm "$work/b/package/extra.txt"

# permission drift: same content, exec bit lost — diff -r alone would
# miss this; the mode listing must catch it.
chmod -x "$work/b/package/bin/skill-graph"
pack "$work/b" "$work/b.tgz"
expect_reject "exec-bit drift rejected" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/b.tgz"
chmod +x "$work/b/package/bin/skill-graph"

# file-type drift: regular file replaced by a directory of same name
rm "$work/b/package/bin/skill-graph"
mkdir "$work/b/package/bin/skill-graph"
printf 'not a binary\n' > "$work/b/package/bin/skill-graph/x"
pack "$work/b" "$work/b.tgz"
expect_reject "file->dir drift rejected" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/b.tgz"
mkpkg "$work/b" v1

# corrupt archive
printf 'not a gzip\n' > "$work/corrupt.tgz"
expect_reject "corrupt tarball rejected" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/corrupt.tgz"
expect_reject "corrupt published tarball rejected" \
  bash "$TAR_VERIFY" "$work/corrupt.tgz" "$work/a.tgz"

# missing package/ root (flat tarball)
mkdir -p "$work/flat/root"
printf 'x\n' > "$work/flat/root/file.txt"
tar -czf "$work/flat.tgz" -C "$work/flat/root" .
expect_reject "tarball without package/ root rejected" \
  bash "$TAR_VERIFY" "$work/flat.tgz" "$work/a.tgz"

# Out-of-scope entries: the old verifier diffed only package/ subtrees,
# so extra TOP-LEVEL files were silently accepted (#225 re-review).
# package/ content is identical — only the root scope differs.
mkdir -p "$work/extra-root/package"
cp -r "$work/a/package/." "$work/extra-root/package/"
printf 'smuggled\n' > "$work/extra-root/evil.sh"
tar -czf "$work/extra-pub.tgz" -C "$work/extra-root" package evil.sh
expect_reject "published tarball with extra top-level file rejected" \
  bash "$TAR_VERIFY" "$work/extra-pub.tgz" "$work/a.tgz"

mkdir -p "$work/extra-root2/package" "$work/extra-root2/package2"
cp -r "$work/a/package/." "$work/extra-root2/package/"
printf 'x\n' > "$work/extra-root2/package2/f"
tar -czf "$work/extra-loc.tgz" -C "$work/extra-root2" package package2
expect_reject "local tarball with second root rejected" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/extra-loc.tgz"

# Even IDENTICAL out-of-scope content is rejected: npm tarballs carry
# exactly one root, so scope itself is part of the contract.
mkdir -p "$work/extra-both"
cp -r "$work/a/package" "$work/extra-both/package"
printf 'same\n' > "$work/extra-both/note.txt"
tar -czf "$work/extra-both.tgz" -C "$work/extra-both" package note.txt
expect_reject "identical extra top-level entry still rejected" \
  bash "$TAR_VERIFY" "$work/extra-both.tgz" "$work/a.tgz"

# --- archive-format gate (#225 r5) -------------------------------------
# These reject at the ARCHIVE level, before extraction: the extracted
# tree alone cannot reveal duplicate members (last-wins), member types
# both sides share, or non-gzip containers tar happens to decode.

# uncompressed tar renamed .tgz — magic bytes are not 1f8b
mkdir -p "$work/plain/package"
cp -r "$work/a/package/." "$work/plain/package/"
tar -cf "$work/plain.tar" -C "$work/plain" package
cp "$work/plain.tar" "$work/plain.tgz"
expect_reject "non-gzip tarball rejected (magic)" \
  bash "$TAR_VERIFY" "$work/plain.tgz" "$work/a.tgz"

# truncated gzip stream — magic ok, integrity fails
head -c 64 "$work/a.tgz" > "$work/trunc.tgz"
expect_reject "truncated gzip rejected" \
  bash "$TAR_VERIFY" "$work/trunc.tgz" "$work/a.tgz"

# duplicate member names — extraction is last-wins so the compared
# tree would hide the earlier divergent entry
mkdir -p "$work/dup/package"
cp -r "$work/a/package/." "$work/dup/package/"
tar -czf "$work/dup.tgz" -C "$work/dup" \
  package package/package.json package/package.json
expect_reject "duplicate member rejected" \
  bash "$TAR_VERIFY" "$work/dup.tgz" "$work/a.tgz"

# symlink member — identical on both sides, still not an npm shape
mkdir -p "$work/sym/package"
cp -r "$work/a/package/." "$work/sym/package/"
ln -s package.json "$work/sym/package/link"
tar -czf "$work/sym.tgz" -C "$work/sym" package
expect_reject "symlink member rejected even when identical" \
  bash "$TAR_VERIFY" "$work/sym.tgz" "$work/sym.tgz"

# hardlink member
mkdir -p "$work/hl/package"
cp -r "$work/a/package/." "$work/hl/package/"
ln "$work/hl/package/package.json" "$work/hl/package/package-copy.json"
tar -czf "$work/hl.tgz" -C "$work/hl" package
expect_reject "hardlink member rejected" \
  bash "$TAR_VERIFY" "$work/hl.tgz" "$work/hl.tgz"

# traversal member under package/ — passes the prefix check, must be
# caught by the .. component check (extract would refuse anyway)
mkdir -p "$work/dd/package"
cp -r "$work/a/package/." "$work/dd/package/"
tar -czf "$work/dd.tgz" -C "$work/dd" \
  --transform='s|^package/package.json$|package/../evil.json|' package 2>/dev/null
expect_reject "traversal member rejected" \
  bash "$TAR_VERIFY" "$work/dd.tgz" "$work/a.tgz"

# member outside package/ at ARCHIVE level (tree check also catches,
# but the archive gate must fire first)
mkdir -p "$work/outside/package"
cp -r "$work/a/package/." "$work/outside/package/"
printf 'x\n' > "$work/outside/stray"
tar -czf "$work/outside.tgz" -C "$work/outside" package stray
expect_reject "archive-level member outside package/ rejected" \
  bash "$TAR_VERIFY" "$work/outside.tgz" "$work/a.tgz"

# nonexistent inputs
expect_reject "missing published file rejected" \
  bash "$TAR_VERIFY" "$work/nope.tgz" "$work/a.tgz"
expect_reject "missing local file rejected" \
  bash "$TAR_VERIFY" "$work/a.tgz" "$work/nope.tgz"

# --- bin-format verifier ------------------------------------------------
expect_ok "bin verifier self-test passes" bash scripts/verify-npm-bin.sh --self-test

# e2e: +x text file rejected, +x script accepted, symlink rejected
printf 'plain text, no shebang\n' > "$work/textbin"
chmod +x "$work/textbin"
expect_reject "text file with exec bit rejected" \
  bash scripts/verify-npm-bin.sh "$work/textbin"
printf '#!/bin/sh\necho ok\n' > "$work/scriptbin"
chmod +x "$work/scriptbin"
expect_ok "shebang script accepted" \
  bash scripts/verify-npm-bin.sh "$work/scriptbin"
ln -s "$work/scriptbin" "$work/linkbin"
expect_reject "symlink bin rejected" \
  bash scripts/verify-npm-bin.sh "$work/linkbin"

# --- arch verifier ------------------------------------------------------
expect_ok "arch self-test passes" bash "$ARCH_VERIFY" --self-test

# End-to-end reject paths through the real entry point: FILE_BIN feeds
# fixture `file(1)` output so no cross-compiled binary is needed.
printf 'dummy\n' > "$work/dummy-bin"
cat > "$work/fake-file" <<'SH'
#!/usr/bin/env bash
case "$FAKE_FILE_OUT" in
  __fail__) exit 1 ;;
  __empty__) printf '' ;;
  *) printf '%s\n' "$FAKE_FILE_OUT" ;;
esac
SH
chmod +x "$work/fake-file"

arch_case() { # $1=desc $2=expected(ok|reject) $3=target $4=file-output|__fail__|__empty__
  local desc="$1" want="$2" target="$3" out="$4"
  local rc=0
  FAKE_FILE_OUT="$out" FILE_BIN="$work/fake-file" \
    bash "$ARCH_VERIFY" "$target" "$work/dummy-bin" >/dev/null 2>&1 || rc=$?
  if [ "$want" = ok ] && [ "$rc" -eq 0 ]; then say "ok: $desc"; return; fi
  if [ "$want" = reject ] && [ "$rc" -ne 0 ]; then say "ok: $desc"; return; fi
  fail "$desc (rc=$rc want=$want)"
}

arch_case "matching arch accepted"            ok      linux-x64    'ELF 64-bit LSB pie executable, x86-64'
arch_case "wrong arch rejected end-to-end"    reject  linux-arm64  'ELF 64-bit LSB pie executable, x86-64'
arch_case "Mach-O for ELF target rejected"    reject  linux-x64    'Mach-O 64-bit executable arm64'
arch_case "unknown target rejected"           reject  plan9-amd64  'ELF 64-bit LSB pie executable, x86-64'
arch_case "file(1) failure rejected"          reject  linux-x64    __fail__
arch_case "empty file(1) output rejected"     reject  linux-x64    __empty__
expect_reject "missing binary rejected" \
  bash "$ARCH_VERIFY" linux-x64 "$work/no-such-bin"

if [ "$fails" -gt 0 ]; then
  say "$fails release-verify test(s) FAILED"
  exit 1
fi
say "release-verify tests: all cases pass"

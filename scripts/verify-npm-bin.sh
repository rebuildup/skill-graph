#!/usr/bin/env bash
# verify-npm-bin.sh — prove a shipped bin entry is actually executable,
# not merely chmod +x. The permission bit alone accepts text files,
# scripts without a shebang, corrupt downloads, and wrong-format
# binaries that fail every install with ENOEXEC/ENOEXEC-format
# (#225 r5). `file(1)` output must classify the payload as an
# executable object (ELF/Mach-O) or a script — "ASCII text", "data",
# and empty files are rejected.
#
# Architecture matching is a separate concern handled by
# verify-npm-arch.sh where the target triple is known; this check is
# target-agnostic: it answers "can this file be exec'd at all".
#
# Usage:
#   verify-npm-bin.sh <file>
#   verify-npm-bin.sh --self-test
#
# FILE_BIN overrides the `file` binary (used by the self-test to feed
# fixture outputs without real binaries).
set -euo pipefail

# exec_ok <file-output> — pure predicate, no I/O. Accepts ELF
# executables/shared objects/pie, Mach-O binaries, and anything
# file(1) identifies as a script (shebang present). Rejects text,
# data, empty, and unclassified output.
exec_ok() {
  local out="$1"
  # file(1) appends "corrupted ..." to otherwise-executable-looking
  # descriptions; a truncated download must not pass on keywords alone.
  [[ "$out" == *corrupt* ]] && return 1
  [[ "$out" == *ELF* && ( "$out" == *executable* || "$out" == *"shared object"* || "$out" == *"pie executable"* ) ]] && return 0
  [[ "$out" == *Mach-O* ]] && return 0
  [[ "$out" == *script* ]] && return 0
  return 1
}

self_test() {
  local fails=0
  check() { # $1=desc $2=expected(0/1) $3=output
    local rc=0
    exec_ok "$3" || rc=$?
    if [ "$rc" -eq 0 ] && [ "$2" -eq 1 ]; then return; fi
    if [ "$rc" -ne 0 ] && [ "$2" -eq 0 ]; then return; fi
    echo "FAIL: $1 (rc=$rc want=$2)" >&2
    fails=$((fails + 1))
  }
  check "ELF pie executable accepted"        1 'ELF 64-bit LSB pie executable, x86-64, version 1 (SYSV), dynamically linked'
  check "ELF shared object accepted"         1 'ELF 64-bit LSB shared object, x86-64, version 1 (SYSV)'
  check "Mach-O executable accepted"         1 'Mach-O 64-bit executable arm64'
  check "env-node script accepted"           1 'a /usr/bin/env node script, ASCII text executable'
  check "shell script accepted"              1 'POSIX shell script, ASCII text executable'
  check "plain text REJECTED"                0 'ASCII text'
  check "ASCII text with exec word REJECTED" 0 'ASCII text, with very long lines'
  check "data REJECTED"                      0 'data'
  check "empty REJECTED"                     0 'empty'
  check "JSON REJECTED"                      0 'JSON text data'
  check "directory REJECTED"                 0 'directory'
  check "corrupted ELF REJECTED"             0 'ELF 64-bit LSB pie executable, x86-64, corrupted section header size'
  if [ "$fails" -gt 0 ]; then
    echo "$fails self-test case(s) failed" >&2
    exit 1
  fi
  echo "verify-npm-bin self-test: all cases pass"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit 0
fi

f="${1:?usage: verify-npm-bin.sh <file> | --self-test}"

# Symlinks are rejected outright: npm packs the link itself, and the
# installed bin would dangle or escape the package root.
[ -L "$f" ] && { echo "bin is a symlink: $f" >&2; exit 1; }
[ -f "$f" ] || { echo "bin not a regular file: $f" >&2; exit 1; }
[ -x "$f" ] || { echo "bin not executable (mode): $f" >&2; exit 1; }

file_out="$("${FILE_BIN:-file}" "$f" 2>/dev/null)" || {
  echo "cannot inspect $f (file command failed)" >&2
  exit 1
}
[ -n "$file_out" ] || { echo "empty file(1) output for $f" >&2; exit 1; }

exec_ok "$file_out" || {
  echo "bin is not an executable object: $f — $file_out" >&2
  exit 1
}

# Script-classified payloads must carry a REAL shebang the kernel can
# dispatch: "#!" followed by either `/usr/bin/env <tool>` or an
# absolute interpreter path. file(1) saying "script" is necessary but
# not sufficient — a `#!` with an empty or relative interpreter is
# classified as a script yet fails every exec with ENOEXEC (#225 r8).
if [[ "$file_out" == *script* ]]; then
  IFS= read -r shebang < "$f" || {
    echo "script bin has no readable first line: $f" >&2; exit 1; }
  [[ "$shebang" == '#!'* && ${#shebang} -gt 2 ]] || {
    echo "script bin lacks a valid shebang: $f" >&2; exit 1; }
  interp="${shebang:2}"
  interp="${interp%%[[:space:]]*}"
  rest="${shebang:2+${#interp}}"
  rest="${rest#"${rest%%[![:space:]]*}"}"
  if [[ "$interp" == */usr/bin/env || "$interp" == /bin/env ]]; then
    # env dispatch — a tool argument is REQUIRED: bare `#!/usr/bin/env`
    # execs env itself and never runs the script.
    [ -n "$rest" ] || {
      echo "script bin env shebang has no tool argument: $f — $shebang" >&2
      exit 1; }
  elif [[ "$interp" != /* ]]; then
    echo "script bin shebang interpreter is not absolute: $f — $shebang" >&2
    exit 1
  fi
fi
echo "$f: $file_out"

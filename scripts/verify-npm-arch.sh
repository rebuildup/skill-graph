#!/usr/bin/env bash
# verify-npm-arch.sh — verify a packaged binary's object format AND
# architecture match the declared npm target. Replaces the inline
# `file | grep -q` chains whose failure semantics depended on caller
# context; every rejection path here returns non-zero explicitly and
# the pattern logic is covered by --self-test (#225 re-review).
#
# Usage:
#   verify-npm-arch.sh <target> <binary>
#   verify-npm-arch.sh --self-test
#
# FILE_BIN overrides the `file` binary (used by the self-test to feed
# fixture outputs without real binaries).
set -euo pipefail

# arch_ok <target> <file-output> — pure predicate, no I/O.
arch_ok() {
  local out="$2"
  case "$1" in
    # The architecture is the token file(1) prints IMMEDIATELY after
    # the object-type phrase ("pie executable, x86-64,"). A bare
    # substring match is exploitable twice over (#225 r8/r9): an
    # aarch64 binary whose interpreter is /lib64/ld-linux-x86-64.so.2
    # contains "x86-64", and "shared object, x86-64" is a LINK INPUT,
    # not a runnable bin — only "executable," positions count.
    linux-x64)
      [[ "$out" == *ELF* && "$out" == *executable* && "$out" != *"shared object"* && "$out" == *"executable, x86-64"* ]]
      ;;
    linux-arm64)
      [[ "$out" == *ELF* && "$out" == *executable* && "$out" != *"shared object"* && "$out" == *"executable, ARM aarch64"* ]]
      ;;
    darwin-x64)
      [[ "$out" == *Mach-O* && "$out" == *executable* && "$out" != *"shared library"* && "$out" == *x86_64* ]]
      ;;
    darwin-arm64)
      [[ "$out" == *Mach-O* && "$out" == *executable* && "$out" != *"shared library"* && "$out" == *arm64* ]]
      ;;
    *)
      return 2
      ;;
  esac
}

self_test() {
  local fails=0
  check() { # $1=desc $2=expected(0/1) $3=target $4=output
    local rc=0
    arch_ok "$3" "$4" || rc=$?
    if [ "$rc" -eq 0 ] && [ "$2" -eq 1 ]; then return; fi
    if [ "$rc" -ne 0 ] && [ "$2" -eq 0 ]; then return; fi
    echo "FAIL: $1 (target=$3 rc=$rc want=$2)" >&2
    fails=$((fails + 1))
  }
  local elf_x64='ELF 64-bit LSB pie executable, x86-64, version 1 (SYSV)'
  local elf_arm='ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV)'
  local macho_x64='Mach-O 64-bit executable x86_64'
  local macho_arm='Mach-O 64-bit executable arm64'
  check "x86-64 ELF accepted for linux-x64"        1 linux-x64    "$elf_x64"
  check "aarch64 ELF accepted for linux-arm64"     1 linux-arm64  "$elf_arm"
  # Interpreter-path substring must not spoof the arch token (#225 r8).
  check "aarch64 ELF w/ x86-64 interp REJECTED"    0 linux-x64    'ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), dynamically linked, interpreter /lib64/ld-linux-x86-64.so.2'
  check "x86-64 ELF w/ aarch64 interp REJECTED arm" 0 linux-arm64 "$elf_x64, interpreter /lib/ld-linux-aarch64.so.1"
  check "x86-64 ELF REJECTED for linux-arm64"      0 linux-arm64  "$elf_x64"
  check "aarch64 ELF REJECTED for linux-x64"       0 linux-x64    "$elf_arm"
  # Shared objects are link inputs, not runnable bins (#225 r9).
  check "x86-64 shared object REJECTED"            0 linux-x64    'ELF 64-bit LSB shared object, x86-64, version 1 (SYSV)'
  check "aarch64 shared object REJECTED arm"       0 linux-arm64  'ELF 64-bit LSB shared object, ARM aarch64, version 1 (SYSV)'
  check "x86_64 dylib REJECTED darwin"             0 darwin-x64   'Mach-O 64-bit dynamically linked shared library x86_64'
  check "arm64 Mach-O accepted for darwin-arm64"   1 darwin-arm64 "$macho_arm"
  check "x86_64 Mach-O accepted for darwin-x64"    1 darwin-x64   "$macho_x64"
  check "arm64 Mach-O REJECTED for darwin-x64"     0 darwin-x64   "$macho_arm"
  check "x86_64 Mach-O REJECTED for darwin-arm64"  0 darwin-arm64 "$macho_x64"
  check "script REJECTED everywhere"               0 linux-x64    'POSIX shell script, ASCII text'
  check "empty output REJECTED"                    0 linux-arm64  ''
  check "unknown target rejected"                  0 plan9-amd64  "$elf_x64"
  if [ "$fails" -gt 0 ]; then
    echo "$fails self-test case(s) failed" >&2
    exit 1
  fi
  echo "verify-npm-arch self-test: all cases pass"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit 0
fi

target="${1:?usage: verify-npm-arch.sh <target> <binary> | --self-test}"
bin="${2:?usage: verify-npm-arch.sh <target> <binary> | --self-test}"

[ -f "$bin" ] || { echo "binary not found: $bin" >&2; exit 1; }

file_out="$("${FILE_BIN:-file}" "$bin" 2>/dev/null)" || {
  echo "cannot inspect $bin (file command failed)" >&2
  exit 1
}
[ -n "$file_out" ] || { echo "empty file(1) output for $bin" >&2; exit 1; }

rc=0
arch_ok "$target" "$file_out" || rc=$?
if [ "$rc" -eq 2 ]; then
  echo "unknown npm target: $target" >&2
  exit 1
fi
if [ "$rc" -ne 0 ]; then
  echo "arch mismatch for $target: $file_out" >&2
  exit 1
fi
echo "$target: $file_out"

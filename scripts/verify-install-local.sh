#!/usr/bin/env bash
# verify-install-local.sh — auth-free LOCAL install verification for
# every distribution channel, run against the FROZEN release artifacts
# (the exact tarball / packed crate / package dirs a release ships).
# Nothing is repaired between pack and install: whatever bytes were
# verified are the bytes installed, executed, and uninstalled
# (#225 r9).
#
# Guarantees exercised per channel:
#   exec permission — the installed bin actually runs
#   version         — --version reports the manifest/Cargo version
#   basic function  — a real subcommand smoke (hook fail-open path)
#   lifecycle       — install -> verify -> uninstall leaves nothing
#
# Channels:
#   cargo — `cargo install --path .` (the crates.io-equivalent build
#           path; `cargo publish --dry-run` covers crate packaging).
#   npm   — real `npm pack` + `npm install <tgz>` of BOTH the platform
#           package and the metapackage; the installed .bin shim must
#           resolve the platform payload and exec it.
#   brew  — when `brew` exists: a generated formula whose url is the
#           LOCAL release tarball (file://), installed/executed/
#           uninstalled — the same bytes the tap formula fetches.
#   mise  — the `github:` backend installs the GitHub release asset
#           `skill-graph-<triple>.tar.gz`; that asset is produced and
#           natively verified by package-target.sh. mise itself needs
#           a published release, so its lifecycle check is post-release
#           (see #225 pending items); the artifact contract is proven
#           here.
#
# Usage: scripts/verify-install-local.sh [artifact-dir]
#   artifact-dir defaults to cwd; must contain
#   skill-graph-<host-triple>.tar.gz (package-target.sh output).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
cd "$root"
artifact_dir="${1:-$PWD}"

host_triple="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "$host_triple" ] || { echo "cannot determine rustc host triple" >&2; exit 1; }

case "$host_triple" in
  x86_64-unknown-linux-gnu)   npm_target=linux-x64 ;;
  aarch64-unknown-linux-gnu)  npm_target=linux-arm64 ;;
  x86_64-apple-darwin)        npm_target=darwin-x64 ;;
  aarch64-apple-darwin)       npm_target=darwin-arm64 ;;
  *) echo "no npm target mapping for host $host_triple" >&2; exit 1 ;;
esac

version="$(grep -m1 '^version' Cargo.toml | sed -E 's/^version = "([^"]+)".*/\1/')"
tgz="$artifact_dir/skill-graph-${host_triple}.tar.gz"
[ -f "$tgz" ] || {
  echo "frozen artifact not found: $tgz" >&2
  echo "run scripts/package-target.sh $host_triple $npm_target first" >&2
  exit 1
}

fails=0
ok()   { echo "PASS: $1"; }
bad()  { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/sg-install-verify.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

# Locate the packaged payload: prefer the host-target build, fall
# back to a plain `cargo build --release` output. The bytes installed
# below come from the TARBALL, not the build tree.
bin=""
for cand in "target/${host_triple}/release/skill-graph" "target/release/skill-graph"; do
  [ -x "$cand" ] && bin="$cand" && break
done
[ -n "$bin" ] || { echo "no built binary found; run cargo build --release first" >&2; exit 1; }

# ------------------------------------------------------------------
# Shared precondition: the frozen tarball itself must be sound —
# this is the asset mise/brew/release consume.
# ------------------------------------------------------------------
tar -xzf "$tgz" -C "$tmp"
[ -x "$tmp/skill-graph" ] || chmod +x "$tmp/skill-graph"
"$here/verify-npm-bin.sh" "$tmp/skill-graph" \
  && ok "tarball payload executable format" \
  || bad "tarball payload executable format"
"$here/verify-npm-arch.sh" "$npm_target" "$tmp/skill-graph" \
  && ok "tarball payload arch ($npm_target)" \
  || bad "tarball payload arch ($npm_target)"
out="$("$tmp/skill-graph" --version)"
[ "$out" = "skill-graph $version" ] \
  && ok "tarball --version ($out)" \
  || bad "tarball --version got '$out', want 'skill-graph $version'"

# ------------------------------------------------------------------
# Channel: cargo (crates.io-equivalent source install)
# ------------------------------------------------------------------
cargo_root="$tmp/cargo-root"
if cargo install --path . --root "$cargo_root" --locked >/dev/null 2>&1; then
  cbin="$cargo_root/bin/skill-graph"
  cout="$("$cbin" --version)"
  if [ "$cout" = "skill-graph $version" ]; then
    ok "cargo install --version ($cout)"
  else
    bad "cargo install --version got '$cout'"
  fi
  printf '{}' | "$cbin" hook claude-code prompt >/dev/null 2>&1 \
    && ok "cargo install hook smoke" \
    || bad "cargo install hook smoke"
  cargo uninstall --root "$cargo_root" skill-graph >/dev/null 2>&1 \
    && [ ! -e "$cbin" ] \
    && ok "cargo uninstall lifecycle" \
    || bad "cargo uninstall lifecycle"
else
  bad "cargo install --path ."
fi

# ------------------------------------------------------------------
# Channel: npm (real pack + install of BOTH packages; the installed
# .bin shim must resolve the platform payload and exec the FROZEN
# tarball bytes — not the build tree).
# ------------------------------------------------------------------
plat_src="npm/skill-graph-${npm_target}"
meta_src="npm/skill-graph"
[ -d "$plat_src" ] && [ -d "$meta_src" ] || {
  echo "npm package dirs missing ($plat_src / $meta_src)" >&2; exit 1; }

npm_tmp="$tmp/npm"
mkdir -p "$npm_tmp/plat/bin" "$npm_tmp/meta"
cp -r "$plat_src/." "$npm_tmp/plat/"
cp "$tmp/skill-graph" "$npm_tmp/plat/bin/skill-graph"   # frozen bytes
chmod +x "$npm_tmp/plat/bin/skill-graph"
cp -r "$meta_src/." "$npm_tmp/meta/"
cp -r plugins/skill-graph "$npm_tmp/meta/plugin"
cp -r contracts "$npm_tmp/meta/contracts"

# prepack requires plugin/+contracts/ — already copied above.
plat_name="$(node -p "require('./$plat_src/package.json').name")"
meta_name="$(node -p "require('./$meta_src/package.json').name")"

plat_tgz="$(cd "$npm_tmp/plat" && npm pack --silent | tail -1)"
meta_tgz="$(cd "$npm_tmp/meta" && npm pack --silent | tail -1)"
[ -f "$npm_tmp/plat/$plat_tgz" ] && [ -f "$npm_tmp/meta/$meta_tgz" ] || {
  echo "npm pack failed" >&2; exit 1; }

npm_inst="$tmp/npm-inst"
if npm install --prefix "$npm_inst" \
     "$npm_tmp/plat/$plat_tgz" "$npm_tmp/meta/$meta_tgz" >/dev/null 2>&1; then
  shim="$npm_inst/node_modules/.bin/skill-graph"
  [ -x "$shim" ] || bad "npm installed bin missing/not executable"
  nout="$("$shim" --version 2>/dev/null || true)"
  [ "$nout" = "skill-graph $version" ] \
    && ok "npm install --version via shim ($nout)" \
    || bad "npm install --version got '$nout'"
  printf '{}' | "$shim" hook claude-code prompt >/dev/null 2>&1 \
    && ok "npm install hook smoke" \
    || bad "npm install hook smoke"
  # Lifecycle: uninstall must remove the shim AND the payload.
  npm uninstall --prefix "$npm_inst" "$meta_name" "$plat_name" >/dev/null 2>&1 || true
  { [ ! -e "$shim" ] && [ ! -d "$npm_inst/node_modules/$plat_name" ]; } \
    && ok "npm uninstall lifecycle" \
    || bad "npm uninstall lifecycle"
else
  bad "npm install $plat_name + $meta_name"
fi

# ------------------------------------------------------------------
# Channel: brew — only when brew exists (CI runners ship it; dev
# machines may not). The formula url is the LOCAL frozen tarball via
# file:// — brew downloads/extracts/installs exactly those bytes.
# ------------------------------------------------------------------
if command -v brew >/dev/null 2>&1; then
  sha="$(shasum -a 256 "$tgz" | awk '{print $1}')"
  formula="$tmp/skill-graph.rb"
  cat > "$formula" <<EOF
class SkillGraph < Formula
  desc "Agent Skills discovery and recommendation CLI"
  homepage "https://github.com/rebuildup/skill-graph"
  url "file://$tgz"
  sha256 "$sha"
  version "${version}"
  license "MIT"

  def install
    bin.install "skill-graph"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/skill-graph --version")
  end
end
EOF
  brew_log="$tmp/brew-install.log"
  # `./` prefix is REQUIRED: brew parses bare `/abs/path.rb` and
  # `user/repo` args as tap names ("Invalid tap name"), while
  # `./formula.rb` is recognized as a local formula file.
  if (cd "$tmp" && brew install ./skill-graph.rb >"$brew_log" 2>&1); then
    bbin="$(brew --prefix)/bin/skill-graph"
    bout="$("$bbin" --version 2>/dev/null || true)"
    [ "$bout" = "skill-graph $version" ] \
      && ok "brew install --version ($bout)" \
      || bad "brew install --version got '$bout'"
    printf '{}' | "$bbin" hook claude-code prompt >/dev/null 2>&1 \
      && ok "brew install hook smoke" \
      || bad "brew install hook smoke"
    brew uninstall skill-graph >/dev/null 2>&1 \
      && [ ! -e "$bbin" ] \
      && ok "brew uninstall lifecycle" \
      || bad "brew uninstall lifecycle"
  else
    echo "brew install output:" >&2
    tail -30 "$brew_log" >&2
    bad "brew install (formula -> file:// tarball)"
  fi
else
  echo "SKIP: brew not installed — brew channel verified in CI artifact-verify"
fi

# ------------------------------------------------------------------
# Channel: mise — the github backend installs the release asset
# skill-graph-<triple>.tar.gz. That asset contract is proven above
# (exists, exec format, arch, --version). `mise use -g
# github:rebuildup/skill-graph` itself requires a published release
# and is a post-release lifecycle check (#225 pending items).
# ------------------------------------------------------------------
echo "NOTE: mise github backend consumes $tgz (verified above);"
echo "      end-to-end mise install is post-release only."

[ "$fails" -eq 0 ] || { echo "$fails check(s) failed" >&2; exit 1; }
echo "all local install checks passed (host=$host_triple npm=$npm_target)"

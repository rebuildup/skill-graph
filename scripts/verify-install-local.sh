#!/usr/bin/env bash
# verify-install-local.sh — auth-free LOCAL install verification for
# every distribution channel, run against the FROZEN release artifacts
# (the exact tarball / packed crate / package dirs a release ships).
# Nothing is repaired between pack and install: whatever bytes were
# verified are the bytes installed, executed, and uninstalled
# (#225 r9).
#
# Guarantees exercised per channel:
#   exec permission — the installed bin actually runs (exec bit comes
#                     from the artifact itself; nothing is chmod'ed)
#   version         — --version reports the manifest/Cargo version
#   basic function  — hook claude-code prompt must emit the
#                     documented additionalContext injection for a
#                     deterministic prompt (not merely exit 0)
#   lifecycle       — install -> verify -> uninstall leaves nothing
#   artifact bind   — cargo installs the PACKED .crate; npm installs
#                     real npm-pack tarballs of both packages; brew
#                     installs the frozen release tarball via a local
#                     tap. What is verified IS what the channel ships.
#
# Channels:
#   cargo — `cargo package` -> extract .crate -> `cargo install
#           --path` on the extracted packaged sources: the crates.io
#           channel installs exactly these bytes (`cargo publish
#           --dry-run` in CI covers the publish-side pack).
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

# hook_smoke <bin> <session-id> — basic FUNCTION proof, not just
# fail-open exit 0: feed a deterministic prompt and require the
# documented additionalContext injection contract in stdout
# (#225 r10 — an empty-JSON smoke proves nothing about whether the
# binary can actually recommend).
hook_smoke() {
  local bin="$1" sess="$2" out
  out="$(printf '%s' \
    "{\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"please review this pull request carefully\",\"session_id\":\"$sess\"}" \
    | "$bin" hook claude-code prompt 2>/dev/null)" || return 1
  printf '%s' "$out" | grep -q '"additionalContext":"skill-graph: skills relevant to this task'
}

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
# NO repair: the frozen tarball must carry the exec bit itself —
# tar preserves modes, so a missing +x is a defect to REPORT, not
# to chmod away (#225 r10).
[ -x "$tmp/skill-graph" ] || bad "tarball payload missing exec bit"
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
# Channel: cargo — install the PACKAGED CRATE, not the source tree:
# `cargo package` produces the exact .crate `cargo publish` would
# upload; extracting it and `cargo install --path`-ing the extracted
# sources proves the published artifact compiles, installs, runs,
# and uninstalls (#225 r10 — path install alone never touched the
# packaged file set).
# ------------------------------------------------------------------
cargo_root="$tmp/cargo-root"
crate_dir="$tmp/crate"
if cargo package --allow-dirty --locked >/dev/null 2>&1; then
  crate="$(ls -t target/package/skill-graph-*.crate 2>/dev/null | head -1)"
  if [ -n "$crate" ] && mkdir -p "$crate_dir" \
     && tar -xzf "$crate" -C "$crate_dir"; then
    pkg_dir="$crate_dir/skill-graph-$version"
    if [ -n "$pkg_dir" ] \
       && cargo install --path "$pkg_dir" --root "$cargo_root" --locked >/dev/null 2>&1; then
      cbin="$cargo_root/bin/skill-graph"
      cout="$("$cbin" --version)"
      if [ "$cout" = "skill-graph $version" ]; then
        ok "crate install --version ($cout)"
      else
        bad "crate install --version got '$cout'"
      fi
      hook_smoke "$cbin" "verify-crate-$$-$RANDOM" \
        && ok "crate install hook output" \
        || bad "crate install hook output"
      cargo uninstall --root "$cargo_root" skill-graph >/dev/null 2>&1 \
        && [ ! -e "$cbin" ] \
        && ok "crate uninstall lifecycle" \
        || bad "crate uninstall lifecycle"
    else
      bad "cargo install from packaged .crate"
    fi
  else
    bad "cargo package / crate extract"
  fi
else
  bad "cargo package --allow-dirty"
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
# cp -p preserves the tarball's exec bit EXACTLY — no chmod repair:
# if the frozen payload lacked +x the installed bin would fail to
# exec and this check would catch it (#225 r10).
cp -p "$tmp/skill-graph" "$npm_tmp/plat/bin/skill-graph"
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
  hook_smoke "$shim" "verify-npm-$$-$RANDOM" \
    && ok "npm install hook output" \
    || bad "npm install hook output"
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
  # Homebrew >= 7.0.14 refuses to load formulae from file paths
  # (forbid_packages_from_paths is unconditional). The supported
  # local equivalent of `brew install rebuildup/tap/skill-graph` is
  # a LOCAL TAP: a git repo containing Formula/skill-graph.rb that
  # `brew tap` clones — the same tap-qualified install path the real
  # channel uses, against the frozen tarball via file:// (#225 r9).
  tap_repo="$tmp/homebrew-sg-tap"
  mkdir -p "$tap_repo/Formula"
  cat > "$tap_repo/Formula/skill-graph.rb" <<EOF
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
  git -C "$tap_repo" init -q
  git -C "$tap_repo" add -A
  git -C "$tap_repo" -c user.email=ci@localhost -c user.name=ci commit -qm init
  tap_name="sg-verify/skill-graph-tap"
  brew_log="$tmp/brew-install.log"
  brew_untap=0
  if brew tap "$tap_name" "$tap_repo" >>"$brew_log" 2>&1; then
    brew_untap=1
    # NO_AUTO_UPDATE: the tap's "remote" is a local path; an update
    # fetch is meaningless and could fail offline.
    if HOMEBREW_NO_AUTO_UPDATE=1 brew install "$tap_name/skill-graph" >>"$brew_log" 2>&1; then
      bbin="$(brew --prefix)/bin/skill-graph"
      bout="$("$bbin" --version 2>/dev/null || true)"
      [ "$bout" = "skill-graph $version" ] \
        && ok "brew install --version ($bout)" \
        || bad "brew install --version got '$bout'"
      hook_smoke "$bbin" "verify-brew-$$-$RANDOM" \
        && ok "brew install hook output" \
        || bad "brew install hook output"
      HOMEBREW_NO_AUTO_UPDATE=1 brew uninstall "$tap_name/skill-graph" >/dev/null 2>&1 \
        && [ ! -e "$bbin" ] \
        && ok "brew uninstall lifecycle" \
        || bad "brew uninstall lifecycle"
    else
      echo "brew install output:" >&2
      tail -30 "$brew_log" >&2
      bad "brew install (local tap -> file:// tarball)"
    fi
  else
    echo "brew tap output:" >&2
    tail -30 "$brew_log" >&2
    bad "brew tap local repo"
  fi
  [ "$brew_untap" -eq 1 ] && brew untap "$tap_name" >/dev/null 2>&1 || true
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

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
#                     deterministic CORPUS of prompts with pinned
#                     expected skill lists (not merely exit 0, and not
#                     a single anecdotal prompt — #225 r11)
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

# hook_corpus <bin> <session-prefix> — basic FUNCTION proof on a
# deterministic corpus, not fail-open exit 0 and not one anecdotal
# prompt (#225 r10/r11): each prompt must produce the EXACT
# additionalContext skill list the engine recommends for it against
# THIS repository's skill set. Expectations are pinned on purpose —
# a change to repo skills or ranking that alters recommendations must
# update this corpus deliberately, which is exactly the signal a
# release gate should give. The corpus covers multiple distinct
# recommendation outcomes so a binary that always emits the same
# context (or none) cannot pass.
hook_corpus() {
  local bin="$1" sess="$2" i=0 out expected fails=0 prompt want
  # Corpus isolation (#225 F04): pin the skill roots to THIS repo's
  # canonical skill dir (.agents/skills — .claude/.devin entries are
  # symlinks into it) so HOME-level or runtime-specific skills cannot
  # change the expected recommendation lists. Without this the same
  # binary could pass or fail depending on the runner's HOME.
  export SKILL_GRAPH_ROOTS="$root/.agents/skills"
  while IFS=$'\t' read -r prompt want; do
    [ -n "$prompt" ] || continue
    i=$((i + 1))
    out="$(printf '%s' \
      "{\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"$prompt\",\"session_id\":\"$sess-$i\"}" \
      | "$bin" hook claude-code prompt 2>/dev/null)" || {
      echo "corpus[$i] hook invocation failed: $prompt" >&2
      fails=$((fails + 1)); continue; }
    expected="\"additionalContext\":\"skill-graph: skills relevant to this task — $want. Read the matching SKILL.md before relying on it.\""
    if printf '%s' "$out" | grep -qF "$expected"; then
      continue
    fi
    echo "corpus[$i] mismatch for prompt '$prompt'" >&2
    echo "  want substring: $expected" >&2
    echo "  got: $out" >&2
    fails=$((fails + 1))
  done <<'CORPUS'
please review this pull request carefully	code-review, github-delivery, writing-discipline
improve this rust code quality	system-quality, code-naming, quality-gate
check for security vulnerabilities	security-maintenance
create a new agent skill	skill-selection, system-quality
write documentation for the new API	onboarding
CORPUS
  [ "$fails" -eq 0 ]
}

# interface_smoke <bin> — every PUBLIC interface on the installed
# artifact, not just the hook path (#225 F03): discover/resolve/
# recommend/deps JSON contracts, the MCP stdio server, and adapter
# status must all work from the packaged bytes. Roots are pinned to
# the repo's canonical skills (same isolation as hook_corpus).
interface_smoke() {
  local bin="$1" i=0 fails=0
  export SKILL_GRAPH_ROOTS="$root/.agents/skills"
  i=$((i + 1))
  "$bin" discover --format json 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin)
assert isinstance(d.get("skills"),list) and d["skills"], "empty skills"
s=d["skills"][0]
for k in ("name","description","path","root","digest","availability","spec_violations"):
    assert k in s, f"missing {k}"
' || { echo "interface[$i] discover contract failed" >&2; fails=$((fails + 1)); }

  i=$((i + 1))
  "$bin" resolve code-review --format json 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin)
assert d.get("kind") in ("name","path"), d
' || { echo "interface[$i] resolve failed" >&2; fails=$((fails + 1)); }

  i=$((i + 1))
  "$bin" recommend "please review this pull request" --format json 2>/dev/null \
    | python3 -c '
import json,sys
d=json.load(sys.stdin)
recs=d.get("recommendations") if isinstance(d,dict) else d
assert isinstance(recs,list) and recs, "no recommendations"
assert any(r["skill"]["name"]=="code-review" for r in recs), [r["skill"]["name"] for r in recs]
' || { echo "interface[$i] recommend failed" >&2; fails=$((fails + 1)); }

  i=$((i + 1))
  "$bin" deps code-review --format json 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin)
assert isinstance(d,list), d
' || { echo "interface[$i] deps failed" >&2; fails=$((fails + 1)); }

  i=$((i + 1))
  printf '%s\n%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"verify","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
    | "$bin" mcp 2>/dev/null | python3 -c '
import json,sys
tools=None
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    m=json.loads(line)
    r=m.get("result") or {}
    if isinstance(r.get("tools"),list): tools={t["name"] for t in r["tools"]}
assert tools and {"recommend","discover","resolve"} <= tools, tools
' || { echo "interface[$i] mcp tools/list failed" >&2; fails=$((fails + 1)); }

  i=$((i + 1))
  "$bin" adapter status claude-code >/dev/null 2>&1 \
    || { echo "interface[$i] adapter status failed" >&2; fails=$((fails + 1)); }

  i=$((i + 1))
  obs_dir="$tmp/obs-empty-$$"
  mkdir -p "$obs_dir"
  "$bin" observe "$obs_dir" --summary 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin)
for k in ("sessions","recommendations","emitted_recommendations","delivery_rate_pct","events"):
    assert k in d, f"missing {k}"
' || { echo "interface[$i] observe --summary failed" >&2; fails=$((fails + 1)); }

  i=$((i + 1))
  "$bin" resolve definitely-not-a-skill --format json 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin)
assert d.get("kind")=="unresolved", d
' || { echo "interface[$i] resolve unresolved contract failed" >&2; fails=$((fails + 1)); }

  # adapter lifecycle in an isolated HOME: enable writes config,
  # status reports it, disable removes the managed hook — the full
  # install/modify/remove cycle on the packaged binary (#225 F03).
  i=$((i + 1))
  iso_home="$tmp/iso-home-$$"
  mkdir -p "$iso_home"
  if HOME="$iso_home" "$bin" adapter enable claude-code --user >/dev/null 2>&1 \
     && HOME="$iso_home" "$bin" adapter status claude-code --user >/dev/null 2>&1 \
     && [ -s "$iso_home/.claude/settings.json" ]; then
    if HOME="$iso_home" "$bin" adapter disable claude-code --user >/dev/null 2>&1; then
      # disable removes the managed hook; when nothing else remains
      # it may delete settings.json entirely — both are clean states.
      if [ -f "$iso_home/.claude/settings.json" ]; then
        python3 -c '
import json,sys
s=json.load(open(sys.argv[1]))
hooks=json.dumps(s.get("hooks",{}))
assert "skill-graph" not in hooks, hooks
' "$iso_home/.claude/settings.json" \
          && ok "interface[$i] adapter enable/status/disable lifecycle" \
          || { echo "interface[$i] adapter disable left managed hook" >&2; fails=$((fails + 1)); }
      else
        ok "interface[$i] adapter enable/status/disable lifecycle"
      fi
    else
      echo "interface[$i] adapter disable failed" >&2; fails=$((fails + 1));
    fi
  else
    echo "interface[$i] adapter enable/status failed" >&2; fails=$((fails + 1));
  fi

  [ "$fails" -eq 0 ]
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
      hook_corpus "$cbin" "verify-crate-$$-$RANDOM" \
        && ok "crate install hook corpus (5 prompts)" \
        || bad "crate install hook output"
      interface_smoke "$cbin" \
        && ok "crate install public interfaces (discover/resolve/recommend/deps/mcp/adapter)" \
        || bad "crate install interface smoke"
      # Upgrade path: reinstalling over an existing install must
      # succeed and leave a working binary (#225 F03).
      if cargo install --path "$pkg_dir" --root "$cargo_root" --locked >/dev/null 2>&1 \
         && [ "$("$cbin" --version)" = "skill-graph $version" ]; then
        ok "crate reinstall (upgrade path)"
      else
        bad "crate reinstall (upgrade path)"
      fi
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
  hook_corpus "$shim" "verify-npm-$$-$RANDOM" \
    && ok "npm install hook corpus (5 prompts)" \
    || bad "npm install hook output"
  interface_smoke "$shim" \
    && ok "npm install public interfaces" \
    || bad "npm install interface smoke"
  # Upgrade path: reinstalling the same tgz set over the existing
  # install must succeed and keep the shim working (#225 F03).
  if npm install --prefix "$npm_inst" \
       "$npm_tmp/plat/$plat_tgz" "$npm_tmp/meta/$meta_tgz" >/dev/null 2>&1 \
     && [ "$("$shim" --version 2>/dev/null)" = "skill-graph $version" ]; then
    ok "npm reinstall (upgrade path)"
  else
    bad "npm reinstall (upgrade path)"
  fi
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
      hook_corpus "$bbin" "verify-brew-$$-$RANDOM" \
        && ok "brew install hook corpus (5 prompts)" \
        || bad "brew install hook output"
      interface_smoke "$bbin" \
        && ok "brew install public interfaces" \
        || bad "brew install interface smoke"
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

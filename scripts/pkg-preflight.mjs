#!/usr/bin/env node
// pkg-preflight.mjs — structured package.json preflight for
// publish-npm.sh. Replaces ad-hoc sed/awk string extraction with a
// real JSON parser: manifest shape, files[] payload presence, and
// publish-order constraints are all validated here, BEFORE any
// `npm pack`/`npm publish` runs (#225 r6).
//
// Usage: pkg-preflight.mjs <package-dir> [<package-dir>...]
//
// stdout: one "NAME\tVERSION\tFILES_JSON" line per package, in
// argument order — publish-npm.sh pairs these with the packed
// artifacts and verifies the artifact actually contains every
// declared files[] payload (#225 r7). FILES_JSON is compact JSON so
// it cannot contain a literal tab.
// stderr: one violation per line; exit 1 if ANY check fails.
//
// Node is a hard requirement by construction: npm itself is a Node
// program, so every environment that can run `npm publish` can run
// this script.
import { readFileSync, statSync, readdirSync } from "node:fs";
import { join, isAbsolute } from "node:path";

const dirs = process.argv.slice(2);
if (dirs.length === 0) {
  console.error("usage: pkg-preflight.mjs <package-dir>...");
  process.exit(1);
}

const fail = (msg) => {
  console.error(`preflight: ${msg}`);
  process.exit(1);
};

const pkgs = dirs.map((dir, i) => {
  const manifestPath = join(dir, "package.json");
  let m;
  try {
    m = JSON.parse(readFileSync(manifestPath, "utf8"));
  } catch (e) {
    fail(`cannot parse ${manifestPath}: ${e.message}`);
  }
  if (m === null || typeof m !== "object" || Array.isArray(m)) {
    fail(`${manifestPath} is not a JSON object`);
  }
  const { name, version } = m;
  if (typeof name !== "string" || name === "") {
    fail(`${manifestPath}: missing or invalid "name"`);
  }
  if (typeof version !== "string" || version === "") {
    fail(`${manifestPath}: missing or invalid "version"`);
  }

  // files[]: every declared payload path must exist on disk before
  // pack — npm silently drops missing entries, shipping a broken
  // package that only fails at install time (#225 r5). Entries must
  // be relative paths inside the package: absolute paths and ".."
  // segments would escape the packed root.
  if (m.files !== undefined && !Array.isArray(m.files)) {
    fail(`${manifestPath}: "files" is not an array`);
  }
  for (const f of m.files ?? []) {
    if (typeof f !== "string" || f === "" || isAbsolute(f) || f.split("/").includes("..")) {
      fail(`${manifestPath}: invalid files[] entry ${JSON.stringify(f)}`);
    }
    const p = join(dir, f);
    let st;
    try {
      st = statSync(p);
    } catch {
      fail(`required file missing: ${dir}/${f}`);
    }
    if (st.isDirectory() && readdirSync(p).length === 0) {
      fail(`required directory is empty: ${dir}/${f}`);
    }
    if (!st.isDirectory() && !st.isFile()) {
      fail(`required path is not a file or directory: ${dir}/${f}`);
    }
  }

  // bin field: string (single bin named after the package) or an
  // object mapping bin names to payload paths. Every declared target
  // is a REQUIRED executable payload — npm packs it whether or not
  // files[] covers it, and a missing/non-executable target ships a
  // broken install (#225 r8). Targets must be relative paths inside
  // the package, exist on disk as regular files, and carry the exec
  // bit; executable FORMAT is verified by verify-npm-bin.sh in the
  // publish path (it needs file(1)).
  const binTargets = [];
  if (m.bin !== undefined) {
    const entries =
      typeof m.bin === "string" ? [[m.name, m.bin]] : Object.entries(m.bin ?? {});
    if (typeof m.bin !== "string" && (m.bin === null || typeof m.bin !== "object" || Array.isArray(m.bin))) {
      fail(`${manifestPath}: "bin" is not a string or object`);
    }
    for (const [binName, target] of entries) {
      if (typeof binName !== "string" || binName === "" || typeof target !== "string" || target === "") {
        fail(`${manifestPath}: invalid bin entry ${JSON.stringify([binName, target])}`);
      }
      if (isAbsolute(target) || target.split("/").includes("..")) {
        fail(`${manifestPath}: bin target escapes package: ${target}`);
      }
      // bin/ confinement: verify-npm-archive.sh --check-bin inspects
      // every package/bin/* member of the artifact — a target outside
      // bin/ would escape executable-format verification inside the
      // published bytes (#225 r8).
      if (!target.startsWith("bin/")) {
        fail(`${manifestPath}: bin target must live under bin/: ${target}`);
      }
      const p = join(dir, target);
      let st;
      try {
        st = statSync(p);
      } catch {
        fail(`required bin target missing: ${dir}/${target}`);
      }
      if (!st.isFile()) {
        fail(`bin target is not a regular file: ${dir}/${target}`);
      }
      if ((st.mode & 0o111) === 0) {
        fail(`bin target lacks exec bit: ${dir}/${target}`);
      }
      binTargets.push(target);
    }
  }

  // Publish-order graph: dependencies + optionalDependencies names.
  // dev/peer deps do not constrain publish order — consumers never
  // install them.
  const deps = new Set();
  for (const key of ["dependencies", "optionalDependencies"]) {
    const block = m[key];
    if (block === undefined) continue;
    if (block === null || typeof block !== "object" || Array.isArray(block)) {
      fail(`${manifestPath}: "${key}" is not an object`);
    }
    for (const dep of Object.keys(block)) deps.add(dep);
  }
  return { dir, name, version, deps, files: m.files ?? [], binTargets };
});

// Order invariant: when two packages in this invocation are related
// by dependencies/optionalDependencies, the dependency must be
// published FIRST — a metapackage published before its platform
// packages would resolve to missing/broken optional deps for early
// adopters (#225 r5).
for (let i = 0; i < pkgs.length; i++) {
  for (const dep of pkgs[i].deps) {
    const j = pkgs.findIndex((p) => p.name === dep);
    if (j > i) {
      fail(
        `publish order violation: ${pkgs[i].dir} (${pkgs[i].name}) depends on ` +
          `${dep} but ${pkgs[j].dir} comes later`,
      );
    }
  }
}

for (const p of pkgs) {
  // FILES_JSON and BIN_JSON are compact JSON — no literal tabs.
  console.log(
    `${p.name}\t${p.version}\t${JSON.stringify(p.files)}\t${JSON.stringify(p.binTargets)}`,
  );
}

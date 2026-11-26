#!/usr/bin/env node
"use strict";

// Resolves the platform-specific optional dependency and execs the native
// binary shipped inside it. No postinstall scripts, no downloads.

const { spawnSync } = require("node:child_process");
const path = require("node:path");

function fail(message) {
  process.stderr.write(`skill-graph: ${message}\n`);
  process.exit(1);
}

// Derive the platform -> package mapping from this package's own
// optionalDependencies so the shim and package.json can never drift apart.
const { optionalDependencies = {} } = require("../package.json");
const PLATFORM_PACKAGES = {};
for (const dep of Object.keys(optionalDependencies)) {
  const idx = dep.lastIndexOf("skill-graph-");
  if (idx === -1) continue;
  PLATFORM_PACKAGES[dep.slice(idx + "skill-graph-".length)] = dep;
}

function isMusl() {
  try {
    const report = process.report && process.report.getReport();
    return !(report && report.header && report.header.glibcVersionRuntime);
  } catch {
    return false;
  }
}

const key = `${process.platform}-${process.arch}`;
let pkgName = PLATFORM_PACKAGES[key];
if (pkgName && process.platform === "linux" && isMusl()) {
  pkgName = undefined; // glibc-linked binary cannot run on musl
}
if (!pkgName) {
  fail(
    `unsupported platform ${key}${process.platform === "linux" && isMusl() ? " (musl detected)" : ""}. ` +
      `Prebuilt binaries are available for: ${Object.keys(PLATFORM_PACKAGES).join(", ")}. ` +
      `Install from source with: cargo install --git https://github.com/rebuildup/skill-graph`,
  );
}

let pkgJson;
try {
  pkgJson = require.resolve(`${pkgName}/package.json`);
} catch {
  fail(
    `platform package ${pkgName} is not installed. ` +
      `Reinstall @rebuildup/skill-graph (npm install -g @rebuildup/skill-graph).`,
  );
}

const bin = path.join(path.dirname(pkgJson), "bin", "skill-graph");
const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  fail(`failed to execute ${bin}: ${result.error.message}`);
}
if (result.signal) {
  // Re-raise the signal on ourselves instead of exiting, so the parent
  // observes the same termination (e.g. 130 for SIGINT).
  process.kill(process.pid, result.signal);
  setTimeout(() => process.exit(1), 1000).unref();
} else {
  process.exit(result.status === null ? 1 : result.status);
}

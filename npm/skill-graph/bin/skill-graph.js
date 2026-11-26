#!/usr/bin/env node
"use strict";

// Resolves the platform-specific optional dependency and execs the native
// binary shipped inside it. No postinstall scripts, no downloads.

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const PLATFORM_PACKAGES = {
  "linux-x64": "@rebuildup/skill-graph-linux-x64",
  "linux-arm64": "@rebuildup/skill-graph-linux-arm64",
  "darwin-x64": "@rebuildup/skill-graph-darwin-x64",
  "darwin-arm64": "@rebuildup/skill-graph-darwin-arm64",
};

function fail(message) {
  process.stderr.write(`skill-graph: ${message}\n`);
  process.exit(1);
}

const key = `${process.platform}-${process.arch}`;
const pkgName = PLATFORM_PACKAGES[key];
if (!pkgName) {
  fail(
    `unsupported platform ${key}. ` +
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
  process.kill(process.pid, result.signal);
}
process.exit(result.status === null ? 1 : result.status);

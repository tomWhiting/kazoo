#!/usr/bin/env node
// kzoo: runs the kazoo program built for this machine, which npm installs
// beside this package as kzoo-<platform>-<arch>.
"use strict";

const { spawnSync } = require("node:child_process");

const platforms = {
  "darwin-arm64": "kzoo-darwin-arm64",
  "darwin-x64": "kzoo-darwin-x64",
};

const key = `${process.platform}-${process.arch}`;
const name = platforms[key];
if (!name) {
  console.error(
    `kzoo: there is no build for ${key} yet; kazoo runs on macOS (Apple silicon or Intel).\n` +
      "From source, with Rust installed: cargo install --locked kzoo",
  );
  process.exit(1);
}

let binary;
try {
  binary = require.resolve(`${name}/bin/kzoo`);
} catch {
  console.error(
    `kzoo: the ${name} package that holds the program is missing.\n` +
      "npm leaves it out when optional dependencies are turned off: install again without " +
      "--omit=optional (or --no-optional), or: cargo install --locked kzoo",
  );
  process.exit(1);
}

const run = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (run.error) {
  console.error(`kzoo: could not start ${binary}: ${run.error.message}`);
  process.exit(1);
}
if (run.signal) {
  // Ended by a signal: end this process the same way.
  process.kill(process.pid, run.signal);
}
process.exit(run.status ?? 1);

// The launcher's body, which each command's own file (kzhz.js, kzoo.js,
// kazoo.js) calls with its name: it runs the kazoo program built for this
// machine, which npm installs beside this package as
// kzoo-<platform>-<arch>. The program is the one the terminal talks to:
// Ctrl-C reaches it, a stop asked of this launcher is passed on to it, and
// this launcher ends as it ended. Each command names itself in its own
// file, as a runner's shim (pnpm's) runs the file and not the link.
"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");

const platforms = {
  "darwin-arm64": "kzoo-darwin-arm64",
  "darwin-x64": "kzoo-darwin-x64",
};

// Run the program as `ranAs` (`kzhz`, `kzoo` or `kazoo`): the name its help
// and errors use, and this launcher's own errors too.
module.exports = function launch(ranAs) {
  function fail(message) {
    console.error(`${ranAs}: ${message}`);
    process.exit(1);
  }

  const key = `${process.platform}-${process.arch}`;
  const name = platforms[key];
  if (!name) {
    const from_source =
      process.platform === "win32"
        ? ""
        : "\nFrom source, with Rust 1.89 or later: cargo install kzoo";
    fail(
      `there is no build for ${key}: kazoo runs on macOS (Apple silicon or Intel).${from_source}`,
    );
  }

  let manifest;
  try {
    manifest = require.resolve(`${name}/package.json`);
  } catch {
    fail(
      `the ${name} package that holds the program was not installed.\n` +
        "npm leaves it out when optional dependencies are omitted (--omit=optional), or when a " +
        "lockfile made on another kind of Mac is reused: remove node_modules and the lockfile, " +
        "and install again.",
    );
  }
  const binary = path.join(path.dirname(manifest), "bin", "kzoo");
  try {
    fs.accessSync(binary, fs.constants.X_OK);
  } catch {
    fail(`${name} is installed but holds no program it can run (${binary}): install it again.`);
  }

  // Started by the name typed, which it answers to; nothing is put in the
  // environment, so what it starts in turn is not told a name.
  const child = spawn(binary, process.argv.slice(2), {
    stdio: "inherit",
    argv0: ranAs,
  });

  // Ctrl-C goes to the whole foreground group, the program included: it
  // decides what to do with it, and this launcher waits for it to end.
  const ignore = () => {};
  process.on("SIGINT", ignore);
  process.on("SIGQUIT", ignore);
  // A stop asked of this launcher alone is passed on.
  const passOn = (signal) => () => {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill(signal);
    }
  };
  const passed = { SIGTERM: passOn("SIGTERM"), SIGHUP: passOn("SIGHUP") };
  for (const [signal, handler] of Object.entries(passed)) {
    process.on(signal, handler);
  }

  child.on("error", (error) => fail(`could not start ${binary}: ${error.message}`));
  child.on("exit", (code, signal) => {
    process.removeListener("SIGINT", ignore);
    process.removeListener("SIGQUIT", ignore);
    for (const [sig, handler] of Object.entries(passed)) {
      process.removeListener(sig, handler);
    }
    if (signal) {
      // Ended by a signal: end the same way where Node can be ended by it,
      // and with the shell's code for it otherwise (SIGPIPE, SIGUSR1).
      const number = os.constants.signals[signal] ?? 0;
      if (!["SIGPIPE", "SIGUSR1", "SIGUSR2"].includes(signal)) {
        process.kill(process.pid, signal);
      }
      process.exit(128 + number);
    }
    process.exit(code ?? 1);
  });
};

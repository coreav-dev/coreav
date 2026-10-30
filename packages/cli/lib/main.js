"use strict";
// The coreav command from npm (docs/design/npm-cli.md): the native binary
// of this platform's package, `@coreav/cli-<platform>-<arch>`, an optional
// dependency; where there is none (Windows, other platforms), the WASI
// build (`wasi.mjs`). Where the install script could not make the native
// binary the `coreav` command itself (installs without scripts), npm's
// `coreav` runs this file, which starts the binary.

const { spawn } = require("node:child_process");
const { constants } = require("node:os");

/** Platforms with a native package: `${process.platform}-${process.arch}`. */
const PLATFORMS = ["linux-x64", "linux-arm64", "darwin-arm64", "darwin-x64"];

/**
 * The native binary to run: `COREAV_BINARY` when set (a path) and
 * `override` allows it, else this platform's package's, or null.
 */
function nativeBinary({ override = true } = {}) {
  if (override && process.env.COREAV_BINARY) return process.env.COREAV_BINARY;
  const platform = `${process.platform}-${process.arch}`;
  if (!PLATFORMS.includes(platform)) return null;
  try {
    return require.resolve(`@coreav/cli-${platform}/bin/coreav`);
  } catch {
    return null;
  }
}

/**
 * Signals passed on to the binary, so that stopping the launcher stops it
 * (Windows has no SIGHUP to send).
 */
const FORWARDED = process.platform === "win32" ? ["SIGINT", "SIGTERM"] : ["SIGINT", "SIGTERM", "SIGHUP"];

/** Runs the command with this process's arguments, streams and exit code. */
function run() {
  const binary = nativeBinary();
  if (binary === null) {
    import("./wasi.mjs");
    return;
  }
  let child;
  try {
    child = spawn(binary, process.argv.slice(2), { stdio: "inherit", windowsHide: true });
  } catch {
    // Errors that spawn throws at once (ENOEXEC on macOS): as below.
    import("./wasi.mjs");
    return;
  }
  const forward = FORWARDED.map((signal) => {
    const handler = () => {
      try {
        child.kill(signal);
      } catch {
        // The binary has ended.
      }
    };
    process.on(signal, handler);
    return [signal, handler];
  });
  const release = () => {
    for (const [signal, handler] of forward) process.off(signal, handler);
  };
  let started = false;
  child.on("spawn", () => {
    started = true;
  });
  child.on("error", () => {
    // A binary that cannot be spawned here (ENOENT, EACCES: missing, no
    // execute permission): the WASI build gives the same results, more
    // slowly. A binary that starts and fails is not retried (on Linux a
    // binary of another architecture starts through /bin/sh and exits
    // 126); the install script ran it once already.
    if (started) return;
    release();
    import("./wasi.mjs");
  });
  child.on("exit", (code, signal) => {
    release();
    if (signal !== null) {
      // Ended by a signal: end the same way, or exit as a shell reports it
      // (128 + the signal's number) if this process ignores it.
      process.exitCode = 128 + (constants.signals[signal] ?? 0);
      process.kill(process.pid, signal);
      return;
    }
    process.exitCode = code;
  });
}

module.exports = { PLATFORMS, nativeBinary, run };

if (require.main === module) run();

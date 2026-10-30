"use strict";
// Install script (docs/design/npm-cli.md, "Native binaries"): makes the
// installed `coreav` command this platform's native binary itself, so that
// a call does not start Node first. It leaves the Node launcher in place,
// which starts the binary or the WASI build, wherever that cannot be done:
// without a platform package (Windows among them), or with a binary that
// does not run here. It never fails the install.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

function install() {
  // Only as an installed package: in the repository, the tracked launcher
  // must stay what `npm pack` ships.
  if (!__dirname.split(path.sep).includes("node_modules")) return;
  const { nativeBinary } = require("./lib/main.js");
  // This platform's package only, never a binary named by the environment.
  const binary = nativeBinary({ override: false });
  if (binary === null) return; // No native package: the WASI build.
  const { version } = require("./package.json");
  const printed = execFileSync(binary, ["--version"], { encoding: "utf8", timeout: 30000 }).trim();
  if (printed !== `coreav ${version}`) return; // Not this version's binary.
  const target = path.join(__dirname, "bin", "coreav");
  // A name of this process's own, so that installs into the same directory
  // at once do not write one file.
  const temporary = `${target}.${process.pid}.tmp`;
  try {
    fs.copyFileSync(binary, temporary);
    fs.chmodSync(temporary, 0o755);
    fs.renameSync(temporary, target);
  } finally {
    fs.rmSync(temporary, { force: true });
  }
}

try {
  install();
} catch {
  // The Node launcher stays, and still runs the binary or the WASI build.
}

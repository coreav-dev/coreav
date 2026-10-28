// The native path: the launcher starts the platform's binary with the
// caller's arguments, streams and exit code, and falls back to the WASI
// build when it cannot; the install script makes the `coreav` command the
// binary itself, or leaves the launcher. Needs COREAV_BIN (a native coreav
// of this package's version); the fixture cases also COREAV_FIXTURE_DIRS.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const main = join(root, "lib", "main.js");
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const bin = process.env.COREAV_BIN;
const dirs = (process.env.COREAV_FIXTURE_DIRS ?? "").split(",").filter(Boolean);
const needsBin = bin ? false : "needs COREAV_BIN";
const platform = `${process.platform}-${process.arch}`;
const exe = process.platform === "win32" ? "coreav.exe" : "coreav";

function run(command, args, env = {}, cwd = undefined) {
  const done = spawnSync(command, args, { cwd, env: { ...process.env, ...env }, maxBuffer: 64 << 20 });
  return { status: done.status, signal: done.signal, stdout: done.stdout, stderr: done.stderr };
}

test("the platforms, the optional packages and the versions agree", () => {
  const { PLATFORMS } = createRequire(import.meta.url)(main);
  assert.deepEqual(Object.keys(pkg.optionalDependencies).sort(), PLATFORMS.map((p) => `@coreav/cli-${p}`).sort());
  for (const version of Object.values(pkg.optionalDependencies)) assert.equal(version, pkg.version);
  const cargo = readFileSync(join(root, "..", "..", "Cargo.toml"), "utf8");
  assert.match(cargo, new RegExp(`\\[workspace\\.package\\][^\\[]*version = "${pkg.version.replaceAll(".", "\\.")}"`));
});

test("the launcher runs the native binary with the caller's arguments, streams and exit code", { skip: needsBin }, () => {
  const env = { COREAV_BINARY: bin };
  assert.deepEqual(run(process.execPath, [main, "--version"], env), run(bin, ["--version"]));
  assert.deepEqual(run(process.execPath, [main, "no-such-command"], env), run(bin, ["no-such-command"]));
  assert.deepEqual(run(process.execPath, [main, "probe", "no-such-file.mp4"], env), run(bin, ["probe", "no-such-file.mp4"]));
  for (const dir of dirs) {
    for (const name of ["basic.mp4", "remux_metadata.mov", "webm_vp9_opus.webm"]) {
      const path = join(dir, name);
      if (!existsSync(path)) continue;
      for (const args of [["probe", path], ["clip", "--start", "0", "--end", "1", path, "-"], ["remux", path, "-"]]) {
        assert.deepEqual(run(process.execPath, [main, ...args], env), run(bin, args), `${name} ${args[0]}`);
      }
    }
  }
});

test("a binary that cannot start falls back to the WASI build", { skip: needsBin }, () => {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-native-"));
  try {
    const broken = join(scratch, exe);
    writeFileSync(broken, "not a program");
    chmodSync(broken, 0o644);
    assert.deepEqual(run(process.execPath, [main, "--version"], { COREAV_BINARY: broken }), run(bin, ["--version"]));
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

/** A copy of the package in a project, with `binary` as this platform's package if given. */
function installed(scratch, binary) {
  const project = join(scratch, "node_modules");
  const copy = join(project, "coreav");
  for (const part of ["bin", "lib", "install.js", "package.json"]) cpSync(join(root, part), join(copy, part), { recursive: true });
  if (binary !== undefined) {
    const native = join(project, "@coreav", `cli-${platform}`);
    mkdirSync(join(native, "bin"), { recursive: true });
    cpSync(binary, join(native, "bin", exe));
    chmodSync(join(native, "bin", exe), 0o755);
    writeFileSync(join(native, "package.json"), JSON.stringify({ name: `@coreav/cli-${platform}`, version: pkg.version }));
  }
  return copy;
}

test("the install script makes the command the native binary", { skip: needsBin || (process.platform === "win32" && "the launcher stays on Windows") }, () => {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-install-"));
  try {
    const copy = installed(scratch, bin);
    const done = run(process.execPath, [join(copy, "install.js")], { COREAV_BINARY: "" });
    assert.equal(done.status, 0, done.stderr.toString());
    assert.ok(readFileSync(join(copy, "bin", "coreav")).equals(readFileSync(bin)), "bin/coreav is not the binary");
    assert.deepEqual(run(join(copy, "bin", "coreav"), ["--version"]), run(bin, ["--version"]));
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

test("the install script leaves the launcher for another version's binary, or none", { skip: needsBin }, () => {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-install-"));
  try {
    const other = join(scratch, "other");
    writeFileSync(other, "#!/bin/sh\necho coreav 9.9.9\n");
    chmodSync(other, 0o755);
    for (const binary of [process.platform === "win32" ? undefined : other, undefined]) {
      rmSync(join(scratch, "node_modules"), { recursive: true, force: true });
      const copy = installed(scratch, binary);
      const launcher = readFileSync(join(copy, "bin", "coreav"));
      const done = run(process.execPath, [join(copy, "install.js")], { COREAV_BINARY: "" });
      assert.equal(done.status, 0, done.stderr.toString());
      assert.ok(readFileSync(join(copy, "bin", "coreav")).equals(launcher), "the launcher was replaced");
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

test("the install script changes nothing outside node_modules, and ignores COREAV_BINARY", { skip: needsBin || (process.platform === "win32" && "the launcher stays on Windows") }, () => {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-install-"));
  try {
    // A checkout: no node_modules in the path, a platform package beside it.
    const copy = installed(scratch, bin);
    const checkout = join(scratch, "checkout");
    cpSync(copy, checkout, { recursive: true });
    const launcher = readFileSync(join(checkout, "bin", "coreav"));
    run(process.execPath, [join(checkout, "install.js")], { COREAV_BINARY: bin });
    assert.ok(readFileSync(join(checkout, "bin", "coreav")).equals(launcher), "a checkout's launcher was replaced");
    // Installed, without a platform package: COREAV_BINARY is not installed.
    rmSync(join(scratch, "node_modules"), { recursive: true, force: true });
    const bare = installed(scratch);
    run(process.execPath, [join(bare, "install.js")], { COREAV_BINARY: bin });
    assert.ok(readFileSync(join(bare, "bin", "coreav")).equals(launcher), "COREAV_BINARY was installed");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

test("stopping the launcher stops the binary", { skip: process.platform === "win32" && "POSIX signals" }, async () => {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-signal-"));
  try {
    // A stand-in binary that records its pid and waits.
    const pidFile = join(scratch, "pid");
    const slow = join(scratch, "slow");
    writeFileSync(slow, `#!/bin/sh\necho $$ > ${pidFile}\nexec sleep 30\n`);
    chmodSync(slow, 0o755);
    const { spawn } = await import("node:child_process");
    const launcher = spawn(process.execPath, [main], { env: { ...process.env, COREAV_BINARY: slow }, stdio: "ignore" });
    for (let i = 0; i < 100 && !existsSync(pidFile); i++) await new Promise((r) => setTimeout(r, 50));
    const pid = Number(readFileSync(pidFile, "utf8"));
    launcher.kill("SIGTERM");
    const [code, signal] = await new Promise((resolve) => launcher.on("exit", (c, s) => resolve([c, s])));
    assert.ok(signal === "SIGTERM" || code === 143, `launcher exit ${code} ${signal}`);
    await new Promise((r) => setTimeout(r, 200));
    assert.throws(() => process.kill(pid, 0), "the binary is still running");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});

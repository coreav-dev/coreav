// The WASI build (the fallback, lib/wasi.mjs) against the native coreav
// command: the same stdout, stderr, exit code and output bytes for every
// subcommand on every fixture, with relative and absolute paths. Needs COREAV_BIN (the native coreav) and
// COREAV_FIXTURE_DIRS (directories from scripts/gen-fixtures.sh and
// scripts/gen-fixtures-mkv.sh, comma separated); skipped otherwise. The
// tests without fixtures need only the built module (scripts/build.sh).

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const launcher = join(here, "..", "lib", "wasi.mjs");
const policies = join(here, "..", "..", "..", "docs", "schema", "policy-examples");
const bin = process.env.COREAV_BIN;
const dirs = (process.env.COREAV_FIXTURE_DIRS ?? "").split(",").filter(Boolean);
const skip = !bin || dirs.length === 0 ? "needs COREAV_BIN and COREAV_FIXTURE_DIRS" : false;

/** Exit status and output as bytes: a clip to `-` writes MP4 to stdout. */
function run(command, args, cwd) {
  const done = spawnSync(command, args, { cwd, maxBuffer: 64 << 20 });
  return { status: done.status, stdout: done.stdout, stderr: done.stderr };
}

const npx = (args, cwd) => run(process.execPath, [launcher, ...args], cwd);

test("the version is the package's", () => {
  const { version } = JSON.parse(readFileSync(join(here, "..", "package.json"), "utf8"));
  const got = npx(["--version"]);
  assert.equal(got.status, 0);
  assert.equal(got.stdout.toString(), `coreav ${version}\n`);
  assert.equal(got.stderr.length, 0);
});

test("usage errors and missing files keep their exit codes, with nothing else on stderr", () => {
  const usage = npx(["no-such-command"]);
  assert.equal(usage.status, 2);
  assert.match(usage.stderr.toString(), /^error: unrecognized subcommand/);
  const missing = npx(["probe", "no-such-file.mp4"], tmpdir());
  assert.equal(missing.status, 3);
  assert.equal(JSON.parse(missing.stderr.toString()).schema, "coreav.error/1");
});

test("a file written to stdout survives a reader slower than the writer", { skip }, async () => {
  // More than a pipe buffer (64 KiB), not read for a second: the writes
  // block instead of failing with EAGAIN.
  const input = dirs.map((dir) => join(dir, "fragmented_many.mp4")).find((path) => existsSync(path));
  assert.ok(input, "fragmented_many.mp4 is missing from the fixtures");
  const expected = spawnSync(bin, ["remux", input, "-"], { maxBuffer: 64 << 20 }).stdout;
  assert.ok(expected.length > 1 << 20);
  const child = spawn(process.execPath, [launcher, "remux", input, "-"], { stdio: ["ignore", "pipe", "pipe"] });
  const closed = new Promise((resolve) => child.on("close", resolve));
  const stderr = [];
  child.stderr.on("data", (chunk) => stderr.push(chunk));
  child.stdout.pause();
  await new Promise((resolve) => setTimeout(resolve, 1000));
  const chunks = [];
  child.stdout.on("data", (chunk) => chunks.push(chunk));
  child.stdout.resume();
  const code = await closed;
  assert.equal(code, 0, Buffer.concat(stderr).toString());
  assert.ok(Buffer.concat(chunks).equals(expected), "stdout differs from the native command's");
});

/** A case's output: a file's bytes, or every segment `split` wrote, by name; null if none. */
function snapshot(output) {
  if (!output.includes("%")) return existsSync(output) ? readFileSync(output) : null;
  const dir = dirname(output);
  const names = existsSync(dir) ? readdirSync(dir).sort() : [];
  return names.length === 0 ? null : names.map((n) => [n, readFileSync(join(dir, n))]);
}

function clear(output) {
  if (!output.includes("%")) return rmSync(output, { force: true });
  rmSync(dirname(output), { recursive: true, force: true });
  mkdirSync(dirname(output));
}

/**
 * Every case: [label, arguments with IN and OUT (a file) or SEGMENTS (a
 * `split` pattern) placeholders, writes an output].
 */
function cases() {
  const list = [
    ["probe", ["probe", "IN"], false],
    ["probe compat", ["probe", "--ffprobe-compat", "IN"], false],
    ["probe frame sizes", ["probe", "--scan-frame-sizes", "IN"], false],
    ["chapters", ["chapters", "IN"], false],
    ["extract-audio", ["extract-audio", "IN", "OUT"], true],
    ["clip", ["clip", "--start", "0", "--end", "1", "IN", "OUT"], true],
    ["clip to stdout", ["clip", "--start", "0", "--end", "1", "IN", "-"], false],
    ["extract-subtitles", ["extract-subtitles", "IN", "OUT"], true],
    ["remux", ["remux", "IN", "OUT"], true],
    ["sanitize", ["sanitize", "IN", "OUT"], true],
    ["split", ["split", "--every", "1", "IN", "SEGMENTS"], true],
  ];
  for (const name of readdirSync(policies).sort()) {
    list.push([`check ${name}`, ["check", "--policy", join(policies, name), "IN"], false]);
  }
  return list;
}

test("every subcommand equals the native command on every fixture", { skip }, () => {
  const scratch = mkdtempSync(join(tmpdir(), "coreav-npx-"));
  let compared = 0;
  let outputs = 0;
  // Labels of the cases that succeeded on some fixture: none may be a
  // usage error everywhere.
  const succeeded = new Set();
  try {
    for (const dir of dirs) {
      for (const name of readdirSync(dir).filter((f) => f !== "MANIFEST").sort()) {
        for (const [label, template, writes] of cases()) {
          // Absolute paths; an input relative to the working directory;
          // outputs relative to it.
          for (const [input, relative, cwd] of [
            [join(dir, name), false, scratch],
            [name, false, dir],
            [join(dir, name), true, scratch],
          ]) {
            const segments = template.includes("SEGMENTS");
            const argument = segments ? join("segments", "%03d.mp4") : "out";
            const target = join(scratch, argument);
            const output = relative ? argument : target;
            const args = template.map((a) => (a === "IN" ? input : a === "OUT" || a === "SEGMENTS" ? output : a));
            clear(target);
            const expected = run(bin, args, cwd);
            const bytes = writes ? snapshot(target) : null;
            clear(target);
            const got = npx(args, cwd);
            const where = `${name} ${label} (${input}${relative ? ", relative output" : ""})`;
            assert.deepEqual(got, expected, where);
            if (expected.status === 0) succeeded.add(label);
            if (bytes !== null) {
              assert.deepEqual(snapshot(target), bytes, `${where}: output bytes differ`);
              outputs += 1;
            } else if (writes) {
              assert.equal(snapshot(target), null, `${where}: an output the native command did not write`);
            }
            compared += 1;
          }
        }
      }
    }
    assert.deepEqual(readdirSync(scratch).filter((f) => f.endsWith(".part")), [], "temporary files left behind");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
  assert.deepEqual(cases().map(([label]) => label).filter((label) => !succeeded.has(label)), []);
  assert.ok(outputs > 50, `only ${outputs} outputs compared`);
  console.log(`${compared} comparisons, ${outputs} outputs`);
});

// The WASI build (the fallback, lib/wasi.mjs) against the native coreav
// command: the same stdout, stderr, exit code and output bytes for every
// subcommand on every fixture, with relative and absolute paths. Needs COREAV_BIN (the native coreav) and
// COREAV_FIXTURE_DIRS (directories from scripts/gen-fixtures.sh and
// scripts/gen-fixtures-mkv.sh, comma separated); skipped otherwise. The
// tests without fixtures need only the built module (scripts/build.sh).

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { compareCases } from "./parity.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const launcher = join(here, "..", "lib", "wasi.mjs");
const policies = join(here, "..", "..", "..", "docs", "schema", "policy-examples");
const bin = process.env.COREAV_BIN;
const dirs = (process.env.COREAV_FIXTURE_DIRS ?? "").split(",").filter(Boolean);
const skip = !bin || dirs.length === 0 ? "needs COREAV_BIN and COREAV_FIXTURE_DIRS" : false;

/** Exit status and output as bytes: a clip to `-` writes MP4 to stdout. */
function run(command, args, cwd) {
  const done = spawnSync(command, args, { cwd, maxBuffer: 64 << 20 });
  if (done.error) throw done.error;
  if (done.signal) throw new Error(`child terminated by ${done.signal}`);
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

/**
 * Every case: [label, arguments with IN and OUT (a file) or SEGMENTS (a
 * `split` pattern) placeholders, writes an output].
 */
function cases() {
  const list = [
    ["probe", ["probe", "IN"], false],
    ["ffprobe", ["ffprobe", "-of", "json", "-show_format", "-show_streams", "IN"], false],
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

test("every subcommand equals the native command on every fixture", { skip }, async () => {
  const commands = cases();
  const enumeration = [];
  for (const dir of dirs) {
    for (const name of readdirSync(dir).filter((f) => f !== "MANIFEST").sort()) {
      for (const [label, template, writes] of commands) {
        for (const [relativeInput, relativeOutput] of [[false, false], [true, false], [false, true]]) {
          enumeration.push({ label, template, writes, input: join(dir, name), relativeInput, relativeOutput });
        }
      }
    }
  }
  const result = await compareCases(enumeration, { native: [bin], wasi: [process.execPath, launcher] });
  assert.deepEqual(commands.map(([label]) => label).filter((label) => !result.succeeded.includes(label)), []);
  assert.ok(result.outputs > 50, `only ${result.outputs} outputs compared`);
  console.log(`${result.compared} comparisons, ${result.outputs} outputs`);
  if (process.env.COREAV_PARITY_SUMMARY) writeFileSync(process.env.COREAV_PARITY_SUMMARY, JSON.stringify(result));
});

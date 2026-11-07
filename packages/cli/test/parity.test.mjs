import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { compareCases, run, workerCount } from "./parity.mjs";

const child = join(dirname(fileURLToPath(import.meta.url)), "parity-child.mjs");
const native = [process.execPath, child, "native"];
const wasi = [process.execPath, child, "wasi"];

function fixture(t, specs) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "parity-test-")));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const log = join(root, "calls.jsonl");
  const cases = specs.map((spec, id) => {
    const input = join(root, `${id}.json`);
    writeFileSync(input, JSON.stringify({ ...spec, id, log }));
    return { label: `command-${id % 3}`, template: ["IN", id % 2 ? "SEGMENTS" : "OUT"], writes: true, input };
  });
  return { root, log, cases };
}

test("serial and four workers cover every path variant once with isolated outputs", async (t) => {
  for (const workers of [1, 4]) {
    const { root, log, cases } = fixture(t, Array.from({ length: 18 }, () => ({})));
    const enumeration = cases.flatMap((item) => [[false, false], [true, false], [false, true]].map(
      ([relativeInput, relativeOutput]) => ({ ...item, relativeInput, relativeOutput }),
    ));
    const result = await compareCases(enumeration, { native, wasi, root, workers });
    assert.deepEqual(result, { compared: 54, outputs: 54, succeeded: ["command-0", "command-1", "command-2"] });
    const calls = readFileSync(log, "utf8").trim().split("\n").map(JSON.parse);
    assert.equal(calls.length, enumeration.length * 2);
    for (let id = 0; id < cases.length; id++) {
      const own = calls.filter((call) => call.id === id);
      const variants = new Map();
      for (const call of own) {
        const key = `${call.cwd}|${call.output}`;
        if (!variants.has(key)) variants.set(key, []);
        variants.get(key).push(call);
      }
      assert.equal(variants.size, 3);
      for (const pair of variants.values()) assert.deepEqual(pair.map((call) => call.arm), ["native", "wasi"]);
      assert.equal(own.filter((call) => call.cwd === root).length, 2);
      assert.equal(own.filter((call) => !call.output.startsWith("/")).length, 2);
    }
    const scratches = new Set(calls.filter((call) => call.cwd !== root).map((call) => call.cwd));
    assert.equal(scratches.size, workers);
    assert.deepEqual(readdirSync(root).filter((name) => name.startsWith("coreav-npx-")), []);
  }
});

test("the earliest enumeration failure wins after later children drain and cleanup", async (t) => {
  const { root, log, cases } = fixture(t, [
    { mismatch: true, delay: { native: 150 } }, { mismatch: true }, {}, {}, {}, {},
  ]);
  await assert.rejects(compareCases(cases, { native, wasi, root, workers: 4 }), (error) => {
    assert.equal(error.message, "parity case 0 failed");
    assert.match(error.cause.message, /case 0:/);
    return true;
  });
  assert.equal(readFileSync(log, "utf8").trim().split("\n").length, 12);
  assert.deepEqual(readdirSync(root).filter((name) => name.startsWith("coreav-npx-")), []);
});

test("nonzero exit codes compare, while leftover temporary files fail and cleanup", async (t) => {
  const { root, cases } = fixture(t, [{ status: 3 }]);
  assert.deepEqual(await compareCases(cases, { native, wasi, root }), { compared: 1, outputs: 1, succeeded: [] });
  const partial = fixture(t, [{ part: true }]);
  await assert.rejects(compareCases(partial.cases, { native, wasi, root: partial.root }), (error) => {
    assert.match(error.cause.message, /temporary files left behind/);
    return true;
  });
  assert.deepEqual(readdirSync(partial.root).filter((name) => name.startsWith("coreav-npx-")), []);
  const different = fixture(t, [{ status: { native: 0, wasi: 3 } }]);
  await assert.rejects(compareCases(different.cases, { native, wasi, root: different.root }), /parity case 0 failed/);
});

test("capture is isolated, bounded on both pipes, and reports launch and signal errors", async () => {
  assert.deepEqual(await run(process.execPath, ["-e", "process.stdout.write('out'); process.stderr.write('err'); process.exitCode=3"]), {
    status: 3, stdout: Buffer.from("out"), stderr: Buffer.from("err"),
  });
  assert.equal((await run(process.execPath, ["-e", ""])).stderr.length, 0);
  for (const stream of ["stdout", "stderr"]) {
    assert.equal((await run(process.execPath, ["-e", `process.${stream}.write(Buffer.alloc(1024))`], undefined, 1024))[stream].length, 1024);
    await assert.rejects(run(process.execPath, ["-e", `process.${stream}.write(Buffer.alloc(1025))`], undefined, 1024), new RegExp(`${stream} exceeds 1024 bytes`));
  }
  await assert.rejects(run("/no-such-coreav-child", []), { code: "ENOENT" });
  await assert.rejects(run(process.execPath, ["-e", "process.kill(process.pid, 'SIGTERM')"]), /SIGTERM/);
});

test("launch failure also removes worker scratch and invalid worker counts fail", async (t) => {
  const { root, cases } = fixture(t, [{}]);
  await assert.rejects(compareCases(cases, { native: ["/no-such-coreav-child"], wasi, root }), (error) => error.cause.code === "ENOENT");
  assert.deepEqual(readdirSync(root).filter((name) => name.startsWith("coreav-npx-")), []);
  for (const value of [0, 5, "1.5", "", "04"]) assert.throws(() => workerCount(value));
});

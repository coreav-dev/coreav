import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

export function workerCount(value = process.env.COREAV_PARITY_WORKERS ?? "4") {
  assert.match(String(value), /^[1-4]$/, "COREAV_PARITY_WORKERS must be an integer from 1 to 4");
  return Number(value);
}

export function run(command, args, cwd, maxBuffer = 64 << 20) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
    const chunks = { stdout: [], stderr: [] };
    const lengths = { stdout: 0, stderr: 0 };
    let failure;
    child.on("error", (error) => { failure ??= error; });
    for (const stream of ["stdout", "stderr"]) {
      child[stream].on("data", (chunk) => {
        if (failure) return;
        lengths[stream] += chunk.length;
        if (lengths[stream] > maxBuffer) {
          failure = new Error(`${stream} exceeds ${maxBuffer} bytes`);
          child.kill("SIGKILL");
          chunks.stdout.length = 0;
          chunks.stderr.length = 0;
          return;
        }
        chunks[stream].push(chunk);
      });
    }
    child.on("close", (status, signal) => {
      if (failure) return reject(failure);
      if (signal) return reject(new Error(`child terminated by ${signal}`));
      resolve({ status, stdout: Buffer.concat(chunks.stdout), stderr: Buffer.concat(chunks.stderr) });
    });
  });
}

function snapshot(output) {
  if (!output.includes("%")) return existsSync(output) ? readFileSync(output) : null;
  const dir = dirname(output);
  const names = existsSync(dir) ? readdirSync(dir).sort() : [];
  return names.length === 0 ? null : names.map((name) => [name, readFileSync(join(dir, name))]);
}

function clear(output) {
  if (!output.includes("%")) return rmSync(output, { force: true });
  rmSync(dirname(output), { recursive: true, force: true });
  mkdirSync(dirname(output));
}

export async function compareCases(cases, { native, wasi, workers = workerCount(), root = tmpdir() }) {
  workers = workerCount(workers);
  let next = 0;
  let completed = 0;
  let compared = 0;
  let outputs = 0;
  let failure;
  const succeeded = new Set();
  await Promise.all(Array.from({ length: workers }, async () => {
    const scratch = mkdtempSync(join(root, "coreav-npx-"));
    try {
      while (next < cases.length) {
        const index = next++;
        const { label, template, writes, input, relativeInput, relativeOutput } = cases[index];
        try {
          const argument = template.includes("SEGMENTS") ? join("segments", "%03d.mp4") : "out";
          const target = join(scratch, argument);
          const output = relativeOutput ? argument : target;
          const cwd = relativeInput ? dirname(input) : scratch;
          const source = relativeInput ? basename(input) : input;
          const args = template.map((a) => a === "IN" ? source : a === "OUT" || a === "SEGMENTS" ? output : a);
          const where = `case ${index}: ${input} ${label}${relativeOutput ? " (relative output)" : ""}`;
          clear(target);
          const expected = await run(native[0], [...native.slice(1), ...args], cwd);
          const bytes = writes ? snapshot(target) : null;
          clear(target);
          const got = await run(wasi[0], [...wasi.slice(1), ...args], cwd);
          assert.deepEqual(got, expected, where);
          if (expected.status === 0) succeeded.add(label);
          if (bytes !== null) {
            assert.deepEqual(snapshot(target), bytes, `${where}: output bytes differ`);
            outputs += 1;
          } else if (writes) {
            assert.equal(snapshot(target), null, `${where}: an output the native command did not write`);
          }
          assert.deepEqual(readdirSync(scratch).filter((f) => f.endsWith(".part")), [], `${where}: temporary files left behind`);
          compared += 1;
        } catch (error) {
          if (!failure || index < failure.index) failure = { index, error };
        } finally {
          completed += 1;
        }
      }
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }));
  assert.equal(completed, cases.length, "every enumerated case must execute exactly once");
  if (failure) throw new Error(`parity case ${failure.index} failed`, { cause: failure.error });
  assert.equal(compared, cases.length);
  return { compared, outputs, succeeded: [...succeeded].sort() };
}

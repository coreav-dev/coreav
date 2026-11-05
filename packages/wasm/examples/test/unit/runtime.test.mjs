import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { EventEmitter, once } from "node:events";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
import { PassThrough } from "node:stream";
import { test } from "node:test";
import { setImmediate as turn } from "node:timers/promises";
import { WorkerdDiagnostics } from "../runtime.mjs";

const platform = process.platform === "win32" ? "windows" : process.platform;
const architecture = process.arch === "x64" ? "64" : process.arch;
const workerd = createRequire(import.meta.url).resolve(`@cloudflare/workerd-${platform}-${architecture}/bin/workerd${process.platform === "win32" ? ".exe" : ""}`);

test("diagnostic streams preserve UTF-8 split across chunks", async () => {
  const child = Object.assign(new EventEmitter(), { stdout: new PassThrough(), stderr: new PassThrough() });
  const runtime = new WorkerdDiagnostics(child, "split-output");
  for (const stream of [child.stdout, child.stderr]) {
    stream.write(Buffer.from([0xe2]));
    stream.write(Buffer.from([0x82, 0xac]));
  }
  child.emit("close");
  await runtime.closed;
  assert.equal(runtime.record().stdout, "\u20ac");
  assert.equal(runtime.record().stderr, "\u20ac");
});

test("the secret scan waits for pipe closure after process exit", async () => {
  const child = Object.assign(new EventEmitter(), { stdout: new PassThrough(), stderr: new PassThrough() });
  const runtime = new WorkerdDiagnostics(child, "late-output");
  child.emit("exit", 0, null);
  let scanned = false;
  const scan = runtime.assertOutputAbsent(/SECRETTOK/).then(() => { scanned = true; });
  await turn();
  assert.equal(scanned, false);
  child.stderr.write("SECRETTOK");
  child.emit("close");
  await assert.rejects(scan, error => {
    assert.match(error.message, /output contains a secret marker/);
    assert.doesNotMatch(error.message, /SECRETTOK/);
    return true;
  });
});

test("real workerd startup stderr and exit status belong only to that process", async () => {
  const directory = mkdtempSync(join(tmpdir(), "coreav-runtime-test-"));
  try {
    const prior = new WorkerdDiagnostics(spawn(process.execPath, ["-e", 'console.log(JSON.stringify({example:"media",route:"probe",outcome:"ok"}))']), "prior");
    await once(prior.child, "close");
    assert.match(prior.record().stdout, /"outcome":"ok"/);
    const config = join(directory, "invalid.capnp");
    writeFileSync(config, 'using Workerd = import "/workerd/workerd.capnp";\nconst config :Workerd.Config = (invalidField = true);\n');
    const current = new WorkerdDiagnostics(spawn(workerd, ["serve", config]), "current");
    await once(current.child, "close");
    await current.assertOutputAbsent(/SECRETTOK/);
    assert.notEqual(current.child.exitCode, 0);
    const error = current.failure("exit");
    assert.match(error.message, /stage=current reason=exit exit=[1-9]/);
    const record = current.record();
    assert.match(record.stderr, /invalidField/);
    assert.doesNotMatch(record.stdout + record.stderr + error.message, /"outcome":"ok"/);
    assert.equal(record.startupFailure.exitCode, current.child.exitCode);
    assert.equal(record.startupFailure.signalCode, null);
    assert.doesNotMatch(error.message, /invalidField/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("launch errors and signals remain fatal with their own recorded status", async () => {
  const missing = new WorkerdDiagnostics(spawn(join(tmpdir(), "coreav-missing-runtime", "workerd")), "missing");
  await new Promise(resolve => missing.child.once("close", resolve));
  assert.equal(missing.record().startupFailure, null);
  assert.ok(missing.failure("launch") instanceof Error);
  assert.equal(missing.record().startupFailure.launchError, "ENOENT");

  const stopped = new WorkerdDiagnostics(spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"]), "stopped");
  await once(stopped.child, "spawn");
  const closed = once(stopped.child, "close");
  stopped.child.kill("SIGTERM");
  await closed;
  assert.match(stopped.failure("exit").message, /reason=exit exit=none signal=SIGTERM/);
});

test("a live process that never becomes ready still produces a deadline failure", async () => {
  const runtime = new WorkerdDiagnostics(spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"]), "unready");
  try {
    await once(runtime.child, "spawn");
    assert.match(runtime.failure("deadline").message, /reason=deadline exit=none signal=none/);
    assert.equal(runtime.record().startupFailure.reason, "deadline");
    assert.equal(runtime.record().startupFailure.exitCode, null);
  } finally {
    const closed = once(runtime.child, "close");
    runtime.child.kill("SIGTERM");
    await closed;
  }
  assert.equal(runtime.record().startupFailure.signalCode, null);
});

import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { CoreAVError } from "../src/errors.js";
import { boundedOpeners } from "../src/bounded-source.js";
import {
  DEADLINE_MS,
  createAdmission,
  runBounded,
  splitHostOptions,
} from "../src/node-broker.js";
import { brokerSource } from "../src/node-session.js";

test("host options strip deadline and signal without dropping unknown fields", () => {
  const signal = new AbortController().signal;
  const host = splitHostOptions({ at: "1", deadline: 50, signal, format: "nope" });
  assert.equal(host.ms, 50);
  assert.equal(host.signal, signal);
  assert.deepEqual(host.options, { at: "1", format: "nope" });
  assert.equal(splitHostOptions({ at: "1" }).ms, DEADLINE_MS);
  assert.equal(DEADLINE_MS, 5000);
  assert.throws(() => splitHostOptions({ deadline: 5001 }), /1 to 5000/);
  assert.throws(() => splitHostOptions({ deadline: 0 }), /1 to 5000/);
});

test("a caller race does not terminate the worker or release admission", async () => {
  const admission = createAdmission();
  const order = [];
  let timer = null;
  let opened;
  const opening = new Promise((resolve) => {
    opened = resolve;
  });
  const run = runBounded(admission, { at: "1" }, {
    schedule(ms, fn) {
      timer = { ms, fn };
      return timer;
    },
    clear() {
      timer = null;
    },
    terminate() {
      order.push("terminate");
    },
    open() {
      return opening;
    },
  }, (io) => new Promise((resolve, reject) => {
    io.onStop(reject);
    io.open().then(resolve, reject);
  }));
  assert.equal(await Promise.race([run, Promise.resolve("left")]), "left");
  assert.equal(admission.held, true);
  assert.deepEqual(order, []);
  assert.equal(timer.ms, 5000);
  timer.fn();
  opened({
    close: async () => {
      order.push("close");
    },
  });
  await assert.rejects(run, (error) => error instanceof CoreAVError && error.kind === "limit" && error.limit === "timeout");
  assert.equal(order[0], "terminate");
  assert.equal(order.at(-1), "close");
  assert.equal(admission.held, false);
});

test("completion releases admission only after the session closes", async () => {
  const admission = createAdmission();
  let closed = false;
  let finish;
  const pending = new Promise((resolve) => {
    finish = resolve;
  });
  const value = await runBounded(admission, { at: "1", deadline: 20 }, {
    schedule() {
      return {};
    },
    clear() {},
    terminate() {
      throw new Error("completion must not terminate the worker");
    },
    async open() {
      return {
        close: async () => {
          closed = true;
        },
      };
    },
  }, async (io) => {
    await io.open();
    return { value: { ok: true }, pending };
  });
  assert.equal(value.ok, true);
  assert.equal(admission.held, true);
  assert.equal(closed, false);
  finish();
  await pending;
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(closed, true);
  assert.equal(admission.held, false);
});

test("abort before opening does not terminate, and abort during the call does", async () => {
  const early = createAdmission();
  let terminated = false;
  const signal = AbortSignal.abort();
  await assert.rejects(
    () => runBounded(early, { at: "1", signal }, { schedule() {}, clear() {}, terminate() { terminated = true; }, open() { throw new Error("opened"); } }, async () => "ran"),
    (error) => error.name === "AbortError",
  );
  assert.equal(terminated, false);
  assert.equal(early.held, false);

  const admission = createAdmission();
  const controller = new AbortController();
  let opened;
  const opening = new Promise((resolve) => {
    opened = resolve;
  });
  const run = runBounded(admission, { at: "1", signal: controller.signal }, {
    schedule() {
      return {};
    },
    clear() {},
    terminate() {
      terminated = true;
    },
    open() {
      return opening;
    },
  }, (io) => new Promise((resolve, reject) => {
    io.onStop(reject);
    io.open().then(resolve, reject);
  }));
  controller.abort();
  opened({ close: async () => {} });
  await assert.rejects(run, (error) => error.name === "AbortError");
  assert.equal(terminated, true);
  assert.equal(admission.held, false);
});

test("a second call is refused while admission is held", async () => {
  const admission = createAdmission();
  let terminateCount = 0;
  const first = runBounded(admission, { at: "1" }, {
    schedule() {
      return {};
    },
    clear() {},
    terminate() {
      terminateCount += 1;
    },
    open() {
      return new Promise(() => {});
    },
  }, (io) => new Promise((resolve, reject) => {
    io.onStop(reject);
  }));
  await new Promise((resolve) => setImmediate(resolve));
  await assert.rejects(
    () => runBounded(admission, { at: "2" }, { schedule() {}, clear() {}, terminate() { terminateCount += 1; }, open() {} }, async () => "second"),
    /already has an active operation/,
  );
  assert.equal(terminateCount, 0);
  assert.equal(admission.held, true);
  first.catch(() => {});
});

test("the broker source is admitted only through the private bounded opener", async () => {
  const port = new EventEmitter();
  const posted = [];
  port.postMessage = (message) => {
    posted.push(message);
  };
  const source = brokerSource(port);
  const opener = boundedOpeners.get(source);
  assert.equal(typeof opener, "function");
  const opening = opener.call(source, { memoryBytes: 1 });
  await new Promise((resolve) => setImmediate(resolve));
  const request = posted[0];
  assert.equal(request.type, "need-open");
  assert.deepEqual(request.open, { memoryBytes: 1 });
  port.emit("message", { id: request.id, type: "opened", size: 4, inMemory: true, bytes: new Uint8Array([1, 2, 3, 4]) });
  const session = await opening;
  const view = new Uint8Array(4);
  assert.equal(session.readInto(0, view), 4);
  assert.deepEqual(view, new Uint8Array([1, 2, 3, 4]));
});

test("pending termination and a pending open abort keep the lease until both settle", async () => {
  const admission = createAdmission();
  let finishTermination;
  const termination = new Promise((resolve) => {
    finishTermination = resolve;
  });
  let finishOpen;
  const opening = new Promise((resolve) => {
    finishOpen = resolve;
  });
  let openSignal = null;
  let closes = 0;
  let fire = null;
  const run = runBounded(admission, { at: "1" }, {
    schedule(_ms, fn) {
      fire = fn;
      return {};
    },
    clear() {},
    terminate() {
      return termination;
    },
    open(arg) {
      openSignal = arg.signal;
      return opening;
    },
  }, (io) => new Promise((resolve, reject) => {
    io.onStop(reject);
    io.open().then(resolve, reject);
  }));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(openSignal.aborted, false);
  fire();
  assert.equal(openSignal.aborted, true);
  assert.equal(admission.held, true);
  assert.equal(closes, 0);
  finishTermination();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(admission.held, true);
  assert.equal(closes, 0);
  finishOpen({
    close: async () => {
      closes += 1;
    },
  });
  await assert.rejects(run, (error) => error instanceof CoreAVError && error.limit === "timeout");
  assert.equal(closes, 1);
  assert.equal(admission.held, false);
});

test("a failed close poisons the host after termination and is not swallowed", async () => {
  const admission = createAdmission();
  let fire = null;
  const run = runBounded(admission, { at: "1" }, {
    schedule(_ms, fn) {
      fire = fn;
      return {};
    },
    clear() {},
    terminate() {
      return Promise.resolve(1);
    },
    async open() {
      return {
        close: async () => {
          throw new Error("close failed");
        },
      };
    },
  }, (io) => new Promise((resolve, reject) => {
    io.onStop(reject);
    io.open().then(resolve, reject);
  }));
  await new Promise((resolve) => setImmediate(resolve));
  const failed = run.then((value) => ({ value }), (error) => ({ error }));
  fire();
  const got = await failed;
  assert.equal(got.error instanceof CoreAVError, true);
  assert.equal(got.error.kind, "io");
  assert.equal(admission.held, false);
  await assert.rejects(
    () => runBounded(admission, { at: "2" }, { schedule() { return {}; }, clear() {}, terminate() {}, open() {} }, async () => "again"),
    /cannot run again/,
  );
});

test("the deadline owner does not use Promise.race", () => {
  for (const name of ["node-broker.js", "node.js", "node-worker.js", "node-session.js"]) {
    const text = readFileSync(fileURLToPath(new URL(`../src/${name}`, import.meta.url)), "utf8");
    assert.equal(text.includes("Promise.race"), false, name);
  }
  const node = readFileSync(fileURLToPath(new URL("../src/node.js", import.meta.url)), "utf8");
  assert.match(node, /return current \? current\.terminate\(\) : Promise\.resolve\(0\)/);
});

for (const stopping of ["deadline", "abort"]) {
  test(`a document cannot succeed when ${stopping} occurs during cleanup`, async () => {
    const admission = createAdmission();
    const aborts = new AbortController();
    let fire;
    let entered;
    let finish;
    let terminate;
    const closing = new Promise(resolve => { finish = resolve; });
    const closed = new Promise(resolve => { entered = resolve; });
    let terminated = false;
    const run = runBounded(admission, { deadline: 100, signal: aborts.signal }, {
      schedule(_ms, fn) { fire = fn; return {}; },
      clear() {},
      terminate() { terminated = true; return new Promise(resolve => { terminate = resolve; }); },
      async open() { return { close() { entered(); return closing; } }; },
    }, async io => { await io.open(); return { value: "document" }; });
    run.catch(() => {});
    await closed;
    if (stopping === "deadline") fire();
    else aborts.abort();
    assert.equal(terminated, true);
    assert.equal(admission.held, true);
    finish();
    await Promise.resolve();
    assert.equal(admission.held, true);
    terminate();
    await assert.rejects(run, error => stopping === "deadline" ? error.kind === "limit" && error.limit === "timeout" : error.name === "AbortError");
    assert.equal(admission.held, false);
  });
}

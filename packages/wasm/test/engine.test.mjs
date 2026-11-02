import assert from "node:assert/strict";
import { test } from "node:test";
import { createEngine } from "../src/engine-core.js";
import { SourceCleanupError, boundedOpeners } from "../src/bounded-source.js";

function deferred() {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function engine(exclusive = true, overrides = {}) {
  const result = createEngine(
    {
      initSync: () => ({ memory: { buffer: new ArrayBuffer(65536) } }),
      version: () => "test",
      runDocument: async () => "{}",
      runDocumentInMemory: () => "{}",
      runStream: async (_operation, _session, _size, _memory, sink) => {
        await sink.write(new Uint8Array([1, 2, 3]));
        return "{}";
      },
      ...overrides,
    },
    "test-fingerprint",
    { exclusive },
  );
  const saved = [
    WebAssembly.Suspending,
    WebAssembly.promising,
    WebAssembly.validate,
  ];
  WebAssembly.Suspending = function () {};
  WebAssembly.promising = function () {};
  WebAssembly.validate = () => true;
  try {
    result.init({});
  } finally {
    [WebAssembly.Suspending, WebAssembly.promising, WebAssembly.validate] =
      saved;
  }
  return result;
}

function source({ opening, closing } = {}) {
  const closed = deferred();
  const result = {
    opens: 0,
    closes: 0,
    closed: closed.promise,
    async open() {
      this.opens += 1;
      await opening;
      return {
        size: 1,
        read: async () => new Uint8Array([0]),
        close: async () => {
          this.closes += 1;
          closed.resolve();
          await closing;
        },
      };
    },
  };
  boundedOpeners.set(result, () => result.open());
  return result;
}

test("signals are not serialized into Rust options", async () => {
  const aborts = new AbortController();
  const instance = engine(true, {
    async runDocument(_op, _source, _size, _memory, options) {
      assert.deepEqual(JSON.parse(options), { at: "0" });
      return "{}";
    },
  });
  await instance.runToText("probe", source(), { signal: aborts.signal, at: "0" });
});

test("abort during opening retains admission until close settles", async () => {
  const instance = engine();
  const gate = deferred();
  const closing = deferred();
  const aborts = new AbortController();
  const input = source({ opening: gate.promise, closing: closing.promise });
  let signal;
  boundedOpeners.set(input, (options) => {
    signal = options.signal;
    return input.open();
  });
  const running = instance.runToText("probe", input, { signal: aborts.signal });
  aborts.abort(new Error("private abort marker"));
  assert.equal(signal.aborted, true);
  await assert.rejects(instance.runToText("probe", source(), {}), /active operation/);
  gate.resolve();
  await input.closed;
  await assert.rejects(instance.runToText("probe", source(), {}), /active operation/);
  closing.resolve();
  await assert.rejects(running, (error) => error.name === "AbortError" && !error.message.includes("private abort marker") && error.cause === aborts.signal.reason);
  assert.equal(input.closes, 1);
  await instance.runToText("probe", source(), {});
});

test("abort unblocks output backpressure but awaits cleanup", async () => {
  const instance = engine();
  const closing = deferred();
  const input = source({ closing: closing.promise });
  const aborts = new AbortController();
  const output = await instance.runToStream("thumbnail", input, { signal: aborts.signal });
  aborts.abort();
  await input.closed;
  await assert.rejects(instance.runToText("probe", source(), {}), /active operation/);
  await assert.rejects(output.body.getReader().read(), (error) => error.name === "AbortError");
  closing.resolve();
  await assert.rejects(output.summary, (error) => error.name === "AbortError");
  assert.equal(input.closes, 1);
  await instance.runToText("probe", source(), {});
});

test("private diagnostics bind the real lease and output capacities", async () => {
  const instance = engine();
  const input = source();
  const output = await instance.runToStream("thumbnail", input, {});
  const held = instance.diagnostics();
  assert.deepEqual(held.lease, { active: true, acquired: 1, released: 0, rejected: 0, sourceOpens: 1 });
  assert.equal(held.settled, false);
  assert.equal(held.host.requestId, 1);
  assert.equal(held.host.maxOutputBackingBytes, 3);
  assert.equal(held.host.maxOutputChargedBytes, 3);
  assert.equal(held.host.liveBytes, 3);
  await assert.rejects(instance.runToText("probe", source(), {}), /active operation/);
  assert.equal(instance.diagnostics().lease.rejected, 1);
  assert.equal(instance.diagnostics().lease.sourceOpens, 1);
  const reader = output.body.getReader();
  assert.deepEqual((await reader.read()).value, new Uint8Array([1, 2, 3]));
  assert.equal((await reader.read()).done, true);
  await output.summary;
  const settled = instance.diagnostics();
  assert.equal(settled.settled, true);
  assert.equal(settled.host.liveBytes, 0);
  assert.equal(settled.lease.released, 1);
  await instance.runToText("probe", source(), {});
  assert.equal(instance.diagnostics().host.requestId, 2);
});

test("engine construction does not initialize another artifact", () => {
  let initialized = 0;
  const unloaded = createEngine(
    {
      initSync() {
        initialized += 1;
      },
    },
    "other",
  );
  const loaded = engine();
  assert.equal(initialized, 0);
  assert.throws(() => unloaded.moduleInfo(), /not loaded/);
  assert.deepEqual(loaded.moduleInfo(), {
    version: "test",
    build: "test-fingerprint",
    memoryBytes: 65536,
  });
});

test("exclusive engines reject every operation before another session opens", async () => {
  const opening = deferred();
  const input = source({ opening: opening.promise });
  const other = source();
  const host = engine();
  const first = host.runToText("probe", input, {});
  await assert.rejects(host.runToText("probe", other, {}), /active operation/);
  await assert.rejects(
    host.runToStream("thumbnail", other, {}),
    /active operation/,
  );
  assert.equal(other.opens, 0);
  opening.resolve();
  assert.equal(await first, "{}");
  assert.equal(await host.runToText("probe", other, {}), "{}");
  assert.equal(input.closes, 1);
});

test("document admission lasts until deferred session cleanup settles", async () => {
  const closing = deferred();
  const input = source({ closing: closing.promise });
  const other = source();
  const host = engine();
  const first = host.runToText("probe", input, {});
  await input.closed;
  await assert.rejects(host.runToText("probe", other, {}), /active operation/);
  assert.equal(other.opens, 0);
  closing.resolve();
  await first;
  await host.runToText("probe", other, {});
});

test("stream admission lasts through output drain", async () => {
  const input = source();
  const other = source();
  const host = engine();
  const output = await host.runToStream("thumbnail", input, {});
  await assert.rejects(host.runToText("probe", other, {}), /active operation/);
  const reader = output.body.getReader();
  assert.deepEqual((await reader.read()).value, new Uint8Array([1, 2, 3]));
  await assert.rejects(
    host.runToStream("thumbnail", other, {}),
    /active operation/,
  );
  assert.deepEqual(await reader.read(), { value: undefined, done: true });
  assert.deepEqual(await output.summary, {});
  await host.runToText("probe", other, {});
  assert.equal(input.closes, 1);
});

test("cancellation retains admission until cleanup and execution settle", async () => {
  const closing = deferred();
  const input = source({ closing: closing.promise });
  const other = source();
  const host = engine();
  const output = await host.runToStream("thumbnail", input, {});
  const cancelled = output.body.cancel(new Error("cancelled"));
  await assert.rejects(host.runToText("probe", other, {}), /active operation/);
  assert.equal(other.opens, 0);
  closing.resolve();
  await cancelled;
  await assert.rejects(output.summary, /cancelled/);
  await host.runToText("probe", other, {});
  assert.equal(input.closes, 1);
});

test("opening and option failures release admission without retaining input", async () => {
  const host = engine();
  const failed = source();
  failed.open = () => {
    throw new Error("open");
  };
  await assert.rejects(
    host.runToText("probe", failed, {}),
    /source could not be opened/,
  );
  const options = {
    toJSON() {
      throw new TypeError("options");
    },
  };
  const input = source();
  await assert.rejects(
    host.runToStream("thumbnail", input, options),
    /options/,
  );
  assert.equal(input.opens, 0);
  await host.runToText("probe", input, {});
});

test("a synchronous export failure closes its session once", async () => {
  const host = engine(true, {
    runStream() {
      throw new Error("export failed");
    },
  });
  const input = source();
  await assert.rejects(
    host.runToStream("thumbnail", input, {}),
    /export failed/,
  );
  assert.equal(input.closes, 1);
  await host.runToText("probe", source(), {});
});

test("exclusive output refuses an oversized write before copying it", async () => {
  const host = engine(true, {
    async runStream(_operation, _session, _size, _memory, sink) {
      await sink.write(new Uint8Array(256 * 1024 + 1));
      return "{}";
    },
  });
  const input = source();
  await assert.rejects(host.runToStream("thumbnail", input, {}), /256 KiB/);
  assert.equal(input.closes, 1);
  await host.runToText("probe", source(), {});
});

test("the default engine retains concurrent document operations", async () => {
  const work = deferred();
  const host = engine(false, {
    runDocument: async () => {
      await work.promise;
      return "{}";
    },
  });
  const first = source();
  const second = source();
  const a = host.runToText("probe", first, {});
  const b = host.runToText("probe", second, {});
  assert.equal(first.opens, 1);
  assert.equal(second.opens, 1);
  work.resolve();
  assert.deepEqual(await Promise.all([a, b]), ["{}", "{}"]);
});

test("foreign sources are rejected before opening in an exclusive engine", async () => {
  let opened = false;
  await assert.rejects(
    engine().runToText(
      "probe",
      {
        open() {
          opened = true;
        },
      },
      {},
    ),
    /bounded module/,
  );
  assert.equal(opened, false);
});

test("failed cleanup keeps arbitrary host text in cause and poisons reuse", async () => {
  const host = engine();
  const input = source();
  input.open = async () => ({
    size: 1,
    close() {
      throw new Error("credential-marker");
    },
  });
  await assert.rejects(host.runToText("probe", input, {}), (error) => {
    assert.equal(error.kind, "io");
    assert.ok(!error.documentText.includes("credential-marker"));
    assert.equal(error.cause.message, "credential-marker");
    return true;
  });
  await assert.rejects(host.runToText("probe", source(), {}), /cleanup failed/);
});

test("failed cleanup during opening also makes the instance unusable", async () => {
  const host = engine();
  const input = source();
  input.open = async () => {
    throw new SourceCleanupError(new Error("credential-marker"));
  };
  await assert.rejects(
    host.runToText("probe", input, {}),
    (error) => error.kind === "io",
  );
  await assert.rejects(host.runToText("probe", source(), {}), /cleanup failed/);
});

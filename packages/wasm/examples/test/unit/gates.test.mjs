import assert from "node:assert/strict";
import { test } from "node:test";
import { GateLedger, residentMemory } from "../gates.mjs";

const build = "a".repeat(64);
const info = (isolate = "isolate-a", changes = {}) => ({ build, isolate, served: 0, inFlight: 0, pending: 0, memoryBytes: 65536, ...changes });
function setup(names = ["one"]) {
  const ledger = new GateLedger(new Map(names.map((name) => [name, 65536])), build);
  const worker = { build };
  ledger.start(worker, info());
  return { ledger, worker, reading: info("isolate-a", { served: 1 }) };
}

test("unavailable resident memory is diagnostic rather than a failure", () => {
  assert.deepEqual(residentMemory(null), { peakRss: null, peakRssError: "unavailable_on_this_os" });
  for (const read of [() => { throw new Error("unavailable"); }, () => "", () => "VmHWM: 0 kB", () => "VmHWM: 999999999999999999999 kB"])
    assert.deepEqual(residentMemory(read), { peakRss: null, peakRssError: "process_high_water_mark_unavailable" });
  assert.deepEqual(residentMemory(() => "VmHWM: 64 kB\n"), { peakRss: 65536, peakRssError: null });
});

test("only a complete closed registry passes and equality is accepted", () => {
  const { ledger, worker, reading } = setup();
  assert.equal(ledger.verdict().state, "running");
  ledger.gate(worker, "one", reading, { served: 1 });
  assert.deepEqual(ledger.finish(), { state: "passed", gatesPassed: 1, failures: [] });
  assert.throws(() => ledger.gate(worker, "one", reading), /finished/);
});

test("missing duplicate unregistered and misclassified gates cannot pass", () => {
  assert.equal(setup().ledger.finish().failures[0].reason, "missing");
  const { ledger, worker, reading } = setup();
  assert.throws(() => ledger.gate(worker, "other", reading), /unregistered/);
  assert.throws(() => ledger.diagnostic("one"), /cannot be diagnostic/);
  ledger.diagnostic("warm");
  ledger.gate(worker, "one", reading);
  assert.throws(() => ledger.gate(worker, "one", reading), /unavailable/);
  assert.equal(ledger.finish().failures[0].reason, "duplicate");
});

test("readings and completion must be available without leaking reading digits", () => {
  const missing = setup();
  assert.throws(() => missing.ledger.gate(missing.worker, "one", null), /one \(bar 65536 bytes\)/);
  assert.equal(missing.ledger.finish().failures[0].reason, "unavailable");
  for (const changes of [{ memoryBytes: undefined }, { memoryBytes: 0 }, { memoryBytes: -1 }, { memoryBytes: 1.5 }, { memoryBytes: Infinity }, { inFlight: 1 }, { pending: 1 }, { served: 0 }, { served: 2 }]) {
    const { ledger, worker, reading } = setup();
    assert.throws(() => ledger.gate(worker, "one", { ...reading, ...changes }, { served: 1 }), /unavailable/);
    assert.equal(ledger.finish().state, "failed");
  }
  const { ledger, worker, reading } = setup();
  ledger.gate(worker, "one", { ...reading, memoryBytes: 123456789 });
  const verdict = ledger.finish();
  assert.equal(verdict.failures[0].reason, "over_bar");
  assert.doesNotMatch(JSON.stringify(verdict), /123456789|memoryBytes/);
});

test("new isolates and every gated build must match their provenance", () => {
  for (const changes of [{ isolate: "changed" }, { build: "b".repeat(64) }]) {
    const { ledger, worker, reading } = setup();
    assert.throws(() => ledger.gate(worker, "one", { ...reading, ...changes }), /unavailable/);
  }
  const { ledger } = setup();
  assert.throws(() => ledger.start({ build }, info()), /reused/);
  assert.throws(() => ledger.start({ build }, info("new", { served: 1 })), /fresh/);
  const variant = setup(); variant.worker.defaultReaders = true;
  assert.throws(() => variant.ledger.gate(variant.worker, "one", variant.reading), /unavailable/);
});

test("only an explicitly shared scenario can reuse a gate's isolate", () => {
  const first = setup(["one", "two"]);
  first.ledger.gate(first.worker, "one", first.reading);
  assert.throws(() => first.ledger.gate(first.worker, "two", first.reading), /unavailable/);
  const second = setup(["one", "two"]);
  second.ledger.gate(second.worker, "one", second.reading);
  second.ledger.gate(second.worker, "two", second.reading, { sameAs: "one" });
  assert.equal(second.ledger.finish().state, "passed");
});

test("concurrency requires every exact peak entry completion and status field", () => {
  const batch = { size: 4, total: 18, entered: 18, completed: 18, peak: 4, timedOut: false, overflow: false };
  for (const change of [null, { peak: 3 }, { peak: 5 }, { entered: 17 }, { completed: 17 }, { total: 17 }, { timedOut: true }, { overflow: true }, { timedOut: undefined }]) {
    const { ledger, worker, reading } = setup();
    assert.throws(() => ledger.gate(worker, "one", { ...reading, batch: change === null ? null : { ...batch, ...change } }, { concurrency: 4, total: 18 }), /unavailable/);
  }
  const { ledger, worker, reading } = setup();
  ledger.gate(worker, "one", { ...reading, batch }, { concurrency: 4, total: 18 });
  assert.equal(ledger.finish().state, "passed");
});

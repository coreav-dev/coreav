import assert from "node:assert/strict";
import { test } from "node:test";
import { setImmediate as turn } from "node:timers/promises";
import { createTracker } from "../batch.mjs";

test("the entry latch reaches exact peaks at one four and six", async () => {
  for (const size of [1, 4, 6]) {
    const tracker = createTracker();
    tracker.arm({ size, total: size * 2 });
    let released = 0;
    const first = Array.from({ length: size - 1 }, () => tracker.enter().then((complete) => { released += 1; return complete; }));
    await turn(); assert.equal(released, 0);
    const last = tracker.enter();
    const completions = await Promise.all([...first, last]);
    for (const complete of completions) await tracker.track(new Response("fixture"), complete).arrayBuffer();
    const next = await Promise.all(Array.from({ length: size }, () => tracker.enter()));
    for (const complete of next) await tracker.track(new Response(null), complete).arrayBuffer();
    const state = await tracker.reading();
    assert.equal(state.inFlight, 0);
    assert.deepEqual(state.batch, { size, total: size * 2, entered: size * 2, completed: size * 2, peak: size, timedOut: false, overflow: false });
  }
});

test("response EOF completes accounting before the client sees done", async () => {
  const tracker = createTracker();
  const complete = await tracker.enter();
  const response = tracker.track(new Response("fixture"), complete);
  assert.equal(response.headers.has("content-length"), false);
  assert.equal(tracker.snapshot().inFlight, 1);
  await assert.rejects(tracker.reading(), /incomplete/);
  const reader = response.body.getReader();
  assert.equal((await reader.read()).done, false);
  assert.equal(tracker.snapshot().inFlight, 1);
  assert.equal((await reader.read()).done, true);
  assert.equal(tracker.snapshot().inFlight, 0);
});

test("body cancellation errors and handler failures release their accounting once", async () => {
  const tracker = createTracker();
  let complete = await tracker.enter();
  await tracker.track(new Response("fixture"), complete).body.cancel();
  assert.equal(tracker.snapshot().inFlight, 0);
  complete = await tracker.enter();
  const failed = new ReadableStream({ pull(controller) { controller.error(new Error("read failed")); } });
  await assert.rejects(tracker.track(new Response(failed), complete).arrayBuffer(), /read failed/);
  assert.equal(tracker.snapshot().inFlight, 0);
  complete = await tracker.enter(); complete(); complete();
  assert.equal(tracker.snapshot().inFlight, 0);
});

test("active work reset overflow and an incomplete latch fail closed", async () => {
  const tracker = createTracker({ deadlineMs: 10 });
  const active = await tracker.enter();
  assert.throws(() => tracker.arm({ size: 1, total: 1 }), /active/);
  active();
  tracker.arm({ size: 4, total: 4 });
  const complete = await tracker.enter(); complete();
  assert.equal(tracker.snapshot().batch.timedOut, true);
  await assert.rejects(tracker.reading(), /incomplete/);
  const overflow = createTracker(); overflow.arm({ size: 1, total: 1 });
  const first = await overflow.enter(), second = await overflow.enter(); first(); second();
  assert.equal(overflow.snapshot().batch.overflow, true);
  await assert.rejects(overflow.reading(), /incomplete/);
});

test("deferred work added during draining is also drained", async () => {
  const tracker = createTracker();
  let release;
  const last = new Promise((resolve) => { release = resolve; });
  tracker.defer(Promise.resolve().then(() => { tracker.defer(last); }));
  let done = false;
  const reading = tracker.reading().then((state) => { done = true; return state; });
  await turn(); assert.equal(done, false);
  release();
  assert.equal((await reading).pending, 0);
});

test("deferred rejection and timeout are unavailable rather than passing", async () => {
  const failed = createTracker(); failed.defer(Promise.reject(new Error("secret host failure")));
  await assert.rejects(failed.reading(), /incomplete/);
  const hung = createTracker({ deadlineMs: 10 }); hung.defer(new Promise(() => {}));
  await assert.rejects(hung.reading(), /deferred_timeout/);
});

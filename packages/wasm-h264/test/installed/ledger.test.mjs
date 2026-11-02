import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { registeredBars } from "../../../wasm/examples/test/registry.mjs";
import { ThumbnailLedger, validateParity, validateAllocation, validateHostCases, failureRecord } from "./ledger.mjs";
import { fixtureRoster, gateRoster, parityRoster, optionsSha256, roster, sha } from "./roster.mjs";

const hash = "a".repeat(64), other = "b".repeat(64);
const bars = registeredBars(readFileSync(new URL("../../../../docs/design/wasm-thumbnail-memory-bars.md", import.meta.url), "utf8"), "wasm-thumbnail-memory-bars");
const fixtures = fixtureRoster().map(row => ({ ...row, sha256: hash, codedWidth: 3840, codedHeight: 2160 }));
const groups = gateRoster();
const initial = isolate => ({ isolate, build: hash, served: 0, inFlight: 0, pending: 0, diagnostic: { lease: { acquired: 0, active: false }, settled: true } });
const host = () => ({ maxBackingBytes: 1, maxChargedBytes: 1, maxOutputBackingBytes: 1, maxOutputChargedBytes: 1, liveBytes: 0 });
const witness = () => ({ admissionRefused: true, inputSha256: hash, optionsSha256: optionsSha256({ at: "1", width: 320, format: "jpeg" }), binarySha256: hash, requestedBytes: 0, minimumLumaAllocationBytes: 3840 * 2160, troughBytes: 0 });
function record(gate, row) {
  const index = groups.get(gate).findIndex(item => item.id === row.id), served = row.operation === "startup" ? 0 : index + 1, operationId = row.operation === "admission" ? 2 * served : served;
  return { artifactSha256: hash, tarballSha256: hash, isolate: gate, requestId: sha(`${gate}/${row.id}`), operationId, served, inputSha256: hash, optionsSha256: optionsSha256(row.options), complete: true, runtimeExit: false, pending: 0, inFlight: 0, settled: true, outcome: row.expectation, memoryBytes: 65536, host: { ...host(), requestId: operationId }, nativeAllocation: { ...witness(), optionsSha256: optionsSha256(row.options) }, admission: { stage: row.id, reused: true, drained: true, settled: true, holdAcquired: 1, rejections: row.id === "probe-first" ? 1 : 2, extraAcquisitions: 0, extraSourceOpens: 0, activeAtHold: true, activeAfterCleanup: false, holdReleased: 1 } };
}
function ledger() { return new ThumbnailLedger(bars, hash, hash, fixtures); }

test("failure labels expose only owned reasons, closed phases and numeric ordinals", () => {
  let owned;
  try { validateParity(null, null, {}); } catch (error) { owned = error; }
  assert.deepEqual(failureRecord("parity", 3, owned), { stage: "parity", item: 3, reason: "incomplete_output_or_runtime_exit" });
  const untrusted = new Proxy(new Error("private-marker"), { get() { throw new Error("private-marker"); } });
  assert.deepEqual(failureRecord("private-marker", "private-marker", untrusted), { stage: "unexpected", item: 0, reason: "unexpected" });
});

test("the registered eleven gates have the complete deterministic request roster", () => {
  assert.equal(bars.size, 11); assert.equal(fixtureRoster().length, 52);
  assert.equal(groups.get("thumbnail-warm").length, 312);
  assert.equal([...groups.values()].reduce((sum, rows) => sum + rows.length, 0), 338);
  assert.equal(new Set(fixtureRoster().map(row => row.id)).size, 52);
  assert.equal(parityRoster().length, 88);
  assert.equal(new Set(parityRoster().map(row => row.id)).size, 88);
  assert.deepEqual(roster.filters, ["bilinear", "bicubic", "area"]);
  assert.deepEqual(roster.chroma, ["native", "full"]);
  const complete = ledger();
  for (const [gate, rows] of groups) {
    complete.begin(gate, initial(gate));
    for (const row of rows) complete.record(gate, row.id, record(gate, row));
  }
  assert.deepEqual(complete.finish(), { state: "passed", gatesPassed: 11, readings: 338, failures: [] });
});

test("missing, duplicate, unregistered and out-of-order steps cannot pass", () => {
  assert.equal(ledger().finish().failures.length, 338);
  const value = ledger(), gate = "thumbnail-warm", rows = groups.get(gate);
  value.begin(gate, initial(gate));
  assert.throws(() => value.record(gate, rows[1].id, record(gate, rows[1])), /ordering/);
  assert.throws(() => value.record(gate, "missing", record(gate, rows[0])), /unregistered/);
  value.record(gate, rows[0].id, record(gate, rows[0]));
  assert.throws(() => value.record(gate, rows[0].id, record(gate, rows[0])), /duplicate/);
  assert.throws(() => new ThumbnailLedger(new Map([...bars, ["unknown", 65536]]), hash, hash, fixtures), /roster/);
});

test("fresh isolates cannot have prior work or share identities", () => {
  const value = ledger();
  const used = initial("prior"); used.diagnostic.lease.acquired = 1;
  assert.throws(() => value.begin("thumbnail-startup", used), /fresh/);
  value.begin("thumbnail-startup", initial("one"));
  assert.throws(() => value.begin("thumbnail-warm", initial("one")), /identity/);
  assert.throws(() => value.begin("thumbnail-startup", initial("other")), /identity/);
});

for (const [label, mutate] of Object.entries({
  "zero": row => { row.memoryBytes = 0; },
  "negative": row => { row.memoryBytes = -65536; },
  "fraction": row => { row.memoryBytes = 0.5; },
  "not-pages": row => { row.memoryBytes = 65537; },
  "nan": row => { row.memoryBytes = NaN; },
  "infinite": row => { row.memoryBytes = Infinity; },
  "unsafe": row => { row.memoryBytes = 2 ** 54; },
  "artifact": row => { row.artifactSha256 = other; },
  "tarball": row => { row.tarballSha256 = other; },
  "isolate": row => { row.isolate = "changed"; },
  "options": row => { row.optionsSha256 = other; },
  "operation-id": row => { row.operationId = 0; },
  "stale-host-account": row => { row.host.requestId = 0; },
  "completion-count": row => { row.served = 0; },
  "input": row => { row.inputSha256 = other; },
  "incomplete": row => { row.complete = false; },
  "exit": row => { row.runtimeExit = true; },
  "deferred": row => { row.pending = 1; },
  "body": row => { row.inFlight = 1; },
  "cleanup": row => { row.settled = false; },
  "outcome": row => { row.outcome = "limit"; },
  "missing-host": row => { delete row.host; },
  "live-host": row => { row.host.liveBytes = 1; },
  "oversized-host": row => { row.host.maxChargedBytes = 16 * 1048576 + 1; },
  "undercharged": row => { row.host.maxChargedBytes = 0; },
})) test(`rejects ${label} capture`, () => {
  const value = ledger(), gate = "thumbnail-warm", wanted = groups.get(gate)[0], made = record(gate, wanted);
  value.begin(gate, initial(gate)); mutate(made);
  assert.throws(() => value.record(gate, wanted.id, made), /invalid/);
});

test("every intermediate reading is gated, even if a later reading fits", () => {
  const value = ledger(), gate = "fresh-thumbnail-jpeg-mp4", rows = groups.get(gate);
  value.begin(gate, initial(gate));
  const first = record(gate, rows[0]); first.memoryBytes = bars.get(gate) + 65536;
  value.record(gate, rows[0].id, first); value.record(gate, rows[1].id, record(gate, rows[1]));
  assert.ok(value.finish().failures.some(row => row.gate === gate && row.reason === "over_bar"));
});

test("4K refusals need independent qualified allocation evidence", () => {
  const reading = { inputSha256: hash, optionsSha256: witness().optionsSha256 };
  const fixture = { codedWidth: 3840, codedHeight: 2160 };
  validateAllocation(witness(), reading, fixture);
  for (const field of ["admissionRefused", "inputSha256", "optionsSha256", "binarySha256", "requestedBytes", "minimumLumaAllocationBytes", "troughBytes"]) {
    const bad = witness(); delete bad[field]; assert.throws(() => validateAllocation(bad, reading, fixture), /allocation/);
  }
  for (const mutation of [{ requestedBytes: 3840 * 2160 }, { troughBytes: 1 }, { requestedBytes: -1 }, { minimumLumaAllocationBytes: 1e12 }]) assert.throws(() => validateAllocation({ ...witness(), ...mutation }, reading, fixture), /allocation/);
});

test("admission requires actual lease and source-open deltas", () => {
  const gate = "thumbnail-admission", row = groups.get(gate)[0];
  for (const mutation of [{ extraSourceOpens: 1 }, { extraAcquisitions: 1 }, { rejections: 1 }, { activeAtHold: false }, { holdReleased: 0 }, { activeAfterCleanup: true }, { reused: false }]) {
    const value = ledger(), made = record(gate, row); value.begin(gate, initial(gate));
    Object.assign(made.admission, mutation); assert.throws(() => value.record(gate, row.id, made), /admission/);
  }
});

test("parity permits only decoder description text to differ", () => {
  const wanted = { inputSha256: hash, options: { at: "1" }, expectation: "success" };
  const made = { complete: true, runtimeExit: false, inputSha256: hash, optionsSha256: optionsSha256(wanted.options), outcome: "success", bytes: 12, outputSha256: hash, summary: { schema: "coreav.thumbnail/1", packet: { offset: 1, size: 2 }, time: { frame: "1" }, output: { width: 320, height: 180 }, decoder: { name: "native target", backend: "native" } } };
  const native = structuredClone(made); native.summary.decoder.name = "wasm target";
  validateParity(made, native, wanted);
  for (const mutate of [row => { row.outputSha256 = other; }, row => { row.summary.packet.offset = 2; }, row => { row.summary.time.frame = "0"; }, row => { row.summary.decoder.backend = "other"; }, row => { row.summary = undefined; }, row => { row.complete = false; }, row => { row.runtimeExit = true; }]) {
    const bad = structuredClone(made); mutate(bad); assert.throws(() => validateParity(bad, native, wanted), /invalid/);
  }
});

test("shared registry rejects escaped duplicate keys for the thumbnail marker", () => {
  const markdown = '<!-- wasm-thumbnail-memory-bars:begin -->\n```json\n{"signal":"wasm_linear_bytes","predicate":"value <= bar","gates":{"a":65536,"\\u0061":65536}}\n```\n<!-- wasm-thumbnail-memory-bars:end -->';
  assert.throws(() => registeredBars(markdown, "wasm-thumbnail-memory-bars"), /duplicate/);
});

test("request identities cannot be reused across later steps", () => {
  const value = ledger(), gate = "fresh-thumbnail-jpeg-mp4", rows = groups.get(gate);
  value.begin(gate, initial(gate));
  const first = record(gate, rows[0]), second = record(gate, rows[1]);
  value.record(gate, rows[0].id, first); second.requestId = first.requestId;
  assert.throws(() => value.record(gate, rows[1].id, second), /request identity/);
});

test("host case hook cannot omit faults, cleanup or real lease activity", () => {
  const report = { complete: true, cases: [["short-read", "io"], ["giant-backing", "limit"], ["cancellation", "cancelled"]].map(([id, outcome]) => ({ id, outcome, complete: true, settled: true, benignReuse: true })) };
  const before = { lease: { acquired: 0, released: 0 } }, after = { lease: { acquired: 8, released: 8, active: false }, settled: true, host: { liveBytes: 0 } };
  validateHostCases(report, before, after);
  for (const mutate of [row => { row.cases.pop(); }, row => { row.cases[1] = row.cases[0]; }, row => { row.cases[0].outcome = "success"; }, row => { row.cases[2].benignReuse = false; }]) {
    const bad = structuredClone(report); mutate(bad); assert.throws(() => validateHostCases(bad, before, after), /host case/);
  }
  for (const lease of [{ acquired: 0, released: 0, active: false }, { acquired: 8, released: 7, active: false }, { acquired: Infinity, released: Infinity, active: false }]) assert.throws(() => validateHostCases(report, before, { ...after, lease }), /cleanup/);
});

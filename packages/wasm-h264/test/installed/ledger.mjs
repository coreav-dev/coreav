import { canonical, gateRoster, optionsSha256 } from "./roster.mjs";

const hash = value => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
const natural = value => Number.isSafeInteger(value) && value >= 0;
const positive = value => natural(value) && value > 0;
const reasons = new WeakMap();
const fail = (reason, details = {}) => {
  const error = new Error(`thumbnail capture invalid: ${reason}`);
  reasons.set(error, { reason: reason.replaceAll(" ", "_"), ...details });
  throw error;
};
const outcomes = new Set(["success", "runtime", "limit", "io", "decode", "unsupported_feature", "codec_not_allowed", "invalid_structure", "out_of_range", "invalid_argument"]);
const failureTypes = new Set(["none", "wasm_trap", "type_error", "range_error", "coreav", "source_not_found", "request_limit", "abort", "dom_exception", "error", "other"]);
const stages = new Set(["install", "installed-files", "fixture-server", "bundle", "runtime-launch", "runtime-startup", "fresh-isolate", "before-memory", "request", "response", "result", "after-memory", "request-accounting", "native-reference", "parity", "host-accounting", "memory-ledger", "shutdown", "host-cases", "integrity", "verdict"]);

export function failureRecord(stage, item, error) {
  return {
    stage: stages.has(stage) ? stage : "unexpected",
    item: natural(item) ? item : 0,
    ...(reasons.get(error) ?? { reason: "unexpected" }),
  };
}

export function comparableSummary(summary) {
  const copy = structuredClone(summary);
  if (copy?.decoder && typeof copy.decoder.name === "string") delete copy.decoder.name;
  return copy;
}

export function validateParity(actual, native, expected) {
  if (!actual || !native || actual.complete !== true || native.complete !== true || actual.runtimeExit !== false || native.runtimeExit !== false) fail("incomplete output or runtime exit");
  if (!hash(actual.inputSha256) || actual.inputSha256 !== native.inputSha256 || actual.inputSha256 !== expected.inputSha256) fail("input identity");
  if (!hash(actual.optionsSha256) || actual.optionsSha256 !== native.optionsSha256 || actual.optionsSha256 !== optionsSha256(expected.options)) fail("options identity");
  if (actual.outcome !== native.outcome) fail("outcome parity", {
    actual: outcomes.has(actual.outcome) ? actual.outcome : "other",
    native: outcomes.has(native.outcome) ? native.outcome : "other",
    ...(actual.failureType !== undefined && { type: failureTypes.has(actual.failureType) ? actual.failureType : "other", cause: failureTypes.has(actual.causeType) ? actual.causeType : "other" }),
  });
  if (expected.expectation !== actual.outcome) fail("unexpected outcome");
  if (actual.outcome === "success") {
    if (actual.summary?.schema !== "coreav.thumbnail/1" || native.summary?.schema !== "coreav.thumbnail/1" || !actual.summary.packet || !actual.summary.time || !actual.summary.output || !actual.summary.decoder) fail("summary missing");
    if (!positive(actual.bytes) || actual.bytes !== native.bytes || !hash(actual.outputSha256) || actual.outputSha256 !== native.outputSha256) fail("image bytes");
    if (canonical(comparableSummary(actual.summary)) !== canonical(comparableSummary(native.summary))) fail("summary parity");
  } else {
    if (actual.bytes !== 0 || native.bytes !== 0 || actual.error?.schema !== "coreav.error/1" || native.error?.schema !== "coreav.error/1" || actual.error.kind !== native.error.kind || actual.error.kind !== expected.expectation) fail("structured refusal");
  }
}

export class ThumbnailLedger {
  constructor(bars, artifact, tarball, fixtures, groups = gateRoster()) {
    if (!hash(artifact) || !hash(tarball)) fail("artifact identity");
    if ([...bars.keys()].sort().join() !== [...groups.keys()].sort().join()) fail("registered gate roster");
    this.bars = new Map(bars); this.groups = groups; this.artifact = artifact; this.tarball = tarball;
    this.fixtures = new Map(fixtures.map(row => [row.id, row]));
    this.isolates = new Map(); this.readings = new Map(); this.requests = new Set(); this.operationIds = new Map(); this.closed = false;
  }
  begin(gate, info) {
    if (this.closed || !this.groups.has(gate) || this.isolates.has(gate) || !info || typeof info.isolate !== "string" || !info.isolate || [...this.isolates.values()].includes(info.isolate)) fail("fresh isolate identity");
    if (info.build !== this.artifact || info.served !== 0 || info.inFlight !== 0 || info.pending !== 0 || info.diagnostic?.lease?.acquired !== 0 || info.diagnostic?.lease?.active !== false || info.diagnostic?.settled !== true) fail("isolate not fresh");
    this.isolates.set(gate, info.isolate);
  }
  record(gate, step, reading) {
    if (this.closed) fail("closed ledger");
    const wanted = this.groups.get(gate)?.find(row => row.id === step);
    if (!wanted) fail("unregistered step");
    const key = `${gate}/${step}`;
    if (this.readings.has(key)) fail("duplicate step");
    if (this.groups.get(gate).filter(row => this.readings.has(`${gate}/${row.id}`)).length !== this.groups.get(gate).findIndex(row => row.id === step)) fail("step ordering");
    if (!reading || reading.artifactSha256 !== this.artifact || reading.tarballSha256 !== this.tarball) fail("artifact identity");
    if (typeof reading.isolate !== "string" || !reading.isolate || typeof reading.requestId !== "string" || !/^[a-z0-9-]{1,64}$/i.test(reading.requestId) || this.requests.has(reading.requestId)) fail("request identity");
    const prior = this.isolates.get(gate);
    if (!prior || prior !== reading.isolate) fail("isolate identity");
    if (reading.complete !== true || reading.runtimeExit !== false || reading.pending !== 0 || reading.inFlight !== 0 || reading.settled !== true) fail("work is incomplete");
    const index = this.groups.get(gate).findIndex(row => row.id === step);
    if (reading.served !== (wanted.operation === "startup" ? 0 : index + 1)) fail("request completion count");
    const operationId = (this.operationIds.get(gate) ?? 0) + (wanted.operation === "startup" ? 0 : wanted.operation === "admission" ? 2 : 1);
    if (!natural(reading.operationId) || reading.operationId !== operationId || wanted.operation !== "startup" && reading.host?.requestId !== operationId) fail("operation identity");
    if (reading.optionsSha256 !== optionsSha256(wanted.options)) fail("options identity");
    if (reading.outcome !== wanted.expectation) fail("unexpected outcome");
    if (!positive(reading.memoryBytes) || reading.memoryBytes % 65536 !== 0) fail("memory reading");
    if (wanted.fixture && (!hash(reading.inputSha256) || reading.inputSha256 !== this.fixtures.get(wanted.fixture)?.sha256)) fail("input identity");
    const host = reading.host;
    if (!(wanted.operation === "startup" && host === null)) {
      if (!host || ![host.maxBackingBytes, host.maxChargedBytes, host.liveBytes, host.maxOutputBackingBytes, host.maxOutputChargedBytes].every(natural) || host.liveBytes !== 0 || host.maxChargedBytes < host.maxBackingBytes || host.maxOutputChargedBytes < host.maxOutputBackingBytes) fail("host accounting");
      if (wanted.operation !== "startup" && host.maxChargedBytes === 0) fail("host accounting missing");
      if (host.maxBackingBytes > 16 * 1048576 || host.maxChargedBytes > 16 * 1048576 || host.maxOutputBackingBytes > 1.25 * 1048576 || host.maxOutputChargedBytes > 1.25 * 1048576) fail("host partition");
    }
    if (wanted.expectation === "limit") validateAllocation(reading.nativeAllocation, reading, this.fixtures.get(wanted.fixture));
    if (wanted.expectation === "admission") validateAdmission(reading.admission, step);
    this.isolates.set(gate, reading.isolate); this.requests.add(reading.requestId);
    this.operationIds.set(gate, reading.operationId);
    this.readings.set(key, { gate, step, over: reading.memoryBytes > this.bars.get(gate) });
  }
  finish() {
    this.closed = true;
    const failures = [];
    for (const [gate, rows] of this.groups) for (const { id } of rows) {
      const row = this.readings.get(`${gate}/${id}`);
      if (!row || row.over) failures.push({ gate, step: id, bar: this.bars.get(gate), reason: row ? "over_bar" : "missing" });
    }
    return { state: failures.length ? "failed" : "passed", gatesPassed: [...this.groups].filter(([gate, rows]) => rows.every(({ id }) => this.readings.has(`${gate}/${id}`) && !this.readings.get(`${gate}/${id}`).over)).length, readings: this.readings.size, failures };
  }
}

export function validateAllocation(witness, reading, fixture) {
  if (!witness || witness.admissionRefused !== true || witness.inputSha256 !== reading.inputSha256 || witness.optionsSha256 !== reading.optionsSha256 || !hash(witness.binarySha256) || !natural(witness.requestedBytes) || !positive(witness.minimumLumaAllocationBytes) || witness.requestedBytes >= witness.minimumLumaAllocationBytes || witness.troughBytes !== 0) fail("native allocation evidence");
  if (!fixture || !positive(fixture.codedWidth) || !positive(fixture.codedHeight) || witness.minimumLumaAllocationBytes > fixture.codedWidth * (fixture.codedHeight + 8)) fail("native allocation bound");
}

export function validateAdmission(record, stage) {
  if (!record || record.stage !== stage || record.reused !== true || record.drained !== true || record.settled !== true || record.holdAcquired !== 1) fail("admission lifecycle");
  const rejected = stage === "probe-first" ? 1 : 2;
  if (record.rejections !== rejected || record.extraAcquisitions !== 0 || record.extraSourceOpens !== 0 || record.activeAtHold !== true || record.activeAfterCleanup !== false || record.holdReleased !== 1) fail("admission lease");
}

export function validateHostCases(report, before, after) {
  const expected = new Map([["short-read", "io"], ["giant-backing", "limit"], ["cancellation", "cancelled"]]);
  if (!report || report.complete !== true || !Array.isArray(report.cases) || report.cases.length !== expected.size) fail("host case roster");
  for (const row of report.cases) {
    if (row.outcome !== expected.get(row.id) || row.complete !== true || row.settled !== true || row.benignReuse !== true) fail("host case outcome");
    expected.delete(row.id);
  }
  if (expected.size || ![before?.lease?.acquired, before?.lease?.released, after?.lease?.acquired, after?.lease?.released].every(natural) || after?.settled !== true || after.lease.active !== false || after.lease.acquired - before.lease.acquired < 6 || after.lease.released - before.lease.released !== after.lease.acquired - before.lease.acquired || after.host.liveBytes !== 0) fail("host case cleanup");
}

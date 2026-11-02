export class GateLedger {
  constructor(bars, build) {
    this.bars = new Map(bars);
    this.build = build;
    this.workers = new WeakMap();
    this.isolates = new Set();
    this.results = new Map();
    this.steps = new Map();
    this.finished = false;
  }
  start(worker, info) {
    if (typeof info.isolate !== "string" || !info.isolate || this.isolates.has(info.isolate) || this.workers.has(worker)) throw new Error("memory isolate identity is unavailable or reused");
    if (info.served !== 0 || info.inFlight !== 0 || info.pending !== 0 || info.build !== worker.build || !Number.isSafeInteger(info.memoryBytes) || info.memoryBytes <= 0) throw new Error("memory isolate did not start fresh");
    this.isolates.add(info.isolate);
    this.workers.set(worker, info.isolate);
  }
  diagnostic(step) {
    if (this.bars.has(step)) throw new Error(`registered memory step cannot be diagnostic: ${step}`);
  }
  gate(worker, step, info, expect = {}) {
    if (this.finished) throw new Error("memory gates are already finished");
    if (!this.bars.has(step)) throw new Error(`unregistered memory gate: ${step}`);
    const bar = this.bars.get(step);
    const prior = this.results.has(step);
    let reason = null;
    if (prior) reason = "duplicate";
    else if (info.isolate !== this.workers.get(worker) || !this.workers.has(worker)) reason = "isolate";
    else if (expect.sameAs ? this.steps.get(expect.sameAs) !== worker : [...this.steps.values()].includes(worker)) reason = "isolate";
    else if (info.build !== this.build || worker.build !== this.build || worker.defaultReaders) reason = "build";
    else if (!Number.isSafeInteger(info.memoryBytes) || info.memoryBytes <= 0 || info.inFlight !== 0 || info.pending !== 0 || !Number.isSafeInteger(info.served) || info.served <= 0) reason = "unavailable";
    else if (expect.served !== undefined && info.served !== expect.served) reason = "served";
    else if (expect.concurrency !== undefined && (!info.batch || info.batch.size !== expect.concurrency || info.batch.peak !== expect.concurrency || info.batch.total !== expect.total || info.batch.entered !== expect.total || info.batch.completed !== expect.total || info.batch.timedOut !== false || info.batch.overflow !== false)) reason = "concurrency";
    else if (info.memoryBytes > bar) reason = "over_bar";
    this.results.set(step, { step, bar, reason });
    this.steps.set(step, worker);
    if (reason && reason !== "over_bar") throw new Error(`memory gate unavailable: ${step} (bar ${bar} bytes)`);
  }
  finish() { this.finished = true; return this.verdict(); }
  verdict() {
    const failures = [...this.results.values()].filter((result) => result.reason);
    if (this.finished) for (const [step, bar] of this.bars) if (!this.results.has(step)) failures.push({ step, bar, reason: "missing" });
    return { state: this.finished ? failures.length ? "failed" : "passed" : "running", gatesPassed: [...this.results.values()].filter((result) => !result.reason).length, failures };
  }
}

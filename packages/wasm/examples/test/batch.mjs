export function createTracker({ deadlineMs = 10000 } = {}) {
  let inFlight = 0, served = 0, batch = null, deferredFailed = false;
  const pending = new Set();
  function arm({ size, total }) {
    if (inFlight || pending.size || batch && batch.completed !== batch.total) throw new Error("harness batch is still active");
    if (![1, 4, 6].includes(size) || !Number.isSafeInteger(total) || total < size) throw new Error("harness batch is invalid");
    batch = { size, total, entered: 0, completed: 0, peak: 0, timedOut: false, overflow: false };
  }
  async function enter() {
    served += 1; inFlight += 1;
    const current = batch;
    let done = false;
    const complete = () => { if (!done) { done = true; inFlight -= 1; if (current) current.completed += 1; } };
    if (current) {
      current.entered += 1;
      current.peak = Math.max(current.peak, inFlight);
      if (current.entered > current.total || inFlight > current.size) current.overflow = true;
      if (!current.release) {
        current.latch = new Promise((resolve) => { current.release = resolve; });
        current.timer = setTimeout(() => { current.timedOut = true; current.release(); }, deadlineMs);
      }
      if (current.entered === current.size) { clearTimeout(current.timer); current.release(); }
      await current.latch;
    }
    return complete;
  }
  function track(response, complete) {
    if (!response.body) { complete(); return response; }
    const reader = response.body.getReader();
    const body = new ReadableStream({
      async pull(controller) {
        try {
          const { done, value } = await reader.read();
          if (done) { complete(); controller.close(); } else controller.enqueue(value);
        } catch (error) { complete(); controller.error(error); }
      },
      async cancel(reason) { try { await reader.cancel(reason); } finally { complete(); } }
    }, { highWaterMark: 0 });
    const headers = new Headers(response.headers); headers.delete("content-length");
    return new Response(body, { status: response.status, statusText: response.statusText, headers });
  }
  function defer(promise) {
    const tracked = Promise.resolve(promise).catch(() => { deferredFailed = true; }).finally(() => pending.delete(tracked));
    pending.add(tracked);
    return tracked;
  }
  function snapshot() {
    const state = batch && Object.fromEntries(Object.entries(batch).filter(([key]) => !["latch", "release", "timer"].includes(key)));
    return { inFlight, served, pending: pending.size, batch: state };
  }
  async function reading() {
    if (inFlight || batch && batch.completed !== batch.total) throw new Error("harness work is incomplete");
    let timer;
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("deferred_timeout")), deadlineMs); });
    try {
      while (pending.size) await Promise.race([Promise.all([...pending]), timeout]);
      if (deferredFailed || inFlight || batch && (batch.completed !== batch.total || batch.timedOut || batch.overflow)) throw new Error("harness work is incomplete");
      return snapshot();
    } finally { clearTimeout(timer); }
  }
  return { arm, enter, track, defer, snapshot, reading };
}

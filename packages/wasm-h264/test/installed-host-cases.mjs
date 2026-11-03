function check(condition, message) {
  if (!condition) throw new Error(message);
}

export async function runHostCases({ api, diagnostics, source, drain, env, fixture, options }) {
  const cases = [];
  async function reuse() {
    await drain(await api.thumbnail(source(fixture), options));
    const state = diagnostics();
    check(state.settled && !state.lease.active && state.host.liveBytes === 0, "benign operation did not settle");
    return true;
  }
  async function refused(input, kind) {
    let failure;
    try {
      await drain(await api.thumbnail(input, options));
    } catch (error) {
      failure = error;
    }
    check(failure instanceof api.CoreAVError && failure.document.kind === kind, "host fault did not return its structured error");
    check(diagnostics().settled, "host fault left work pending");
  }

  const short = api.r2Source({
    async get(key, request) {
      const { offset, length } = request.range;
      const response = await env.FIXTURES.fetch(`http://fixtures.test/${encodeURIComponent(key)}`, {
        headers: { range: `bytes=${offset}-${offset + length - 1}` },
      });
      check(response.status === 206, "short-read fixture was unavailable");
      const range = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(response.headers.get("content-range") ?? "");
      check(range !== null, "short-read fixture range was invalid");
      await response.body.cancel();
      return {
        body: new Response(new Uint8Array([0])).body,
        size: Number(range[3]),
        etag: response.headers.get("etag"),
        range: { offset },
      };
    },
  }, fixture);
  await refused(short, "io");
  cases.push({ id: "short-read", outcome: "io", complete: true, settled: true, benignReuse: await reuse() });

  let giant = new Uint8Array(new ArrayBuffer(4 * 1048576 + 1), 0, 1);
  await refused(api.bytesSource(giant), "limit");
  giant = null;
  cases.push({ id: "giant-backing", outcome: "limit", complete: true, settled: true, benignReuse: await reuse() });

  let release, entered;
  const hold = new Promise(resolve => { release = resolve; });
  const opening = new Promise(resolve => { entered = resolve; });
  const abortOpening = new AbortController();
  const running = api.thumbnail(source(fixture, { wait: hold, notify: entered, entered: false }), { ...options, signal: abortOpening.signal });
  running.catch(() => {});
  await opening;
  abortOpening.abort();
  check(diagnostics().lease.active, "opening abort released admission before I/O settled");
  let overlap;
  try { await api.probe(source(fixture)); } catch (error) { overlap = error; }
  check(overlap instanceof TypeError && /active operation/.test(overlap.message), "opening abort admitted overlapping work");
  release();
  let openingError;
  try { await running; } catch (error) { openingError = error; }
  check(openingError?.name === "AbortError" && diagnostics().settled, "opening abort did not settle as cancellation");
  await reuse();

  const abortOutput = new AbortController();
  const output = await api.thumbnail(source(fixture), { ...options, signal: abortOutput.signal });
  check(diagnostics().lease.active, "output backpressure did not retain admission");
  abortOutput.abort();
  let bodyError, summaryError;
  try { await output.body.getReader().read(); } catch (error) { bodyError = error; }
  try { await output.summary; } catch (error) { summaryError = error; }
  check(bodyError?.name === "AbortError" && summaryError?.name === "AbortError", "output abort did not cancel both result channels");
  check(diagnostics().settled, "output abort left work pending");
  cases.push({ id: "cancellation", outcome: "cancelled", stages: ["opening", "output-backpressure"], complete: true, settled: true, benignReuse: await reuse() });
  return { complete: true, cases };
}

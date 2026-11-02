import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export const nodeCases = ["opening-abort", "reading-abort", "output-abort", "cleanup-abort", "opening-deadline", "reading-deadline", "output-deadline", "cleanup-deadline", "read-error", "cleanup-error"];

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

export async function lifecycle(api, boundedOpeners, bytes, deadline = 5000) {
  const options = { at: "0", format: "jpeg", deadline };
  async function drain(output) {
    output.summary.catch(() => {});
    for await (const _chunk of output.body) {}
    return output.summary;
  }
  async function reuse() {
    const summary = await drain(await api.thumbnail(api.bytesSource(bytes), options));
    assert.equal(summary.schema, "coreav.thumbnail/1");
  }
  function controlled(hooks) {
    const input = api.bytesSource(bytes), open = boundedOpeners.get(input);
    boundedOpeners.set(input, async ({ signal }) => {
      hooks.signal = signal;
      signal.addEventListener("abort", () => hooks.stopped?.resolve(), { once: true });
      await hooks.opening?.();
      const session = await open();
      let pending;
      return {
        size: session.size,
        inMemory: false,
        read(offset, length) {
          pending = (async () => {
            await hooks.reading?.();
            const result = new Uint8Array(length);
            assert.equal(session.readInto(offset, result), length);
            return result;
          })();
          return pending;
        },
        async close() {
          await pending?.catch(() => {});
          try { await hooks.cleanup?.(); } finally { await session.close(); }
        },
      };
    });
    return input;
  }
  const cases = [];
  for (const mode of ["abort", "deadline"]) {
    for (const stage of ["opening", "reading", "output", "cleanup"]) {
      await reuse();
      const entered = deferred(), release = deferred(), stopped = deferred();
      const aborts = new AbortController();
      const hooks = { stopped };
      if (stage !== "output") {
        hooks[stage] = async () => { entered.resolve(); await release.promise; };
      }
      const input = controlled(hooks);
      let output;
      const running = (async () => {
        output = await api.thumbnail(input, { ...options, signal: aborts.signal });
        output.summary.catch(() => {});
        if (stage === "output") { entered.resolve(); await release.promise; }
        return drain(output);
      })();
      running.catch(() => {});
      await Promise.race([entered.promise, running.then(() => assert.fail("operation finished before its held stage"))]);
      if (mode === "abort") aborts.abort();
      await stopped.promise;
      if (stage !== "output") {
        await assert.rejects(api.probe(api.bytesSource(bytes)), error => error instanceof TypeError && /active operation/.test(error.message));
      }
      release.resolve();
      const expected = error => mode === "abort"
        ? error?.name === "AbortError"
        : error instanceof api.CoreAVError && error.document.kind === "limit" && error.document.limit === "timeout";
      await assert.rejects(running, expected);
      if (output) await assert.rejects(output.summary, expected);
      await reuse();
      cases.push(`${stage}-${mode}`);
    }
  }
  const marker = new Error("controlled-source-failure");
  await assert.rejects(api.thumbnail(controlled({ reading() { throw marker; } }), options), error => error instanceof api.CoreAVError && error.document.kind === "io");
  await reuse();
  cases.push("read-error");
  const output = await api.thumbnail(controlled({ cleanup() { throw marker; } }), options);
  await assert.rejects(drain(output), error => error instanceof api.CoreAVError && error.document.kind === "io");
  await assert.rejects(output.summary, error => error instanceof api.CoreAVError && error.document.kind === "io");
  await assert.rejects(api.probe(api.bytesSource(bytes)), /cleanup failed/);
  cases.push("cleanup-error");
  assert.deepEqual(cases, nodeCases);
  return { complete: true, cases };
}

async function main() {
  assert.equal(process.platform, "linux", "installed Node media verification is Linux-only");
  const [configPath, resultPath] = process.argv.slice(2);
  assert.ok(configPath && resultPath);
  const config = JSON.parse(readFileSync(configPath));
  const api = await import("@coreav/wasm-h264/node");
  const { boundedOpeners } = await import("./node_modules/@coreav/wasm-h264/src/bounded-source.js");
  assert.equal(api.moduleInfo().build, config.artifactSha256);
  const records = [];
  for (const row of config.rows) {
    const bytes = readFileSync(row.path), hash = createHash("sha256");
    assert.equal(createHash("sha256").update(bytes).digest("hex"), row.inputSha256);
    const output = await api.thumbnail(api.bytesSource(bytes), row.options);
    output.summary.catch(() => {});
    let length = 0;
    for await (const chunk of output.body) { hash.update(chunk); length += chunk.byteLength; }
    records.push({ id: row.id, complete: true, runtimeExit: false, outcome: "success", inputSha256: row.inputSha256, optionsSha256: row.optionsSha256, bytes: length, outputSha256: hash.digest("hex"), summary: await output.summary });
  }
  const report = await lifecycle(api, boundedOpeners, readFileSync(config.lifecyclePath));
  writeFileSync(resultPath, JSON.stringify({ artifactSha256: api.moduleInfo().build, records, lifecycle: report }) + "\n");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}

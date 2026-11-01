import {
  CoreAVError, errorResponse, hlsGet, hlsIndex, moduleInfo, r2Source, SourceNotFoundError
} from "@coreav/wasm/workers";
import { cache, indexIdentity, log, parsePath, readerOptions, Refusal } from "./indexes.js";

class UsageError extends Error {}

async function resource(index, name, source) {
  try { return await hlsGet(index, name, source); }
  catch (error) {
    if (error instanceof CoreAVError && error.kind === "unsupported_feature" && error.document.message === "no such track") throw new Refusal(404, "not found");
    throw error instanceof TypeError ? new UsageError(error.message) : error;
  }
}

async function serve(request, env, ctx) {
  const { key, name, config } = parsePath(request, env);
  const head = await env.MEDIA.head(key);
  if (head === null) throw new Refusal(404, "not found", "not_found");
  if (typeof head.etag !== "string" || head.etag.length > 255) throw new Refusal(502, "the object's ETag cannot be pinned", "source_unreadable");
  const media = { get: (key, options = {}) => env.MEDIA.get(key, { ...options, onlyIf: options.onlyIf ?? { etagMatches: head.etag } }) };
  const reader = readerOptions(config);
  const identity = await indexIdentity({ key, etag: head.etag, build: moduleInfo().build, options: config.indexOptions });
  const indexFor = (replace) => cache.get(identity, {
    stored: env.INDEXES ?? null,
    buildIndex: async () => {
      try { return (await hlsIndex(r2Source(media, key, reader), { ...config.indexOptions, tag: head.etag })).index; }
      catch (error) { throw error instanceof TypeError ? new UsageError(error.message) : error; }
    },
    waitUntil: (promise) => ctx.waitUntil(promise),
    replace
  });
  const source = r2Source(media, key, reader);
  const index = await indexFor(undefined);
  let output;
  try { output = await resource(index, name, source); }
  catch (error) {
    if (!(error instanceof CoreAVError) || error.kind !== "invalid_index") throw error;
    output = await resource(await indexFor(index), name, source);
  }
  ctx.waitUntil(output.summary.then(() => log("ok"), (error) => log(error instanceof CoreAVError && error.kind === "io" ? "source_unreadable" : "internal")));
  return new Response(output.body, { headers: { "content-type": output.contentType } });
}

function respond(error) {
  if (error instanceof Refusal) { log(error.outcome); return Response.json({ error: error.message }, { status: error.status, headers: error.headers }); }
  if (error instanceof UsageError) { log("usage"); return Response.json({ error: error.message }, { status: 400 }); }
  if (error instanceof SourceNotFoundError) { log("not_found"); return Response.json({ error: "no such object" }, { status: 404 }); }
  if (error instanceof CoreAVError) {
    if (error.kind === "io") { log("source_unreadable"); return Response.json({ error: "the input could not be read" }, { status: 502 }); }
    log("rejected");
    return errorResponse(error);
  }
  log("internal");
  return Response.json({ error: "internal error" }, { status: 500 });
}

export default {
  async fetch(request, env, ctx) {
    let response;
    try { response = await serve(request, env, ctx); }
    catch (error) { response = respond(error); }
    response.headers.set("access-control-allow-origin", env.CORS_ORIGIN || "*");
    return response;
  }
};

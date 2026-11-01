import {
  bytesSource, check, chapters, clip, CoreAVError, errorResponse, extractAudio,
  extractSubtitles, ffprobeText, httpSource, probeText, r2Source, remux,
  sanitize, SourceNotFoundError
} from "@coreav/wasm/workers";
import policy from "./policy.json";
import { parseRequest, readerOptions, Refusal, log } from "./input.js";

class UsageError extends Error {}

async function usage(call) {
  try { return await call(); }
  catch (error) { throw error instanceof TypeError ? new UsageError(error.message) : error; }
}

function outcome(error) {
  if (error instanceof Refusal) return "refused";
  if (error instanceof UsageError) return "usage";
  if (error instanceof SourceNotFoundError) return "not_found";
  if (error instanceof CoreAVError) return error.kind === "io" ? "source_unreadable" : "rejected";
  return "internal";
}

function streamed(operation) {
  return async (source, options, ctx, route) => {
    const output = await usage(() => operation(source, options));
    ctx.waitUntil(output.summary.then(() => log(route, "ok"), (error) => log(route, outcome(error))));
    return new Response(output.body, { headers: { "content-type": output.contentType } });
  };
}

const RUN = {
  probe: async (source, options) => new Response(await usage(() => probeText(source, options)), { headers: { "content-type": "application/json" } }),
  ffprobe: async (source) => new Response(await usage(() => ffprobeText(source)), { headers: { "content-type": "application/json" } }),
  check: async (source, options) => {
    const verdict = await usage(() => check(source, options));
    return Response.json(verdict, { status: verdict.verdict === "accept" ? 200 : 422 });
  },
  chapters: async (source, options) => Response.json(await usage(() => chapters(source, options))),
  "extract-audio": streamed(extractAudio),
  "extract-subtitles": streamed(extractSubtitles),
  clip: streamed(clip),
  remux: streamed(remux),
  sanitize: streamed(sanitize)
};

function sourceFor({ input, config }, env) {
  const reader = readerOptions(config);
  if (input.kind === "key") return r2Source(env.MEDIA, input.key, reader);
  if (input.kind === "url") return httpSource(input.url.href, reader);
  return bytesSource(input.bytes);
}

function respond(error) {
  if (error instanceof Refusal) return Response.json({ error: error.message }, { status: error.status, headers: error.headers });
  if (error instanceof UsageError) return Response.json({ error: error.message }, { status: 400 });
  if (error instanceof SourceNotFoundError) return Response.json({ error: "no such object" }, { status: 404 });
  if (error instanceof CoreAVError) {
    if (error.kind === "io") return Response.json({ error: "the input could not be read" }, { status: 502 });
    return errorResponse(error);
  }
  return Response.json({ error: "internal error" }, { status: 500 });
}

export default {
  async fetch(request, env, ctx) {
    let route = "unknown";
    try {
      const parsed = await parseRequest(request, env);
      route = parsed.route;
      const options = route === "check" ? { ...parsed.options, policy: parsed.config.policyText ?? policy } : parsed.options;
      const response = await RUN[route](sourceFor(parsed, env), options, ctx, route);
      if (!["extract-audio", "extract-subtitles", "clip", "remux", "sanitize"].includes(route)) log(route, response.status === 422 ? "rejected" : "ok");
      return response;
    } catch (error) {
      log(route, outcome(error));
      return respond(error);
    }
  }
};

import { runToStream } from "./engine.js";
import { thumbnailRequest } from "./thumbnail-options.js";

export async function thumbnail(source, options = {}) {
  const shaped = thumbnailRequest(options);
  const { body: body, summary: summary } = await runToStream("thumbnail", source, shaped.request);
  return { body: body, summary: summary, contentType: shaped.contentType };
}

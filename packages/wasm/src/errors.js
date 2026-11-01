/**
 * The input was rejected or could not be read: a `coreav.error/1` object,
 * as `coreav` prints it, in `document`, with its fields copied for
 * convenience. `documentText` retains the original JSON number tokens.
 * When a source failed, `cause` is the source's error.
 */
export class CoreAVError extends Error {
  constructor(document, options) {
    const documentText = typeof document === "string" ? document : JSON.stringify(document);
    if (typeof document === "string") document = JSON.parse(document);
    super(document.message, options);
    this.name = "CoreAVError";
    this.document = document;
    this.documentText = documentText;
    this.kind = document.kind;
    this.limit = document.limit;
    this.offset = document.offset;
  }
}

/** The object behind a source changed while it was read. */
export class SourceChangedError extends Error {
  constructor(message = "the source object changed while it was read") {
    super(message);
    this.name = "SourceChangedError";
  }
}

/** The source object does not exist. Adapters do not put a URL or key in the message. */
export class SourceNotFoundError extends Error {
  constructor(message = "the source object does not exist") {
    super(message);
    this.name = "SourceNotFoundError";
  }
}

/** A source exceeded its request cap (`maxRequests`). */
export class RequestLimitError extends Error {
  constructor(limit) {
    super(`the source needed more than ${limit} requests (maxRequests)`);
    this.name = "RequestLimitError";
    this.limit = limit;
  }
}

/** Map usage, missing-source and CoreAV errors to 400, 404, 422 or 502. Rethrows unrelated errors. */
export function errorResponse(error) {
  if (error instanceof TypeError) {
    return Response.json({
      error: error.message
    }, {
      status: 400
    });
  }
  if (error instanceof SourceNotFoundError) {
    return Response.json({
      error: error.message
    }, {
      status: 404
    });
  }
  if (error instanceof CoreAVError) {
    const status = error.kind === "io" ? 502 : 422;
    return new Response(error.documentText, {
      status: status,
      headers: {
        "content-type": "application/json"
      }
    });
  }
  throw error;
}

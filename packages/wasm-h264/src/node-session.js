import { boundedOpeners } from "./bounded-source.js";
import { CoreAVError, SourceNotFoundError } from "./errors.js";
import { TRUSTED } from "./internal.js";

export function brokerSource(port) {
  const session = {
    [TRUSTED]: true,
    error: null,
    size: 0,
    inMemory: false,
    bytes: null,
    readInto(offset, view) {
      if (!this.inMemory || this.bytes === null) return 0;
      const slice = this.bytes.subarray(offset, offset + view.length);
      view.set(slice);
      return slice.length;
    },
    read(offset, length) {
      return request(port, { type: "read", offset: offset, length: length }).then((message) => {
        if (!(message.bytes instanceof Uint8Array) || message.bytes.length !== length) {
          throw new Error("the source returned the wrong number of bytes");
        }
        return message.bytes;
      }).catch((error) => {
        this.error = error;
        throw error;
      });
    },
    plan(offset, length) {
      port.postMessage({ type: "plan", offset: offset, length: length });
    },
    planRegions(regions) {
      port.postMessage({ type: "plan-regions", regions: regions });
    },
    close() { this.bytes = null; },
  };
  const source = { async open(open) {
    const opened = await request(port, { type: "need-open", open: open ?? null });
    if (!Number.isSafeInteger(opened.size) || opened.size < 0) {
      throw new TypeError("the source session has no valid size");
    }
    session.size = opened.size;
    session.inMemory = opened.inMemory === true;
    session.bytes = opened.bytes ?? null;
    return session;
  } };
  boundedOpeners.set(source, source.open);
  return source;
}

function request(port, message) {
  const id = nextId();
  return new Promise((resolve, reject) => {
    const onMessage = (reply) => {
      if (reply?.id !== id) return;
      port.off("message", onMessage);
      if (reply.type === "failed") reject(restoreError(reply.error));
      else resolve(reply);
    };
    port.on("message", onMessage);
    port.postMessage({ ...message, id: id });
  });
}

let sequence = 0;
function nextId() {
  sequence += 1;
  return sequence;
}

export function serializeError(error) {
  if (error instanceof CoreAVError) {
    const cause = error.cause instanceof Error ? { name: error.cause.name, message: error.cause.message } : null;
    return { name: "CoreAVError", documentText: error.documentText, cause: cause };
  }
  if (error instanceof TypeError) return { name: "TypeError", message: error.message };
  if (error?.name === "AbortError") return { name: "AbortError", message: error.message };
  return { name: error?.name ?? "Error", message: error?.message ?? "the operation failed" };
}

export function restoreError(error) {
  if (error?.name === "SourceNotFoundError") return new SourceNotFoundError(error.message);
  if (error?.name === "CoreAVError" && typeof error.documentText === "string") {
    const cause = error.cause ? Object.assign(new Error(error.cause.message), { name: error.cause.name }) : undefined;
    return new CoreAVError(error.documentText, cause ? { cause: cause } : undefined);
  }
  if (error?.name === "TypeError") return new TypeError(error.message);
  if (error?.name === "AbortError") return new DOMException(error.message, "AbortError");
  return Object.assign(new Error(error?.message ?? "the operation failed"), { name: error?.name ?? "Error" });
}

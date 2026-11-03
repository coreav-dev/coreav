import { CoreAVError } from "./errors.js";
import { SourceCleanupError } from "./bounded-source.js";

export const DEADLINE_MS = 5000;

export function splitHostOptions(options) {
  if (options === null || typeof options !== "object" || Array.isArray(options)) {
    throw new TypeError("options must be an object");
  }
  const deadline = options.deadline;
  let ms = DEADLINE_MS;
  if (deadline !== undefined) {
    if (!Number.isSafeInteger(deadline) || deadline < 1 || deadline > DEADLINE_MS) {
      throw new TypeError("deadline must be an integer from 1 to 5000 milliseconds");
    }
    ms = deadline;
  }
  const { signal: signal, deadline: _deadline, ...rest } = options;
  if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError("signal must be an AbortSignal");
  return { ms: ms, signal: signal, options: rest };
}

export function deadlineDocument() {
  return {
    schema: "coreav.error/1",
    kind: "limit",
    limit: "timeout",
    offset: null,
    message: "the operation exceeded its deadline",
  };
}

export function deadlineError() {
  return new CoreAVError(deadlineDocument());
}

export function abortError(signal) {
  const error = new DOMException("The operation was aborted", "AbortError");
  if (signal?.reason !== undefined) Object.defineProperty(error, "cause", { value: signal.reason });
  return error;
}

export function createAdmission() {
  let held = false;
  let poison = null;
  return {
    get held() {
      return held;
    },
    poisonWith(error) {
      poison ??= error;
    },
    acquire() {
      if (poison) throw poison;
      if (held) throw new TypeError("the CoreAV module already has an active operation");
      held = true;
      let released = false;
      return () => {
        if (released) return;
        released = true;
        held = false;
      };
    },
  };
}

function closeFailure(cause) {
  if (cause instanceof CoreAVError) return cause;
  return new CoreAVError({
    schema: "coreav.error/1",
    kind: "io",
    limit: null,
    offset: null,
    message: "the source could not be closed",
  }, { cause });
}

export function runBounded(admission, options, deps, exec) {
  const result = runBoundedBody(admission, options, deps, exec);
  result.catch(() => {});
  return result;
}

async function runBoundedBody(admission, options, deps, exec) {
  const host = splitHostOptions(options);
  if (host.signal?.aborted) throw abortError(host.signal);
  const release = admission.acquire();
  const openAbort = new AbortController();
  let releaseNow = true;
  let stopping = null;
  let termination = null;
  let opening = null;
  let session = null;
  let cleanupPromise = null;
  let cleanupFailure = null;
  let timer = null;
  const output = { fail() {} };
  const stopHandlers = [];
  const finalizers = [];

  function beginCleanup() {
    if (cleanupPromise) return cleanupPromise;
    const work = performCleanup();
    work.catch((error) => {
      cleanupFailure ??= error instanceof CoreAVError ? error : closeFailure(error);
    });
    cleanupPromise = work.then(() => {}, () => {});
    return cleanupPromise;
  }

  function stop(error) {
    if (stopping) return cleanupPromise;
    stopping = error;
    openAbort.abort(error);
    try {
      termination = Promise.resolve(deps.terminate(error));
    } catch (cause) {
      termination = Promise.reject(cause);
    }
    output.fail(error);
    for (const handler of stopHandlers) handler(error);
    return beginCleanup();
  }

  async function performCleanup() {
    let failure = null;
    try {
      if (opening) {
        try {
          const opened = await opening;
          if (opened) session = opened;
        } catch (error) {
          if (error instanceof SourceCleanupError) failure = error;
        }
      }
      if (session) await session.close();
    } catch (cause) {
      failure ??= cause;
    } finally {
      if (termination) {
        try { await termination; } catch (cause) { failure ??= cause; }
      }
      for (const finish of finalizers) {
        try { await finish(); } catch (cause) { failure ??= cause; }
      }
      deps.clear(timer);
      host.signal?.removeEventListener?.("abort", onAbort);
      if (failure !== null) {
        cleanupFailure = closeFailure(failure);
        admission.poisonWith(new Error(
          "CoreAV source cleanup failed and the instance cannot run again",
          { cause: cleanupFailure },
        ));
      }
      release();
    }
  }

  const onAbort = () => {
    stop(abortError(host.signal)).catch(() => {});
  };
  timer = deps.schedule(host.ms, () => {
    stop(deadlineError()).catch(() => {});
  });
  host.signal?.addEventListener?.("abort", onAbort, { once: true });
  try {
    const result = await exec({
      options: host.options,
      onStop(handler) {
        stopHandlers.push(handler);
        if (stopping) handler(stopping);
      },
      deferCleanup(finish) {
        finalizers.push(finish);
      },
      get stopped() { return stopping !== null; },
      open(arg) {
        if (stopping) return Promise.reject(stopping);
        if (opening === null) {
          const openArg = arg && typeof arg === "object" ? { ...arg, signal: openAbort.signal } : { signal: openAbort.signal };
          opening = Promise.resolve().then(() => deps.open(openArg));
        }
        return opening;
      },
      bind(target) {
        output.fail = (error) => target.fail(error);
        if (stopping) target.fail(stopping);
      },
    });
    if (stopping) throw stopping;
    if (result?.pending) {
      releaseNow = false;
      const completed = Promise.resolve(result.pending).then(
        async () => {
          await beginCleanup();
          if (cleanupFailure) throw cleanupFailure;
          if (stopping) throw stopping;
        },
        async (error) => {
          await beginCleanup();
          throw cleanupFailure ?? stopping ?? error;
        },
      );
      completed.catch(() => {});
      if (result.value?.body?.getReader) {
        Promise.resolve(result.value.summary).catch(() => {});
        const summary = completed.then(() => result.value.summary);
        summary.catch(() => {});
        return { ...result.value, body: afterCleanup(result.value.body, completed), summary };
      }
    }
    return result?.value;
  } finally {
    if (releaseNow) {
      await beginCleanup();
      if (cleanupFailure) throw cleanupFailure;
      if (stopping) throw stopping;
    }
  }
}

function afterCleanup(body, completed) {
  const reader = body.getReader();
  let released = false;
  const release = () => {
    if (released) return;
    released = true;
    reader.releaseLock();
  };
  return new ReadableStream({
    async pull(controller) {
      try {
        const chunk = await reader.read();
        if (!chunk.done) { controller.enqueue(chunk.value); return; }
        await completed;
        release();
        controller.close();
      } catch (error) {
        release();
        controller.error(error);
      }
    },
    async cancel(reason) {
      await Promise.allSettled([reader.cancel(reason), completed]);
      release();
    },
  }, { highWaterMark: 0, size: (chunk) => chunk.byteLength });
}

# @coreav/wasm

CoreAV's probe and stream-copy operations as a JavaScript library over one WebAssembly module. This package is private and is not published to npm.

## Build

Install the `wasm32-unknown-unknown` Rust target and the `wasm-bindgen-cli` version pinned in `crates/coreav-wasm/Cargo.toml`, then run `npm run build` here. The build writes the generated glue and module to `wasm/` and copies the repository's licenses and notice into the package. `wasm-opt` is optional; the build reports when it is absent.

The module needs WebAssembly JSPI, exception references and SIMD. Node.js 24 needs `--experimental-wasm-jspi --experimental-wasm-exnref`. There is no WASI fallback. Browsers are not currently supported.

`moduleInfo()` reports the version, current linear memory size and the SHA-256 `build` fingerprint of the packaged WASM artifact. The fingerprint follows the final bytes after optional optimization, not the version number. A root caller must supply the module matching this package's generated glue and fingerprint.

## Choose an entry

The root makes no file or network requests and does not initialize itself.

```js
import { init, probeText, bytesSource } from "@coreav/wasm";

init(compiledModule);
const text = await probeText(bytesSource(bytes));
```

Use `@coreav/wasm/node` to load the built module from the package's files. Use `@coreav/wasm/workers` when the runtime's bundler imports `.wasm` as a compiled `WebAssembly.Module`. Both entries initialize the same engine and export the root API. Only the Workers entry also exports `r2Source` and `R2_DEFAULTS`.

```js
import { ffprobeText, r2Source } from "@coreav/wasm/workers";

const text = await ffprobeText(r2Source(bucket, "input.mp4"));
const response = new Response(text, {
  headers: { "content-type": "application/json" }
});
```

`probe` returns CoreAV's native document. `ffprobe` returns the fixed `coreav ffprobe -of json -show_format -show_streams` document, not an arbitrary ffprobe command. `probe` accepts `allow`, `strict`, `limits`, `scanFragments` and `scanFrameSizes`; fragment scanning is off by default. `ffprobe` accepts only `filename` and `limits`; the filename defaults to `-`. Pass a display label, never a signed URL, as the filename.

`probeText` and `ffprobeText` preserve the module's exact JSON text, including integer tokens above JavaScript's safe integer range. The parsed variants can round those integers.

## Sources and outputs

`bytesSource` reads bytes in memory. `rangeSource` adapts a range-fetch function. `httpSource` reads an HTTP object; redirects are refused by default. Range sessions pin a strong ETag and size. Without a strong ETag, size checks cannot detect same-size content changes. `r2Source` uses an R2 bucket binding, not an SDK or credentials.

`clip`, `extractAudio`, `extractSubtitles`, `remux`, `sanitize` and `hlsGet` return `{ body, summary, contentType }`. Consume or forward `body` before awaiting completion through `summary`; otherwise backpressure can hold the operation. Cancelling the body ends its session. An error before the first byte rejects the call; an error after that errors the body and rejects the summary. `check`, `chapters` and `hlsIndex` return documents or index bytes.

Reader limits apply per session, not to a whole isolate. WASM memory does not shrink, concurrent operations add their allocations, and `hlsIndex` buffers the completed index. The 40 MiB reader bound used by `remux` and `sanitize` does not guarantee a 128 MiB isolate bound. Applications own upload limits, URL policy, authorization, tenant separation, safe error responses and request budgets. Low-level source errors can retain their original cause; do not return arbitrary network errors or log signed URLs.

## Verification

`npm test` runs source, HTTP, R2-binding and error contracts without building WebAssembly. After building, set `COREAV_FIXTURES` to the output of the repository's `scripts/gen-fixtures.sh` and run `npm run test:module` with Node.js 24. Missing fixtures fail rather than skipping the module tests.

The native npm CLI and its WASI fallback remain separate in `packages/cli`. See [the design note](../../docs/design/wasm.md) for operation coverage and boundaries.

# @coreav/wasm-h264

Private H.264 thumbnail operations for servers, over one WebAssembly module built with `native-h264` on `coreav-wasm`. This package is not published. It does not load `@coreav/wasm`.

The root, Node and Workers entries keep the same initialization and source contracts as `@coreav/wasm`. `thumbnail(source, options)` returns `{ body, summary, contentType }`. `at` is required decimal seconds. Width is 320 only when neither dimension is set. `signal` is not sent to the module. Unknown options and enum values are rejected by the module.

`@coreav/wasm-h264/node` runs that module on a worker thread. A 5,000 ms wall deadline starts before source opening and stays armed through response completion and source cleanup. An application may pass a lower integer `deadline`, in milliseconds. The deadline or `signal` terminates the worker and closes the source session. Admission remains held until termination and cleanup settle. Abandoning the returned promise does not terminate the worker or release admission. The deadline does not interrupt a synchronous WebAssembly call by racing it; termination discards that instance.

The worker starts without the parent's Node entrypoint flags or preload hooks. Process-wide V8 flags, including JSPI and exception-reference support, still apply. Both missing runtime features are checked inside the worker.

`@coreav/wasm-h264/workers` loads the same module in the isolate. It has no in-isolate watchdog. Build with `npm run build` from this directory, which uses the shared compiler, bindings generator and Binaryen pin and writes the raw module under `target/wasm-h264/` so it does not replace the default module.

The H.264 module uses Rust's `opt-level=z` to meet the unchanged 1,048,576-byte gzip limit. Its notice record captures that flag; the default module and native reference keep their existing optimization. This trades optimization for size, not a measured speed improvement.

The bounded route accepts the package's fixed `bytesSource` inputs up to 4 MiB of backing capacity and genuine platform HTTP/R2 range bodies. Custom producers, caller-retained duplicates and unrelated caches do not qualify for its memory envelope. JPEG, PNG and lossless WebP use the same native encoders. Output sides are at most 1,280 pixels with at most 921,600 pixels in total; encoded output is at most 4 MiB. Defaults are quality 91, bicubic and `chroma: "native"`. Other filter values are `bilinear` and `area`; `chroma: "full"` uses native full chroma conversion.

`limits` may lower parser, decoder, planning and output ceilings, never raise them. `maxMemoryBytes` is the decoder budget, not a promise about process RSS or total isolate memory. The eleven memory gates are separate from the default package's sixteen gates. Hosted Workers memory, the runtime reserve and release readiness remain unvalidated; this package publishes no hosted-support claim.

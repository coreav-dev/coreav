# WebAssembly JavaScript library

`packages/wasm` is the private `@coreav/wasm` package. It hosts the existing `crates/coreav-wasm` module without creating a process. It is separate from the native npm CLI and its Node WASI fallback in [npm-cli.md](npm-cli.md). Publishing is not part of this package addition.

## Ownership and entries

Rust owns parsing, option validation, limits, documents and output writing. The JS host owns initialization, source sessions, range requests, error causes and output backpressure. There is one wasm-bindgen build and one engine per module instance.

| Entry | Responsibility |
| --- | --- |
| `@coreav/wasm` | Runtime-neutral exports and explicit synchronous `init(module)`; no automatic file or network access. |
| `@coreav/wasm/workers` | Imports a compiled `.wasm` module, initializes the engine and adds the R2 binding adapter. |
| `@coreav/wasm/node` | Reads the module's bytes from the package files and initializes the same engine. |

The specifiers are explicit, not conditional exports. Common sources and types do not depend on Workers or Node. `api.js`, `engine.js`, `errors.js` and `sources.js` are shared; `r2.js` is re-exported only by the Workers entry.

`init` accepts a compiled `WebAssembly.Module` or its bytes. It initializes once; later calls do not replace the instance. A trap marks the instance unusable for subsequent operations. Recovery needs a new isolate or module instance, not another call to `init`.

The module requires JSPI, exception references and SIMD. Node.js 24 uses `--experimental-wasm-jspi --experimental-wasm-exnref`. The synchronous in-memory document export does not imply that the whole module can initialize without JSPI. Browser loading and browser support are deferred. There is no automatic fallback to WASI.

## Operations

| JavaScript API | Command and result |
| --- | --- |
| `probe`, `probeText` | `coreav probe`, the native probe document. |
| `ffprobe`, `ffprobeText` | Fixed `coreav ffprobe -of json -show_format -show_streams` document. |
| `check` | `coreav check`, the policy verdict. |
| `chapters` | `coreav chapters`, chapter document. |
| `clip` | `coreav clip`, keyframe-aligned MP4 output. |
| `extractAudio` | `coreav extract-audio`, AAC stream-copy output. |
| `extractSubtitles` | `coreav extract-subtitles`, WebVTT output. |
| `remux`, `sanitize` | The corresponding stream-copy commands and summaries. |
| `hlsIndex`, `hlsGet` | HLS index bytes and summary, or one resource cut from an index. |

Native probe, check and remux have MP4/MOV and Matroska/WebM paths. `ffprobe` also has MPEG-TS support. The remaining operations have their own input restrictions; the presence of a probe path does not make every operation support that container.

`probe` options are `allow`, `strict`, `limits`, `scanFragments` and `scanFrameSizes`. Fragment scanning is off by default. It accepts neither `format` nor `filename`. Its Matroska path uses the library's limits; its explicit limit overrides apply to MP4/MOV.

`ffprobe` accepts `filename` and `limits`, with `filename` defaulting to `-`. Its limits reach each container. It does not accept arbitrary argv, packet/frame listings or another output format. Applications must use a label rather than a credential-bearing URL as the filename.

Rust validates operation options. JS passes them through rather than duplicating Rust's field lists. Decimal times are passed as text. `probeText` and `ffprobeText` return the module's JSON unchanged. Their parsed variants use `JSON.parse`, which can round integers above 2^53. `CoreAVError.documentText` likewise preserves the original error text.

## Source sessions

Each operation opens a new session. A memory session has synchronous `readInto`; a remote session has asynchronous `read`, optional cached `readNow`, and advisory `plan` and `planRegions`. Sessions expose a safe nonnegative integer size and can be closed. The engine validates foreign session reads before handing bytes to the module.

`bytesSource` serves existing buffers. `rangeSource` owns the shared reader, pipelines, cache, retries, cancellation and counters. `httpSource` adapts fetch responses; `r2Source` adapts only the R2 binding's `get` method. There is no SDK dependency or secret lookup.

Strong HTTP ETags are pinned with `If-Match`; R2 reads use `onlyIf.etagMatches` after the first response. Size changes are also rejected. Without a strong ETag, same-size replacement cannot be detected. HTTP redirects are refused unless explicitly enabled. This does not implement an application's SSRF policy or the native host's DNS-address policy.

The HTTP adapter refuses userinfo in URLs, requests `Accept-Encoding: identity`, and rejects compressed responses before reading their bodies. Fetch can decode a response whose length and range headers still describe compressed bytes, so accepting it would silently change the input size. Redirects are followed manually, at most five per range request, with no HTTPS downgrade. Caller headers are preserved on same-origin hops and permanently removed for the remainder of a chain after its first cross-origin hop. The adapter owns `Range`, `If-Match` and `Accept-Encoding` on each hop.

`maxRequests` counts range-reader requests and retries, not redirect hops. With redirects enabled, each reader request can make up to six platform subrequests. Reader statistics describe the latest closed session. The defaults are session settings, not a deployment-wide subrequest quota.

## Streaming and memory

An output operation returns after the first bytes are available. The host copies module memory views before yielding and queues approximately 1 MiB before waiting for the consumer. Reading or forwarding `body` allows `summary` to finish. Cancelling the body closes the session and releases pending reads. Failures before the first write reject the call; later failures error the body and reject the summary.

The shared reader defaults retain up to 48 MiB across forward buffers, recent bytes and the cache. R2 raises this to 72 MiB. `remux` and `sanitize` open their readers with a 40 MiB bound because output sample tables also occupy memory, up to the module's 64 MiB table limit. A source's stricter reader bound still wins.

These are not isolate-wide guarantees. Response bodies, output queues, index copies and JS objects also occupy memory. WASM linear memory never shrinks, so an earlier operation's high-water mark matters for later ones. Concurrent sessions add their allocations. `hlsIndex` buffers the entire completed index and `hlsGet` copies index bytes into the module. No 128 MiB safety claim follows from a per-session reader bound.

## Errors and application policy

`CoreAVError` carries a `coreav.error/1` document, exact `documentText` and an optional source/sink cause. Usage errors are `TypeError`. Missing objects use `SourceNotFoundError`; object changes and request caps use `SourceChangedError` and `RequestLimitError` as causes of I/O errors.

`errorResponse` maps usage errors to 400, missing objects to 404, parser rejections to 422 and I/O errors to 502; unrelated errors are rethrown. HTTP/R2 missing-object errors do not include the URL or key. For host I/O failures, the original exception stays only in `cause`; it is not copied to the public error message or response document. Applications must not log arbitrary causes.

Upload byte caps, HTTPS/host/userinfo policy, authorization, tenant scoping and HLS cache identity are application responsibilities. Example applications and their security tests are a separate change, not a blanket permission to expose arbitrary remote inputs.

## Application examples

The [Worker examples](../../packages/wasm/examples/README.md) show these application boundaries without adding them to the library. One media Worker owns the input policy for every document and stream-copy route. A separate HLS Worker serves only R2 objects. Each refuses requests until its authentication hook is replaced; anonymous access is enabled only by the local test harness.

Uploads count actual bytes, URLs require an explicitly allowed HTTPS host without credentials, and response messages and outcome logs do not copy arbitrary host exceptions. Every remote source explicitly sets a 40 MiB reader bound. HLS cache identities include the tenant-prefixed key, ETag, options and exact WASM fingerprint, with separate hash-named stored records. Admission precedes stored reads; retained and reserved bytes share an 8 MiB cap, with at most 16 ready or loading entries and one build. Rebuilds distinguish stored bytes from freshly built bytes and keep one loading owner per identity.

These are retention limits, not isolate-wide memory guarantees. A completed index is generated before its size can be refused, serving copies it into WASM, pending writes and evicted in-use arrays can retain bytes, and warm memory does not shrink. Local workerd checks compare synthetic fixtures with the native operations and record memory without a memory acceptance bar. Deployment, publishing and real R2 validation are outside these examples.

## Build and verification

The build reads the wasm-bindgen pin from `crates/coreav-wasm/Cargo.toml`, compiles profile `edge` for `wasm32-unknown-unknown` with SIMD, and generates web-target glue. It refuses `CARGO_ENCODED_RUSTFLAGS`, which would override the target flags. `wasm-opt` is optional and its absence is reported. After optimization, the build writes a SHA-256 fingerprint of the exact module bytes. `moduleInfo().build` exposes this packaged-artifact fingerprint; a root caller must supply the corresponding module, not an arbitrary compatible build. The prepack check recomputes the hash and refuses stale metadata. Generated artifacts and license copies are ignored by Git; the package includes the two licenses, `NOTICE` and fingerprint when packed.

The contract tier runs byte-reader, local HTTP, mocked R2-binding, fingerprint and error tests without a module build. The module tier needs Node.js 24, the JSPI flags, the generated module and generated fixtures; missing fixtures are failures. It checks initialization, exact document text, operation parity between memory and ranges, output cancellation and concurrent sessions. The path-filtered, non-required `wasm.yml` workflow runs contracts on package changes and the module tier on ready pull requests, main pushes and manual runs, with one Linux environment and no credentials. Draft pull requests do not run the module build.

Tarball installation outside the repository, Workers bundling, type consumption against Workers declarations, the optimized size gate and full native-output parity are separate follow-ups. No size, timing or isolate-memory result is claimed here.

## Alternatives

Separate runtime packages would duplicate versions and contracts without independent release needs. Conditional root exports would make one import execute different loaders and make its declarations ambiguous. A factory/plugin API would add instance-replacement behavior that no current caller needs. Replacing the JS range planner with Rust would change cache placement, copies and suspension points; it is a separate measured change, not part of this host migration.

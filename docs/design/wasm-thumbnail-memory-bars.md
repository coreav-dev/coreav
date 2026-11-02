# WASM thumbnail plan and registered memory bars

Status: plan only. This document fixes the staged API, allocation admission contract and gates before implementation or a judged run. Merging it enables no runtime code, package, feature, workflow or publishing. The existing sixteen gates in [wasm-memory-bars.md](wasm-memory-bars.md) stay unchanged. Implementation follows in a separate change after this plan merges.

## Packages and stages

Servers are the first target, not a restriction on users. Stage 1 adds `@coreav/wasm-h264`, built with the explicit `native-h264` feature on `coreav-wasm`; default `@coreav/wasm` stays decoder-free. Enable the H.264 backend directly, not `coreav-cli/native-h264`, which also selects worker and transcode code. Each package has one module instance, its own fingerprint and notices generated from its exact target/feature closure. Loading H.264 must not also instantiate the default module. Host code keeps one shared source owner.

One package carries one codec. Later `@coreav/wasm-vp9` waits for G4, aggregate memory accounting with fallible allocations, then milestone B's decode-boundary backend, in that order. It accepts profile 0 only until milestone 5. There is no temporary thumbnail-only VP9 adapter. Stage 1 refuses VP9, AV1 and other unavailable codecs with a structured unsupported error. AV1 has no CoreAV-native decoder; external dav1d is not added. No general decoding or transcoding API is added. Publishing either package is a separate release decision.

Stage 2 adds a browser-only WebCodecs path in the default package after stage 1: CoreAV selects and extracts the keyframe, and the browser decodes it without a linked decoder. Its API and parity contract appear below. Node or Workers support does not establish browser support.

## Stage 1 API and selection

The root, `/node` and `/workers` entries of `@coreav/wasm-h264` retain the explicit initialization and Source contracts of [wasm.md](wasm.md). `thumbnail(source, options)` returns the existing Output shape: `body`, `summary` and `contentType`. Options are required decimal-seconds `at`, optional `stream`, `width`, `height`, `format`, `quality`, `filter`, `chroma`, lower-only `limits` and `signal`. Unknown options and unsupported enum values are rejected.

Defaults are JPEG quality 91, bicubic resampling and native FFmpeg-compatible chroma handling. Width defaults to 320 only when neither dimension is supplied; height alone derives width, and both dimensions request that size. This is a bounded thumbnail default, unlike the CLI's source-size default. Native equality uses the same resolved dimensions. JPEG, PNG and lossless WebP retain the native encoders; quality is 1 through 100 and affects JPEG only. Rotation, aspect ratio, colour and filtering stay in the shared native pipeline. Failure before output rejects the call; later failure errors body and summary. Arbitrary host exceptions remain only in `cause`.

The only selection owner is `coreav-thumbnail`'s existing `planned`/`choose` path and `select::offered`. Introduce a narrow internal preparation boundary returning the selected access unit, resolved configuration, geometry and presentation identity. Do not expose the test-only oracle API, list the whole stream as a new public primitive or implement selection in JS. Selection and extraction share one source session and its object-version checks.

First make the unchanged native keyframe choice. Then require `start == target`, `select::offered(target).is_some()`, and one complete IDR picture with intra slices only. Refuse non-IDR/recovery-point choices, mixed predicted slices, multiple pictures concealed in a packet, and exact/inter-frame fallback. Do not search for another IDR or return another time. `exact: true` is unsupported. This accepted subset permits a one-picture allocation contract and satisfies WebCodecs' AVC key-chunk requirement. The native build of this operation uses the same subset; the unrestricted CLI can accept more files.

## Allocation admission

A limit miss returns a structured `limit` error before the corresponding allocation. JSON options may lower but never raise the profile. Required protections below are implementation prerequisites, not claims about existing code.

| Owner | Contract |
| --- | --- |
| Planning | Charge retained container metadata, indexes, cloned configurations, packet/sample/duration vectors, selector candidates and sort scratch together, by capacity. Full-listing fallback consumes the same budget, including overlapping old/new plans. Existing individual byte/work limits do not establish this aggregate bound. |
| Packet/configuration | Restore stripped headers and required parameter sets through the existing packet reader. Charge the restored packet and all configuration, NAL, RBSP and bridge copies before growth. Resolve later sample descriptions and selected-packet in-band sets, not only initial `avcC`. |
| Actual SPS | Validate coded width and height separately, checked product, supported chroma/bit depth, macroblock padding and crop units. Reject overflow, empty or out-of-bounds crops. Compute allocation from the full coded grid before cropping; container dimensions cannot authorize pixels. |
| Changes | A fresh request owns a fresh decoder and one picture. Validate configuration and in-band SPS before pixels; refuse conflicting active geometry/configuration within the selected access unit before any second-size allocation. No old-size reference/pool survives beside a new picture. Earlier file configurations are resolved without decoding earlier pictures. |
| Decoder | One thread, at most one picture started, no previous reference pictures, no frame-number-gap picture synthesis, one caller-held packed output and no retry pass. Enforce the picture count inside the codec before `start_picture`, not only by counting container packets. |
| Rendering | Resolve requested size, SAR and rotation before allocating turned YUV/RGB, conversion channels, filter taps or row caches. Free decoder state, source packets and finished planning vectors first. Count every retained source/output plane and scratch capacity. |
| Encoding | One encoder at a time. Its RGB/packed copies, dependency scratch and final bytes share the phase budget. Use fallible bounded output writes; checking a finished Vec is too late. Each encoder needs a proven allocation upper bound at admitted geometry before the format is enabled. |
| Host | One active operation across all APIs of the H.264 module/isolate, from source opening through output drain, cancellation settlement and deferred work. Reject overlap before a second session; do not queue retained inputs. Release the lease exactly once. Never retain a borrowed WASM view across await. |

Accept visible input with each side at most 1,920 and at most 2,073,600 pixels, including landscape and portrait 1080p. Its coded grid has each side at most 1,920 and at most 2,088,960 samples: `1920 * ceil(1080 / 16) * 16`. Both checks apply independently. Supported formats are progressive 8-bit 4:2:0 and monochrome, counted by actual planes. Permit at most one slice per admitted macroblock, hence at most 8,160 slices, with normal nonoverlap/order validation; every NAL/RBSP is bounded by the 4 MiB restored-packet ceiling. Scope, work, packet, planning and byte budgets can still refuse a geometrically admitted input.

Output has each side at most 1,280 and at most 921,600 pixels (`1280 * 720`), after automatic aspect calculations and rotation. An excessive requested/default result fails rather than silently changing size. A 3840×2160 SPS fails before frame allocation even when container, crop or output is tiny. This is the initial profile ceiling, not a claim that 4K is intrinsically impossible within Workers' limit.

### One-picture decoder bound

The existing general-purpose `Decoder::memory_needed` reserves a DPB, in-flight pictures and flush outputs. It is not a measured keyframe cost. Keep it for normal decoding; the codec must own a new mode-specific bound and enforce the one-IDR invariant before using it. `KeyframesOnly` alone does not prove that invariant. A stream's reference declaration, up to the specification's 16, is validated but does not reserve 16 decoded references here. Setting its limit to one and misrepresenting the stream's needs is not a substitute.

For one-thread coded macroblock dimensions `a,b`, let `A=a*b`, `W=16a`, `H=16b`. Existing source owners give stored-picture bytes `S = W(H+8) + W(H/2+4) + 208A`, including band aprons and motion. The 208 comes from sixteen 12-byte motion records and the asserted 16-byte head. Thread cost is `T*A + 128 KiB`, where `T = sizeof(MbInfo) + 2*sizeof(NbInfo) + 2*sizeof(SliceFilter)` is evaluated for the actual target, not copied into JS.

A starting one-picture model is `K0 = S + 384A + T*A + 128 KiB + 4 MiB NAL + 4 MiB fixed decoder state`. These fixed allowances already belong to the general estimator. With the current `T=132` layout, coded 1920×1088 gives `A=8160`, `S=4,853,760` and `K0=17,584,000` bytes. This is a source-derived calculation, not a memory measurement or completed proof. The implementation must bound `K = K0 + every uncovered transient/capacity/copy overlap`, prove one picture through start/finish/flush/output transfer, and require `K <= 32 MiB` before allocation. Include any additional simultaneous NAL/RBSP copy, rather than assuming K0 covers it.

Use fallible frame-sized reservations and the existing counting allocator to test estimates across admitted geometries, parameter sets, slices, output transfer and refusals. No new unsafe allocator is authorized. If this bound cannot be established, stage 1 is not ready; it cannot bypass the general estimator with an unchecked thumbnail-side estimate.

The decoder-owned prerequisite is `Mode::OneIdr` with `Decoder::decode_one_idr` and `NativeH264::one_idr()`, described in [h264.md](h264.md#pictures-that-need-no-other-picture). Its admission contract requires the complete supplied access unit and checks every slice header before picture start. Exact entropy-decoded macroblock coverage is checked before output, not before pixel allocation; a separate entropy-validation pass is not used. Its SPS-derived fallible bound retains `K0` because the preflight borrows the supplied packet and adds no retained packet/header copy, and it reserves slice-filter capacity once. The caller still owns this plan's tighter geometry, packet restoration, planning, rendering, encoding and host budgets. Native allocation tests do not establish the Workers envelope.

## Derivation of budgets

[Workers documents 128 MB per isolate](https://developers.cloudflare.com/workers/platform/limits/#memory), including JS heap and WASM allocations, shared by requests. Conservatively treat this as 128,000,000 bytes, not 128 MiB. Every MiB below is 1,048,576 bytes.

Choose a policy envelope of 112 MiB: 64 MiB WASM logical memory, 16 MiB package-owned host buffers/objects and 32 MiB runtime/other heap reserve. The sum is 117,440,512 bytes, leaving 10,559,488 bytes below the conservative ceiling. These are partitions fixed before measurement, not observed costs or evidence that the runtime fits its reserve.

| Within 64 MiB WASM | Ceiling and derivation |
| --- | --- |
| Initialized module, static state and stack | 8 MiB, one eighth of the partition. Verify the actual startup reading. |
| Reusable codec/render/encode phase | 32 MiB, half the partition. Phases have disjoint lifetimes; this is not 32 MiB per simultaneously live component. Include source planes, rotated copies, RGB, float channels, taps, packed input, encoder scratch and output. |
| Combined plan | 4 MiB, one sixteenth. |
| Restored packet outside codec | 4 MiB, one sixteenth. Codec-side copies are in K. |
| Bridge, growth and allocator retention | 16 MiB, the remaining quarter. Include BufferedSource's 1 MiB window, returned Rust read Vecs, 256 KiB writes, reallocation overlap, metadata, holes and page rounding. |

The decode subtotal is `8+32+4+4=48 MiB`; adding the last quarter derives the 64 MiB page bar. Render/encode free the plan and packet and reuse the entire 32 MiB phase. Final encoded bytes have a 4 MiB ceiling, one eighth of that phase, included inside it. A bounded writer alone does not bound JPEG/WebP dependency scratch or PNG's simultaneous filtered/compressed/final buffers. The implementation must supply those upper-bound expressions; unknown scratch is not replaced with an invented per-pixel constant.

The 16 MiB host partition divides into 4 MiB reader backing stores, 4 MiB pending JS read result, 4 MiB response/in-flight/join copies, 1.25 MiB output queue plus one-chunk overshoot, and 2.75 MiB bookkeeping/copy reserve. Use one response at a time. Count distinct backing-store capacity, not view lengths. The current `memoryBytes`/held-byte statistic omits live chunks/copies and cannot establish this ledger; JOIN_BYTES is not a chunk ceiling. Require bounded production/BYOB or a proven bounded adapter before this route is admitted. Splitting an already received giant buffer does not undo its allocation. Write output in chunks no larger than 256 KiB; a stream high-water mark does not cap an arbitrary enqueue.

The judged route uses ranged input without whole-object upload buffering and streams output without collecting it. In-memory input is at most 4 MiB and replaces, rather than supplements, the reader backing-store reservation. Unbounded foreign readers, caller-held duplicates, unrelated caches and other loaded modules are outside this envelope. Such an application cannot claim the bounded-route result. Enforce host-owned allocation charges and actual backing-buffer high-water; local counters verify the contract but cannot prove the runtime reserve.

### Total-memory validation

WASM pages never shrink, so `moduleInfo().memoryBytes` after completed work is the logical-memory high-water, not live Rust bytes. Record host-owned backing-buffer peaks and overlap charges too. JS heap and process RSS are separate diagnostics when available; neither substitutes for hosted isolate total memory. Report unavailable data as unavailable.

Local workerd establishes component/outcome evidence only. Workers readiness additionally requires private hosted validation under the documented 128 MB limit with the same complete fresh/warm/admission/boundary roster. Freeze candidate, workload, runtime configuration and verdict before that run. Correlate every response with its runtime outcome; require specified complete output/refusal and no exceededMemory, resource-limit 1102, restart or missing outcome. An HTTP success alone is insufficient because the provider can let in-flight work complete after exceeding memory. Send a cost estimate before any deployment; this plan starts none.

The provider exposes no exact peak in the documented limit contract. Do not invent one or infer total safety from pages, RSS or successful responses. Readiness needs allocation-contract evidence, component captures and correlated hosted outcomes together. Until then, label the Workers envelope unvalidated. This plan authorizes no publishing or announcement of hosted support.

## Deadlines

WASM must not call std::time::Instant or promise that Promise.race interrupts synchronous decoding. Native retry code retains its shared original timeout; this bounded profile has no second decode pass. Parser work limits and finite packet/NAL/slice ceilings bound work, while abort closes I/O at suspension points.

The Node convenience path uses a dedicated worker thread with a parent-owned 5,000 ms wall deadline from source opening through response completion. Source callbacks remain in a parent-owned bounded I/O broker; do not attempt to clone functions into the worker or retain extra uncharged read results. Transfer owned buffers with backpressure and close the broker/session on termination. Deadline or abort terminates that worker and discards its instance; a timer inside the worker is insufficient. Five seconds is a service-policy ceiling, one sixth of Workers Paid's documented default 30-second CPU allowance, not a speed claim. Applications may lower it. No partial image becomes success.

Workers uses a dedicated thumbnail deployment and 5,000 ms cpu_ms ceiling, not an in-isolate watchdog. Network waits are not CPU time. Abort is cooperative during synchronous WASM; a hard wall deadline requires abandonment from outside the isolate. CPU termination is a host resource failure, not a promised structured parser error. Never release admission merely because a caller's Promise timeout won a race. Free-plan CPU suitability is not claimed.

## Fixtures, parity and hostile checks

Commit the deterministic generator, exact invocations and roster before the judged run. Use the FFmpeg/x264 pins from `scripts/oracle-ffmpeg.env`, generated colour patterns, two seconds at 25 fps, closed IDRs every 25 frames, progressive 8-bit 4:2:0 and no audio. Cross MP4/Matroska, CAVLC/CABAC and 640×360, 1280×720, 1920×1080; include portrait 1080p and cropped coded 1920×1088. Generate 3840×2160 rejection counterparts. Bind each capture to input hashes and resolved parameters. Fixture generation and hostile media run on Linux CI, not a personal device.

Compare the final installed, optimized H.264 module with a native build of the same operation/profile: every JPEG/PNG/WebP byte, packet identity, presentation time, display size and refusal kind. Cover JPEG qualities 1, 91 and 100, every existing filter/chroma mode, dimensions, rotation, SAR, edits, nonzero times, stream ties and selection fallbacks. Only target-specific decoder-description text may differ. Different native decoders or browser codecs are not byte-equality references. No speed claim is made.

Hostile checks include enormous/overflowing actual SPS dimensions, coded/visible/container mismatches, invalid/empty crops, sample-description changes, conflicting in-band SPS, many sets/slices, recovery/non-IDR and exact fallbacks, mixed-slice/multi-picture packets, truncated selected IDRs, restored-packet/output limits, planning fallback/work exhaustion, short reads, giant backing buffers, cancellation and overlap. Media/limit cases must return structured errors without WebAssembly.RuntimeError, attempt no prohibited allocation, clean up and permit a benign operation on the same usable instance. A partial Matroska file whose selected complete block is valid need not fail. CPU-termination tests have their separate host-failure expectation.

## Registered memory gates

These eleven gates belong only to the H.264 artifact, separately from the sixteen decoder-free gates. Read the registry and predicate rather than copying constants. This docs-only change runs none. Readings are positive integer bytes. The startup bar is the 8 MiB fixed partition; all operation/refusal bars use the derived 64 MiB whole-WASM ceiling. There is no unexplained smaller refusal bar. Independent allocation counters must prove a 4K refusal allocated no decoded frame: pages alone cannot prove that.

<!-- wasm-thumbnail-memory-bars:begin -->
```json
{
  "signal": "wasm_linear_bytes",
  "predicate": "value <= bar",
  "gates": {
    "thumbnail-startup": 8388608,
    "fresh-thumbnail-jpeg-mp4": 67108864,
    "fresh-thumbnail-png-mp4": 67108864,
    "fresh-thumbnail-webp-mp4": 67108864,
    "fresh-thumbnail-jpeg-matroska": 67108864,
    "fresh-thumbnail-png-matroska": 67108864,
    "fresh-thumbnail-webp-matroska": 67108864,
    "refused-thumbnail-4k-mp4": 67108864,
    "refused-thumbnail-4k-matroska": 67108864,
    "thumbnail-warm": 67108864,
    "thumbnail-admission": 67108864
  }
}
```
<!-- wasm-thumbnail-memory-bars:end -->

Startup has its own new isolate. Each fresh gate has another new isolate and sequentially completes the CAVLC and CABAC maximum-coded-1080p fixtures for its container/format at one second, with output explicitly 1280×720. Read after each completed request and judge both. Fresh means the isolate had no earlier media work; the second request retains the first request's high-water. Each 4K gate has another new isolate, tests honest and lying small container dimensions in both entropy modes, reads after every required refusal, then runs/reads a benign thumbnail. Every reading uses the same ceiling.

`thumbnail-warm` has a new isolate and runs the entire admitted size/container/entropy/format roster twice, at zero and one second, with default and explicit maximum output, followed by rotation/crop/SAR variants. Judge all intermediate readings. `thumbnail-admission` has another new isolate: hold one valid thumbnail at source opening and then at output backpressure; at each hold attempt a thumbnail and probe and require both to refuse without another session/queued input. Drain admitted output and deferred work, then check reuse. Also start a probe first and require a thumbnail to refuse. Instrument the actual lease; simultaneous request arrival does not prove overlapping Rust execution.

Judge the installed tarball and exact optimized/fingerprinted module with pinned compiler, bindings generator, Binaryen, Node and local workerd. Use a named workflow_dispatch run of the existing WASM workflow's module job on ubuntu-24.04 at the frozen candidate SHA; record its URL before judging. The later implementation extends that job's installed checks/selectors for the new package; this plan adds no job or required check. A preflight smoke is not a verdict.

Read memory only after complete bodies, deferred work and cleanup, from the instance that did the work. Bind every record to artifact SHA-256 and isolate identity; new-isolate gates require distinct identities. Reject missing/duplicate/unregistered steps, decoded duplicate JSON keys, invalid readings, changed identities, incomplete work, unexpected success/refusal and runtime exit. Keep raw readings, versions, options, inputs and outcomes private; public logs print verdict counts or failing step/bar without readings. A failure creates the existing kind of release blocker, with no ruleset change. A miss cannot be retuned or rerun unchanged except for independently evidenced capture infrastructure failure. A later bar change must merge as its own plan before its run.

Capture host-owned backing-buffer peaks/charges against the 16 MiB partition, bound to the same requests/isolate. Capture native counting-allocator evidence per allocation owner and refusal, not as WASM-page or hosted-total measurements. Missing accounting or an exceeded contract fails readiness even when all page gates pass.

### Module size and notices

The opt-in module inherits the default package's existing distribution budget: at most 1,048,576 bytes under gzip -9 -n on exact shipped bytes. This is a carried-forward bar, not a decoder-fitted number. Record default/H.264 raw module, gzip, glue and tarball sizes plus incremental module cost with identical optimization settings and reachable operations. An unused decoder removed by the linker is not an arm. A miss requires a separately reviewed budget plan before another judged candidate, not an opt-in exemption.

Generate and independently verify each package's locked normal target/feature closure, including the pinned Rust target closure and image encoders. One-codec packaging does not remove notices for shared dependencies. Verify installed exports/types, fingerprint, default-build patent isolation and all sixteen original gates. Equality and new gates judge the installed H.264 artifact, not a development module.

## Stage 2: decoder-free browser path

The browser-only entry `@coreav/wasm/browser` exposes `thumbnailFrame(source, { at, stream?, limits?, signal? })`, returning `{ frame: VideoFrame, summary }`. The caller owns the frame and closes it in finally. This is a selected-thumbnail API, not arbitrary frame decoding. Initially reject resize, format, quality, filter, chroma, exact and decoder-choice options. DOM types stay in this entry's declarations. Return an unrotated frame with summary instructions for visible rectangle, exact time, SAR and one clockwise container rotation.

Reuse stage 1's internal selected-IDR preparation and prefix-aware packet reader. The private extraction result carries owned access-unit bytes, packet/time/stream identity, resolved coded/visible geometry, colour/SAR/rotation and a configuration union. For length-prefixed AVC, description is the complete active AVCDecoderConfigurationRecord payload with matching length-size/parameter sets; for Annex B, omit description and include the required SPS/PPS with the IDR. Resolve in-band and sample-description changes at the selected packet. Derive the qualified AVC codec string from those bytes, not a hard-coded profile. Transfer/copy bytes into JS ownership before releasing WASM; never return a borrowed memory view or base64. Stage 2 starts with the same H.264 IDR admission profile. Browser availability bypasses neither VP9 prerequisites nor AV1 refusal.

After isConfigSupported for the actual configuration, configure one request-owned VideoDecoder with hardwareAcceleration "no-preference"; hardware use is not promised. Submit one key EncodedVideoChunk and await flush. Timestamp is selected file-relative PTS, floored to integer microseconds and checked for JS safe-integer range, never DTS or requested time. Keep the exact rational as decimal numerator/denominator strings in summary; omit unknown presentation duration. Require one output at that timestamp. Zero, extra or unexpected outputs fail. Close decoder and every unreturned/late frame on success, error or abort. Closing the decoder does not close a returned frame. Reconcile visible geometry/colour with admitted configuration or refuse; do not apply SAR twice through displayWidth.

Stage 2 needs a tested secure-context loader/capability matrix for JSPI, exception references, SIMD and WebCodecs. Unsupported configuration is NotSupportedError, misuse is TypeError, abort is AbortError and media failures retain CoreAV's structured document; arbitrary browser errors stay in cause. Support checking is best-effort and does not guarantee decode success. No silent library fallback. A hard synchronous extraction deadline needs an independently terminable Web Worker. Browser decoder/GPU memory and caller-held frames are outside Workers partitions and stage-1 gates.

Tests compare admitted packet, exact time, configuration and display geometry against native selection across containers, edits, window/full-listing fallback, unknown times and ties. Cover AVC length sizes, Annex B/parameter sets, stripped headers, later descriptions, truncation and stage-1 admission limits. Real browser decoding plus deterministic API fakes cover unsupported configuration, error/no/duplicate output, flush failure, abort races, late callbacks, frame cleanup and subsequent reuse. Browser pixels and image encoding need not equal native bytes; no quality, hardware or speed claim follows without a separate registered method. Stage 1 native/WASM image-byte equality remains mandatory.

Protocol references: [WebCodecs lifecycle, configuration and memory model](https://www.w3.org/TR/webcodecs/) and [AVC codec registration, sections 1–4](https://www.w3.org/TR/webcodecs-avc-codec-registration/).

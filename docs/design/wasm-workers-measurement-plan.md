# WASM operation measurement plan

This plan registers a future measurement of `@coreav/wasm` in local `workerd` and hosted Cloudflare Workers. It contains no deployed results. The plan must merge before the harness records a judged result. A local diagnostic cannot pass these bars or establish a hosted memory or latency guarantee.

## Operations and scenarios

Classify every public export from `packages/wasm/src/index.js` as either a media operation in this table or a non-media export in the inventory below. Freeze both inventories in the manifest. Before timing, the harness must reject an export that is missing, unknown, or classified more than once. Do not derive the measured inventory by filtering exports against this table, because that would hide a new media operation. A changed inventory requires a plan update before measurement, not an omitted row.

| Public operation | Successful scenario | Additional scenario |
| --- | --- | --- |
| `probe`, `probeText` | Native probe document, with each export timed separately | Fragment scanning and frame-size scanning enabled, each separately |
| `ffprobe`, `ffprobeText` | Fixed ffprobe document, with each export timed separately | Safe filename label and chapter metadata |
| `check` | A versioned policy that accepts the input | A policy rejection, reported separately from success |
| `chapters` | Referenced chapter track and Nero chapter metadata, separately | No chapters |
| `extractAudio` | AAC stream-copy output | Sparse and densely interleaved audio, separately |
| `extractSubtitles` | Timed text to WebVTT | Explicit stream selection |
| `clip` | Video and audio from the middle of the input | A cut near the end and audio disabled, separately |
| `remux` | Default stream-copy output | Explicit stream selection |
| `sanitize` | Default sanitized output | Explicit retained metadata |
| `hlsIndex` | Index construction with one-second segments | Audio-only and video-only inputs, separately |
| `hlsGet` | Master playlist, each media playlist, each init segment, and each media-segment rendition, separately | First, middle, and final media segments, then a complete playlist crawl |

`hlsGet` starts with the same prebuilt index in both engines. Index loading is outside its operation timer and has a separate byte and memory record. `hlsIndex` measures index construction. The complete HLS crawl reports both index construction included and index reuse. Application cache hits, stored-index reloads, and damaged-index rebuilds are separate scenarios, not substitutes for library execution.

There is no thumbnail, decode, or transcode operation in this package. Initialization, `moduleInfo`, source adapters, constants, and error helpers are setup or diagnostics. Record their costs separately rather than counting them as media operations.

The non-media inventory is `init`, `moduleInfo`, `CoreAVError`, `RequestLimitError`, `SourceChangedError`, `SourceNotFoundError`, `errorResponse`, `RANGE_DEFAULTS`, `bytesSource`, `httpSource`, and `rangeSource`. The Workers entry adds `R2_DEFAULTS` and `r2Source` to that inventory. Check that entry separately against the shared inventories and these two additions.

## Inputs and frozen artifacts

The harness commit records a manifest before the judged run. Each row contains the fixture recipe or redistributable source and licence, SHA-256, bytes, container, track layout, sample counts, supported operation scenarios, and expected refusals. Freeze the operation options, policy bytes, clip points, HLS resources, and expected outputs beside that manifest. No private or held-out project corpus enters public logs, artifacts, or this measurement.

Use generated inputs near 2 MiB, 90 MiB, and 1 GiB for every successful operation. Keep sample contents offset-distinct so swapped samples cannot pass an output check. Include front and tail movie metadata, dense and sparse interleaving, variable sample sizes, edits, fragmented inputs where supported, and chapter and subtitle tracks. Use at least six independent fixture recipes per operation and size class. A larger file is not merely padding outside the media payload.

The common success matrix uses MP4 or MOV. Add Matroska and WebM scenarios for the operations whose current Rust implementations support them, and MPEG-TS for `ffprobe`. Unsupported format-operation pairs get explicit refusal rows and never enter success latency statistics. Container coverage is a manifest property, not inferred from probe support.

Freeze the public source commit, packed package hash, module fingerprint, native executable hash, compiler pin, optimizer pin, dependency locks, `workerd` version, compatibility date, host CPU and memory, and client environment. Both engines build from that source commit with the repository's `edge` profile. Use the package's existing tool pins. Record any host or runtime change as a new candidate.

## Separate computation from storage

### Same-host warm operation cost

Run the native operation in-process and the package in pinned local `workerd` on one otherwise idle host. Load the identical input and prebuilt index before timing. Native output uses the reference engine's copied-chunk delivery, and WASM output is consumed to completion inside the Worker. Both count output bytes and failures. Do not include a native process launch or file load in one engine's operation time.

Discard one warm-up pass. Calibrate batches to at least 50 ms using an external monotonic clock, then collect five rounds with engine order rotated each round. Record empty control-request batches without subtracting them. If control overhead exceeds 1% of a measured batch, enlarge that batch before starting the judged rounds. Record parsed and text exports separately because JSON parsing belongs to the parsed export.

For each fixture, take the median of its five batch means. For each operation and size class, report `p50(wasm) / p50(native)`, where each p50 is the median over fixtures. Also report per-fixture ratios and every failure. Bootstrap paired fixtures 10,000 times with seed `349`, and report the 95% interval. These are distributions of batch means, not single-request p99 values. Do not pool unrelated operations or size classes to hide a slow row.

### Local delayed range reads

Use one local HTTP range server with 75 ms and 235 ms delays before response headers. Exercise the shipped HTTP adapter and an R2-shaped local binding separately. The latter is an adapter test, not a measurement of R2. Keep a 40 MiB reader bound and record effective settings and hint calls.

For each operation, input, and delay, compare the bounded reader with a whole-object GET followed by `bytesSource`. A whole-download result that exceeds the chosen memory budget remains an infeasible baseline, not a recommended strategy. Playlists and init segments must not download the media object merely to create a baseline.

Record both unthrottled transport and a fixed 60,000,000-byte-per-second body rate. Rotate strategy order across five rounds. Delay, request overlap, body throughput, parser work, and output consumption must remain distinguishable. A range-request count alone cannot establish storage speed.

### Hosted Workers and R2

Deployment needs separate authorization. Freeze the runtime configuration, object placement, client locations, request and CPU budgets, and teardown conditions before deploying. Use only the frozen redistributable fixtures. Record the actual serving data center rather than assuming a requested client region determines Worker placement.

Use one discarded warm-up and five measured requests per fixture and scenario, with strategy order rotated. Report client-observed first-byte and completed-body time, plus an unmodified no-op control on the same connection. Report platform CPU telemetry when available and label it unavailable otherwise. Do not subtract control latency silently or compare a remote Worker with a native process as if that ratio measured compilation overhead.

Record source-object requests and bytes separately from index reads, index writes, telemetry, and other service requests. Include retries and speculative requests, even when cancelled. Request caps and deadlines end the scenario with a recorded failure. A failed or truncated response never enters successful latency statistics.

## Metrics and memory

Every scenario records request attempts, server-observed requests, bytes requested, bytes delivered by the server, bytes consumed by the reader, output bytes, time to first output, completion time, failures, and exact output comparison. Preserve the range trace and overlap. These counters differ when cancellation or read-ahead occurs.

Record fresh and warm WASM linear-memory high water, reader-held-byte high water, output-queue settings, JS heap where the runtime exposes it, and process peak resident memory. Linear memory never shrinks. Process resident memory is not per-isolate hosted accounting, and the reader's bound excludes response chunks and other allocations. Missing measurements stay missing, never zero.

Test slow consumers, cancellation, a warm remux followed by probe and clip, and concurrency 1, 4, and 6 as separate resource scenarios. Keep platform limits distinct from library limits. State which plan and configuration actually ran without promising another plan's behavior.

## Registered bars and verdicts

| Bar | Predicate | Scope |
| --- | --- | --- |
| Correct output | Every successful document matches the native reference under the export's text or parsed-value contract, and every streamed output has the same SHA-256 | Every operation, fixture, mode, and HLS resource |
| Complete execution | No unexpected rejection, trap, timeout, truncated output, missing row, or uncounted timed-call failure | The complete declared success matrix |
| Module size | `gzip -9 -n` of the shipped module is at most 1,048,576 bytes | The exact measured artifact |
| Existing memory bars | Every registered scenario in [WASM memory bars](wasm-memory-bars.md) passes unchanged | Only that document's generated fixtures and isolate sequences |
| Reader retention | Reader-held bytes are at most 41,943,040 bytes with the 40 MiB preset | Each bounded-reader session, not total isolate memory |
| Request ceiling | At most 1,000 attempted source requests, including retries, and no request-cap error on a supported success fixture | Each bounded-reader operation |
| Index-only access | Zero source-media requests | Every HLS playlist and init-segment request with a prebuilt index |
| Warm operation target | Bootstrap 95% upper bound of `p50(wasm) / p50(native)` is at most 2.00 | Each operation and size class in the same-host, memory-input matrix |

The warm operation target extends one comparison rule to the entire declared matrix. It is a measurement target, not a new release-scope decision or an assertion that an earlier probe result applies to another operation. A failure is reported per row. It does not remove an operation from the release automatically. Hosted latency, CPU, and total memory are reported without a new pass threshold because placement and platform accounting differ from the local host.

Judge each frozen candidate once. A miss remains a miss. A corrected candidate needs a new source identity and complete affected measurements. Repeat an unchanged candidate only for an independently evidenced infrastructure fault. Host contention, incomplete coverage, stale fingerprints, and mismatched outputs invalidate the comparison and remain in the record.

## Result publication

The public report includes this plan, the frozen manifest and harness, artifact identities, settings, host and runtime versions, per-operation medians and intervals, failures, memory availability, and sanitized per-call records. Keep credentials, signed URLs, object identifiers, infrastructure names, private source material, and operational records out of the report. Deployment and publication are separate actions. This document does neither.

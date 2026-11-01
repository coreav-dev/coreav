# WASM memory bars

These proposed bars take effect only after this plan merges. Scope: the proposed WebAssembly linear-memory gates of the package's forthcoming example integration check (`packages/wasm/examples/test/check.mjs`). This note fixes every bar, the scenarios it applies to and the rules for judging it before any run judges it. The change that turns the gates on reads the bars from the block at the end of this note.

## What is measured

The package instantiates its module once per JavaScript isolate. The module has one linear memory: it imports none and declares no other. The check reads `moduleInfo().memoryBytes`, the byte length of that memory, through a test-only route of the harness Worker.

Linear memory grows in 64 KiB pages and never shrinks. The value read after a step is therefore the largest size the memory reached since the isolate started, including the size after initialization. It is a peak, not a sample. It counts the module's logical pages, not live allocations, resident pages, or the runtime's larger virtual-address reservation.

All requests in flight in one isolate share that memory. A reading after a concurrent batch is the high-water mark of the shared memory over the whole batch. Nothing is added up across requests or across isolates.

The gates do not measure the JavaScript heap. Range-reader buffers and their 40 MiB bound, upload bodies, output queues, the HLS index cache and response bodies live there. They also do not measure process resident memory or any hosted platform's per-isolate accounting. The check may record the heap and resident memory where the local runtime exposes them. Those records never pass or fail anything.

The runtime is the `workerd` version pinned by the examples' lock file, started locally by the check, one process per isolate. Results describe that local runtime and the generated fixtures only.

## Scenarios and bars

Every request reads its fixture by object key through the examples' R2 reader. Fixtures come from the examples' fixture generator; `video-only.mp4` is the native remux of the video stream of `av.mp4`.

| Gate step | Isolate | Work before the reading | Fixtures | Bar |
| --- | --- | --- | --- | --- |
| `fresh-probe`, `fresh-check`, `fresh-chapters`, `fresh-extract-audio`, `fresh-extract-subtitles`, `fresh-clip` | New, one per step | One request of that operation | `av-text.mp4` | 32 MiB |
| `fresh-ffprobe` | New | One `ffprobe` request | `av-text.mp4` | 32 MiB |
| `fresh-remux`, `fresh-sanitize` | New, one per step | One request of that operation | `av-text.mp4` | 64 MiB |
| `hls-built` | New | Crawl every playlist and segment of each fixture in turn, building each index | `av.mp4`, `audio.m4a`, `video-only.mp4` | 64 MiB |
| `hls-concurrent` | Same as `hls-built` | Then ten error-path requests, including a malformed movie, followed by four parallel requests for one video segment | The preceding HLS fixtures and `not-a-movie.bin` | 64 MiB |
| `hls-reloaded` | New | Crawl using the stored index | `av.mp4` | 64 MiB |
| `hls-rebuilt` | New | Crawl after the stored index is damaged, so it is rebuilt once | `av.mp4` | 64 MiB |
| `concurrent-with-writers-1`, `-4`, `-6` | New, one per level | 18 requests: probe, ffprobe, check, chapters, extract-audio, extract-subtitles, clip, remux and sanitize in that order, then the same nine again, with at most 1, 4 or 6 in flight | `av-text.mp4` | 16 MiB |

The eight `fresh-*` bars other than `fresh-ffprobe` carry forward the package implementation's fresh-operation budgets unchanged. This plan may merge before that implementation; it enables no gate itself. The other eight gates are new.

## How the bars were set

The package implementation's fresh-operation gates allow 32 MiB for operations that return a document or stream a small output, and 64 MiB for remux and sanitize, which hold output sample tables. This note carries those budgets forward rather than predicting real-media costs from the generated fixtures.

`ffprobe` joins the 32 MiB class. It reads the same structures as `probe`, under the same table limit, and returns a document.

HLS joins the 64 MiB class. Building an index holds the sample tables of the whole file, like remux, and serving a resource copies the index into the module. Because memory never shrinks, the bar applies to each reading of the HLS isolate after all the work before it.

Concurrent batches with writers have no earlier measurement to inherit a class from. Their bar follows a rule fixed here: take the highest local reading recorded for the three levels on the generated fixtures, multiply it by the highest covered level (6), round up to a power of two in MiB, and never exceed the heaviest class in the batch (64 MiB). The result is 16 MiB, for every level. Multiplying by the level is an envelope: it allows as much as if each of six requests in flight needed the whole recorded footprint at the same time. The planning anchor is one earlier local macOS diagnostic run of a different module build, with no usable heap or resident-memory readings; it is not a judged pass. Its raw readings remain private. This is a regression budget for these generated fixtures, not a class-wide maximum for arbitrary media. A mixed batch can fail this bar even when its fresh operations stay below their individual bars.

The threshold's source reading and calculation are retained in a machine-local record with the source checkout commit, hashes of uncommitted overlay files, module SHA-256, host and runtime versions, and invocation. The planning command, from the repository root, is `node packages/wasm/examples/test/check.mjs --edge-native PATH --tarball PATH`, using the retained native binary and package with no extra fixture directories. These readings set the threshold only. Reproducing them on macOS, or recalibrating against the release candidate before enforcement, is neither a prerequisite nor a release verdict. The verdict comes only from the named `ubuntu-24.04` run at the frozen release-candidate SHA described below.

Bars are exact byte counts: 1 MiB is 1,048,576 bytes, so 16, 32 and 64 MiB are 16,777,216, 33,554,432 and 67,108,864 bytes. A gate passes when its reading is at most its bar.

## Judging rules

- **One isolate, one reading source.** The harness Worker reports an identity created once per isolate beside every reading. Every reading of a scenario must carry the identity of the isolate that ran its work. Each new isolate must report an identity that no earlier isolate of the run reported.
- **Fresh.** A fresh gate's isolate has served only the start-up readings before its one request.
- **When the reading is taken.** After the client has read every response body of the step to its end, and after work the requests deferred (such as completion logs and index store writes) has finished.
- **Build.** The module's reported build fingerprint must equal the SHA-256 of the module in the packed archive under test.
- **Concurrency.** Reset the in-flight counter before each writer batch and before the four-request `hls-concurrent` batch. Hold the first requests at harness entry until the declared number arrives, then release them together. Count each request until its response body completes, and require the peak to equal the declared level exactly: 1, 4 or 6 for writers, and 4 for HLS. A lower or higher peak fails; this does not prove overlap inside the module.
- **Unavailable is a failure.** A missing, non-integer or non-positive reading, a changed isolate identity, a fingerprint mismatch, a missing, duplicate or unregistered gate step, or a runtime that exits before its reading fails the run. Nothing unavailable counts as a pass. An unavailable heap or resident-memory record is written down and changes nothing.
- **Registry duplicates.** Before parsing the registry JSON, the checker scans its raw text for repeated property names, comparing decoded names so an escaped spelling cannot hide a duplicate. Any duplicate fails. The gating change includes a regression with a repeated gate name; ordinary JSON parsing alone is insufficient because it keeps only the last value.
- **Inputs.** A judged run uses the generated fixtures only, without extra fixture directories.
- **Output.** Public logs print verdicts only: the number of gates passed, and on a failure the step and its bar, without the reading. Raw readings stay on the machine that ran the check.
- **Release verdict and enforcement.** The release verdict comes from one named `workflow_dispatch` run of the `wasm` workflow (`.github/workflows/wasm.yml`), job `wasm-module` on `ubuntu-24.04`, whose head SHA matches the frozen release-candidate SHA; record its run URL before judging its result. These gates stay non-required with no ruleset change, so a failure on a pull request or main gets a release-blocking issue but does not block merges between releases, and release requires the RC verdict run to pass; the gating change must widen the workflow's trigger path filters to every crate the module links.
- **No retuning after a result.** A bar is not changed in the change it judges. A later bar change needs an edit to this note, with its cause, merged before the run it judges. A failed candidate needs a corrected new candidate and a new named verdict run, not a repeat of the unchanged candidate. Repeat a run only after an independently evidenced infrastructure fault, never to overturn a miss. A crash, timeout or start-up failure fails the run and is not automatically a host fault.

## Limits

- The fixtures are generated, small (none above 16 KB) and few: one per fresh and concurrent scenario, three for HLS. The bars catch gross regressions on these inputs. They do not show that media of any real size stays within them.
- Linear memory is one part of an isolate's memory. These gates bound nothing in the JavaScript heap.
- A local `workerd` is not a hosted runtime. This note makes no claim that any workload fits a hosted platform's per-isolate memory limit.
- Requests in flight are counted at the harness. Whether the module's own calls overlap while suspended on reads is not observed.
- These records stay diagnostic, with no bar: the start-up readings, warm mixed passes, concurrent batches without writers, the library-default reader variant, and the long-lived isolate of the parity checks.

## Registered bars

<!-- wasm-memory-bars:begin -->
```json
{
  "signal": "wasm_linear_bytes",
  "predicate": "value <= bar",
  "gates": {
    "fresh-probe": 33554432,
    "fresh-check": 33554432,
    "fresh-chapters": 33554432,
    "fresh-extract-audio": 33554432,
    "fresh-extract-subtitles": 33554432,
    "fresh-clip": 33554432,
    "fresh-ffprobe": 33554432,
    "fresh-remux": 67108864,
    "fresh-sanitize": 67108864,
    "hls-built": 67108864,
    "hls-concurrent": 67108864,
    "hls-reloaded": 67108864,
    "hls-rebuilt": 67108864,
    "concurrent-with-writers-1": 16777216,
    "concurrent-with-writers-4": 16777216,
    "concurrent-with-writers-6": 16777216
  }
}
```
<!-- wasm-memory-bars:end -->

# Worker examples

Two application starting points for the private `@coreav/wasm` package. `media/` serves document and stream-copy operations. `hls/` serves playlists and segments from R2 objects, with optional persisted indexes. Each directory owns its policy code and can be copied on its own. Neither is a deployable service as supplied.

## Before using an example

Replace `authorize()` with your application's authentication and tenant policy. It must return the R2 key prefix the caller may access. The supplied function refuses every request. Only the local harness sets `DEV_ANONYMOUS=1`; neither Wrangler configuration enables it. Keep all manifests private, install a locally built package or an approved private package, and replace the bucket placeholders before running your own tooling. These examples do not publish or deploy anything.

For the media Worker, set `ALLOWED_HOSTS` to the exact HTTPS hosts, including nondefault ports, that your application trusts. An empty list disables URL input. Userinfo is forbidden, fragments are removed, and redirects are refused. This is a host allowlist, not a DNS-address filter; it does not prevent an allowed host from resolving to a private address. Tenant-specific URL rights are your application's responsibility.

For HLS, use separate `MEDIA` and `INDEXES` buckets. Review `CORS_ORIGIN` and add a lifecycle rule for `coreav-hls-index/` objects: each media version, WASM build and option set creates its own record. `INDEXES` is optional; without it indexes stay in the isolate cache only. The index prefix prevents accidental collisions, not unauthorized access.

Set `MAX_SOURCE_REQUESTS` below your platform's subrequest budget. Media operations use the budget for reader requests; HLS also needs a metadata head and, when configured, a stored-index get and put. Retries consume reader requests. The examples do not follow HTTP redirects.

## Media routes

Use `GET /ROUTE?key=OBJECT`, `GET /ROUTE?url=HTTPS_URL`, or `POST /ROUTE` with the file as the raw body. Exactly one input is accepted. Unknown or duplicate query parameters are refused.

| Route | Additional parameters | Response |
| --- | --- | --- |
| `probe` | `scan` | Native probe JSON text, with integer tokens unchanged |
| `ffprobe` | None | Fixed ffprobe JSON; filename is always `-` |
| `check` | None | Verdict JSON, 200 for accept or 422 for reject |
| `chapters` | None | Chapter JSON |
| `extract-audio` | `stream` | M4A |
| `extract-subtitles` | `stream` | WebVTT |
| `clip` | `start`, `end`, `video`, `audio` | MP4; `start` is required, `audio=none` disables audio |
| `remux` | `streams`, `dropUnsupported` | MP4 |
| `sanitize` | `streams`, `keep` | MP4; for example `keep=none` or `keep=rotation,language` |

Stream indexes are nonnegative integers. `streams` is a comma-separated list. Times are decimal seconds. `POLICY_JSON` overrides the example upload policy as exact JSON text; invalid configuration fails closed.

Uploads count bytes actually received, not only `Content-Length`. `MAX_UPLOAD_BYTES` defaults to 8 MiB and cannot exceed 16 MiB. A mismatched length, empty body or content encoding is refused. Without a declared length, retained chunks and the completed buffer briefly coexist, so the upload cap is not a peak-memory bound.

## HLS paths and limits

`GET /OBJECT/master.m3u8` starts a crawl. Other resources are `video.m3u8`, `video-init.mp4`, `video-N.m4s`, and their audio equivalents. Percent-encode object keys. `SEGMENT_DURATION` defaults to six seconds and is part of the cache identity.

An index is identified by the tenant-prefixed object key, ETag, options and SHA-256 of the exact shipped WASM bytes. The cache naming scheme is `coreav-hls-cache/1`; it is independent of the binary index version, which the WASM reader validates. Different WASM builds at the same package version do not share records. Reads pin the media ETag, and a damaged binary index is rebuilt at most once per request.

The cache admits at most 16 ready or loading entries. Stored indexes are size-checked before allocation; retained plus reserved stored-index bytes stay within 8 MiB. At most one index build runs at a time. Excess concurrent work receives 503 with `Retry-After: 1`, rather than waiting in an unbounded queue. An index over 8 MiB receives 422 and leaves a small refusal record, avoiding repeated builds. Concurrent rebuilds share one owner; re-reading a damaged stored record does not count as rebuilding it.

Every remote reader explicitly uses a 40 MiB bound. None of these limits proves a 128 MiB isolate bound: index generation completes before its size is known, operations copy indexes into WASM, evicted bytes can remain in use, pending stores can retain buffers, and warm linear memory does not shrink. Real deployment memory and concurrency require separate measurement.

## Errors and logs

Source failures and missing objects use fixed response messages. Unexpected exceptions, including a binding's `TypeError`, become a fixed 500 response. Only package option-usage messages and parser rejection documents pass through. URLs, keys, credentials and request headers are never used as filenames or log fields. Logs contain only allowlisted route and outcome constants; streaming outcomes come from the operation's completion summary.

## Local checks

From the repository root, build the WASM package and native reference, then run:

```sh
npm ci --prefix packages/wasm/examples --ignore-scripts
npm test --prefix packages/wasm/examples
npm run check --prefix packages/wasm/examples -- --edge-native "$(pwd)/target/edge/edge-native"
```

The integration check uses `workerd`, `esbuild`, deterministic ISO fixtures generated from this repository's test structures, and a local R2 stand-in. It makes no cloud requests. An alternate built package can be selected with `--wasm-package DIR`; `--fixtures DIR,...` adds uniquely named fixtures. Use only trusted generated fixtures on a personal machine.

| Coverage | Local checks |
| --- | --- |
| Every media operation | R2 key, HTTPS URL and upload; native JSON or byte parity |
| Operation options | Fragment scan, stream selection, decimal clip times, remux and sanitize options |
| Documents | Chapters, policy verdicts, exact integers above 2^53, fixed ffprobe filename |
| Boundaries | Actual upload bytes, authentication order, URL policy, fixed errors and safe logs |
| HLS | Native playlist/segment crawl, missing resources, CORS, store-less mode |
| Cache | Size and admission limits, reservations, shared builds, races, damaged records, ETag and build changes |
| Tool graph | Fresh lock checked against the repository license policy |

The harness records warm WASM memory and cache statistics in `.harness/memory.json` as evidence, not as a pass against an isolate-memory bar. It also checks stored-index reuse across restarts and two distinct WASM builds at one version. Empty comparisons fail. Tarball installation, copied-example installation, full format fixtures and hostile-input checks belong to the separate packaging validation; a local smoke run does not establish them.

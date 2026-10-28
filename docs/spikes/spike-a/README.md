# Stage 1 Spike A: the ML loading slice

Throwaway, unpublished experiment for **H1** and **H7**, derived only from the
roadmap's stage 1 plan. It implements
Rust MP4 parsing → a saved frame index → a GOP seek plan → torchcodec decode →
a tensor exported through a PyO3 DLPack producer on CPU and CUDA. No workspace files,
project rules, CI, `bench/`, Spike B or Spike C files are changed. Production
code must be written fresh under the hardened gates if the path graduates.

**Overall verdict: inconclusive.** This is an experiment record, not a
stage-gate verdict; the maintainer signs those off, and
a commit's DCO sign-off is not one. The direct NVDEC sub-path is
**not measured** because Spike B is being built concurrently. No substitute
NVDEC implementation is included.

Over real **Cloudflare R2** storage across the Pacific, the CoreAV loader shows
no warm win (0/6 cells), and both loaders issue identical requests. Bounded
read-ahead cuts index requests from 5/5/897 to **3/4/299**.

**H7 fails against its threshold relative to a full download.**
Probe and index read at most **1.68%** of each object, but they beat a full
download only on the large file in both regions and on the small file in
Asia Pacific. The fragmented file takes **126–139 times** as long to index
as to download whole (about 1.1–1.4 s for 72.8 MB), even with 32 concurrent
fragment reads.

## Hypotheses and fixed criteria

H1: “A persistent index and seek plans remove a material share of end-to-end
random-access time in real ML workloads.”

H7, as restated with that threshold: “Probe and index over object storage read a
small, bounded share of each object and finish faster than downloading the
whole object over the same link.” (Previously: “Probe and index over object
storage need only a few bounded range reads.”)

Spike A's criteria:

> Pass: warm random access beats torchcodec with `custom_frame_mappings` (the strongest baseline, not exact mode) on at least 2 of 4 scenarios by a margin outside measured noise, or a clear win on remote reads.

> Kill: CoreAV's ML role narrows to a mapping supplier and a safe, remote-capable demux and index; its own loader and samplers are dropped.

The Stage 0 H1 pivot is also retained:

> If the index and seek-planning levers can save less than 20% (initial) of end-to-end random-access time, the ML claim moves from "faster exact random access" to batching, remote reads and safety, and H1 leaves the messaging.

The initial H7 threshold was committed before its run. For each input and
each worker region, probe plus index must read at most **5%** of the object
in every trial. It must also finish faster than the faster of a single-stream
GET and a parallel ranged download of the whole object. "Faster" means the
upper end of the bootstrap 95% interval for the mean of paired index/download
ratios is below 1. H7 passes only if every input passes in every region.

The roadmap does not name four particular Spike A scenarios. This experiment reuses the exact
Stage 0 files and requests; it does not invent a fourth representative scenario.
For this measurement, “outside measured noise” means the upper end of the
bootstrap 95% interval for the **mean of paired latency ratios** is below 1,
with correct outputs. This is an operational interpretation, not a new recorded
threshold. Intervals also contain variation in request positions and GOPs;
they do not isolate timing jitter or host-to-host variance.

## Method

The standalone Cargo package uses PyO3 **0.29.3** with the Python 3.13 stable ABI,
serde and serde_json; the committed `Cargo.lock` pins all Rust dependencies.
`mp4.rs` forbids unsafe code and accepts a limits value. The defaults used here
are 1 MiB per payload read, 256 MiB cumulative metadata, one million boxes and
two million video samples. Sample counts are checked against payload length
before allocation. Metadata traversal follows a fixed set of container layers;
unknown boxes are skipped, and media payload is not read to build the index.
These limits and this representation are spike choices, not graduated budgets.

The WAN experiment adds a bounded index-read policy, chosen before its run:
4 KiB speculative windows, at most 4 MiB cumulative extra requested bytes,
and the existing 1 MiB limit on every individual read. Windows combine moof
headers, payloads, adjacent headers and nearby fragment starts; they can include
media bytes, whose cost stays in the counters. `sidx` and trailing `mfro` →
`mfra`/`tfra` offsets guide window boundaries. They never replace the complete
box walk: a sidx subsegment can contain several moofs and tfra can be sparse.
Every sample still comes from the definitive tables/fragments. Setting
`FrameIndex.build(..., optimized=False)` selects the prior exact-read policy
for the paired before/after measurement; it reproduces 5/5/897 requests.
Both paths must produce identical serialized indexes and oracle mappings.

The H7 round adds bounded concurrency (default **32**, accepted range 1–64).
When `sidx` or `tfra` give fragment offsets, the parser requests a 4 KiB
window at every fragment start not yet held, as one batch with at most 32
requests in flight. The batch draws on the same 4 MiB speculative budget.
The box walk then runs over the fetched windows. A wrong or missing hint
costs only extra sequential reads, and every box is still visited.
Concurrency 1 is the earlier sequential policy, measured as a paired
reference. On the fragmented input, one `sidx` lists all 297 fragments. The
build is three sequential reads followed by one batch of 297 windows.

The H7 baselines run in the same worker against the same R2 objects. Each
trial shuffles seven methods, over five trials:

| Method | What is timed |
|---|---|
| index, concurrent / sequential | `FrameIndex.build` over SigV4 range GETs, from offset 0 to a complete index equal to the local one |
| download, single | One GET of the whole object, read in 1 MiB chunks and discarded |
| download, parallel | 8 MiB ranged parts, 32 in flight, read and discarded |
| torchcodec, URL approximate / exact | `VideoDecoder` construction and metadata access on a 15-minute SigV4 presigned HTTPS URL; FFmpeg's own HTTP client reads the object |
| torchcodec, reader approximate | The same open through the counting 256 KiB-block file-like reader |

Downloads are checked by `If-Match`, `Content-Length` and `Content-Range`,
not by hashing. Exact mode through the file-like reader is not run: it would
scan the whole object in sequential 256 KiB requests. Each torchcodec open
runs in a subprocess with a 420 s timeout. The presigned URL reaches it on
standard input and is redacted from any error text. The torchcodec rows
report the worker's network-interface receive counter (including TLS and HTTP
overhead), not request counts, for the URL variants. torchcodec's
approximate mode builds no exact frame index. Its timing is a probe
comparison, not an equivalent of CoreAV's index. The pinned FFmpeg 9.0.2
verifies TLS peers. Its bundled GnuTLS looks for `/etc/pki/tls/cert.pem`,
which the image links to Debian's CA bundle.

The implemented slice covers one self-contained, unencrypted AVC video track:
`stts`, signed/unsigned `ctts`, `stsz`, `stsc`, `stco`/`co64`, `stss`, and
`trex`/`tfhd`/`tfdt`/`trun` fragments. At most one empty edit and one media edit
at rate 1 are applied. A zero-duration edit in an empty fragmented initial movie
sets a timeline offset, as ISO/IEC 14496-12:2012 section 8.6.6 specifies. Edits
whose durations cannot be represented exactly in media ticks are rejected in
this slice. External data references, description switches, encrypted entries,
multiple video tracks and multi-track fragments are rejected. Audio metadata
is skipped. This is not a complete production probe or streaming demuxer.

The saved `.cvx.json` is an explicitly temporary version-1 JSON document:
`version`, SHA-256 content `identity`, `file_bytes`, `track_id`, `timescale`,
`width`, `height`, `fragmented`, `samples` and `frames`. Each sample has `offset`,
`size`, raw `dts`, `media_pts`, edited `pts`, decode `duration` and `key`.
`frames` contains sample ordinals in increasing presentation order. Loading
validates identity, counts, frame ordinals, unique PTS and byte ranges. The
caller must obtain identity from trusted content/ETag metadata. This is not the
public, frozen `.cvx` format, and loading still deserializes the bounded JSON
before count validation; production memory guarantees are not claimed.

A seek plan groups requests by preceding sync sample, lists conservative GOP
prefix packet ranges in decode order, and supplies a target permutation and
its inverse. No packets are marked non-reference or discarded, because this
spike has no bitstream dependency parser. **Torchcodec still performs packet
I/O, seeking, reference handling and decoder drain.** The bridge feeds it
CoreAV-built `custom_frame_mappings` and target order; it does not inject Rust
packets or establish a parser-free NVDEC path. HTTP prefetch uses planned
packet ranges and a cache shared with torchcodec's file-like reads; misses
and metadata/read-ahead requests remain counted.

The PyO3 producer forwards `__dlpack__` and `__dlpack_device__` to the decoded
tensor. Tests compare producer/consumer addresses and validate pixels after
dropping the producer. Restoring requested order uses `index_select`, which
**copies the batch before DLPack export**; only the subsequent DLPack exchange
is zero-copy. CUDA timing includes synchronization, planning, batch reorder,
prefetch and DLPack consumption. Device-to-CPU validation transfer is separate.
Imports, CUDA initialization, sample hashing, oracle decode and pixel hashing
are outside timed requests. Cold access includes saved-index load, mapping
serialization, decoder construction and retrieval. Warm access reuses the
decoder and index with a different seeded request. Storage caches are not
flushed; “cold” describes decoder lifetime, not storage-cold access.

The baseline is torchcodec with the **Stage 0 saved FFprobe mapping**, recreated
and measured in the same worker as the CoreAV path. Mapping identity fields
(PTS, duration, key flag) match for all Stage 0 files; irrelevant SEI side-data
JSON is omitted only from this identity check. Both paths use eight random
frames or two four-frame clips in a single batch. Seeds, five paired repeats,
one decode thread and CPU affinity match Stage 0. Mode order is shuffled per
trial. No conclusions use comparisons with exact mode, the fragmented-file
saved-index regression, or DALI's unequal index behavior.

### Hardware and images

The cloud host reuses the Stage 0 hash-pinned Python 3.13.5 / Debian bookworm OCI base:

```
python:3.13.5-slim-bookworm@sha256:a7cb177dc60dfa77b223192cd0d9e6ce1aac7cf9a569023d68e7c91b6371a718
```

The worker requests and limits **2 physical cores / 8,192 MiB**, selects one
visible CPU by affinity and uses one decode thread. CUDA uses one **NVIDIA L4**.
Stage 0's pinned requirement files (since removed with the Stage 0 harness)
gave the same CPU and CUDA stacks as Stage 0: torch 2.14.1, torchcodec 0.17.0, PyAV 19.0.1 and NumPy
2.5.4. DALI is installed by the reused GPU image but is not measured. The local
wheel is built in release mode with Rust 1.85.1, then installed into both
images; image IDs, source hashes, extension binary hash, linkage, full package
versions, CPU visibility and GPU driver are recorded in the environment JSON.
The cloud host does not pin CPU model, frequency or GPU driver. Matching the allocation
is not proof of matching Stage 0's physical host.

### Inputs and storage

All three complete, hash-pinned Big Buck Bunny encodings come from
Stage 0's sample manifest, reused read-only from its cloud volume (the
manifest went with the Stage 0 harness; the raw reports of
[measurement 0001](../../measurements/0001-baselines.md) record the same URLs
and SHA-256 values). They are three encodings of one film, not independent dataset
categories. Attribution: **(c) copyright 2008, Blender Foundation /
www.bigbuckbunny.org**, CC BY 3.0; media is never committed or remuxed.

| File | Bytes | Frames | Dimensions |
|---|---:|---:|---|
| small | 64,657,027 | 14,315 | 320×180 |
| large | 276,134,947 | 19,036 | 1920×1080 |
| fragmented | 72,756,184 | 14,256 | 1280×720 |

The storage variant uses **actual HTTP GET Range requests to an object server
on loopback in the same cloud container**, with strict 206/Content-Range/ETag
checks and a 10-second client timeout. It runs at native loopback latency and
with an explicitly injected **2 ms server delay per request**. This is a
controlled object-storage model, **not S3, a public bucket or a WAN benchmark**.
The decoder adapters on both sides use the same 256 KiB blocks and 64 MiB LRU
cache. Fresh remote index building uses exact ranges, with no block cache.
All request counts, byte counts, offsets and observed HTTP latencies are saved.
A few index reads and cheap full video loading are separate questions.

The **R2 variant**
reads the same three files from a real **Cloudflare R2** bucket over HTTPS
range GETs. The objects sit under `spike-a/` (413,548,158 bytes stored). Each
upload is checked against the local SHA-256, the single-part MD5 ETag and three
range reads ([upload record](evidence/wan/upload.json)). Requests use SigV4
signing from the Python standard library, `If-Match` on the object ETag and the
same 206/Content-Range checks; there is no SDK. Each request opens a fresh TLS
connection, which matches the loopback adapter's one-connection-per-request
behavior. The bucket's stated (not measured) location is **Asia Pacific**, as
Cloudflare placed it.
The benchmark worker is pinned to the **us-west** region of a cloud provider, so every request crosses the
Pacific. The loaders, the paired design, the 256 KiB block cache and the
oracles are unchanged. The H7 run adds a second worker in the provider's broad
`ap` region, which placed it in **ap-northeast** (GCP). Its one-byte range
latency (about 170–180 ms) shows that this region is closer to the serving
datacenter, not that it shares it. Credentials come from a cloud secret and appear in no
argument, log or evidence file.

## Measured results

The final extension uses **PyO3 0.29.3**. Five paired trials per cell;
latencies are median **[IQR]** in milliseconds. Ratios are median paired
CoreAV/baseline ratios with the bootstrap **95% CI for their mean**.
The interval is not a confidence interval for the median. Full cold/warm
observations, all IQRs and paired ratios are in [summary.json](evidence/summary.json).

### Warm local CPU access

| File | Request | CoreAV ms [IQR] | Mapped baseline ms [IQR] | Paired ratio (95% CI) |
|---|---|---:|---:|---:|
| small | clips | 4.442 [3.823–5.162] | 4.861 [3.466–4.986] | 1.035 (0.898–1.081) |
| small | random | 11.270 [10.771–12.520] | 10.647 [10.004–11.230] | 1.014 (1.006–1.131) |
| large | clips | 917.764 [600.507–960.093] | 885.509 [508.847–923.737] | 1.039 (0.931–1.139) |
| large | random | 4236.609 [3614.599–4511.303] | 4329.510 [2966.425–4548.391] | 0.992 (0.971–1.128) |
| fragmented | clips | 254.878 [249.228–275.564] | 240.662 [236.031–273.559] | 1.007 (0.956–1.047) |
| fragmented | random | 950.476 [889.006–1063.597] | 1037.730 [1004.085–1043.019] | 1.059 (0.902–1.115) |

**0 of 6** warm local CPU file/workload cells meet the outside-noise
interpretation. The JSON index itself reloads more slowly than metadata
index construction on these inputs; no one-hour reload gate was tested.

| File | Index build ms | JSON reload ms | Saved bytes |
|---|---:|---:|---:|
| small | 2.758 | 11.431 | 1,453,253 |
| large | 3.327 | 8.298 | 2,195,076 |
| fragmented | 8.390 | 8.703 | 1,589,262 |

Construction/reload values above are single observations in the CPU worker,
not confidence intervals. Cold request rows remain in the raw report and
summary; cold local CPU CoreAV/baseline paired medians range **1.010–2.377**.
The saved representation and repeated mapping serialization are material
cold costs in this throwaway design.

### Warm HTTP access (CPU)

| File | Request | Added server delay | CoreAV ms | Baseline ms | Paired ratio (95% CI) | Median requests, both paths |
|---|---|---:|---:|---:|---:|---:|
| small | clips | 0 ms | 10.885 | 9.700 | 1.122 (1.077–1.249) | 3 |
| small | random | 0 ms | 28.692 | 33.361 | 0.948 (0.681–1.055) | 8 |
| small | clips | 2 ms | 17.515 | 17.589 | 1.016 (0.938–1.063) | 3 |
| small | random | 2 ms | 52.880 | 52.942 | 0.990 (0.985–1.004) | 8 |
| large | clips | 0 ms | 742.706 | 723.962 | 1.026 (0.951–1.195) | 12 |
| large | random | 0 ms | 4130.594 | 4249.828 | 0.981 (0.972–0.986) | 53 |
| large | clips | 2 ms | 701.301 | 777.443 | 0.952 (0.929–0.996) | 12 |
| large | random | 2 ms | 4146.295 | 4303.770 | 0.969 (0.963–0.990) | 53 |
| fragmented | clips | 0 ms | 236.260 | 239.912 | 0.997 (0.951–1.036) | 5 |
| fragmented | random | 0 ms | 958.782 | 969.938 | 0.969 (0.967–0.982) | 19 |
| fragmented | clips | 2 ms | 237.352 | 248.195 | 1.020 (0.972–1.045) | 5 |
| fragmented | random | 2 ms | 994.972 | 1010.836 | 0.984 (0.966–1.003) | 19 |

Four of twelve warm HTTP model cells have ratio intervals below 1:
large/random at native latency (**0.981**, CI **0.972–0.986**),
large/clips with 2 ms delay (**0.952**, CI **0.929–0.996**),
large/random with 2 ms delay (**0.969**, CI **0.963–0.990**), and
fragmented/random at native latency (**0.969**, CI **0.967–0.982**).
The prior CPU report had two such cells; both reproduce, with two additional
cells in this run. **Request and byte counts match between the two loaders**,
so prefetch establishes no storage-request saving here. These small gains
on same-film loopback controls do not establish a clear general remote-loading
win or the 2-of-4 representative-scenario gate. The HTTP server shares the
worker CPU; host/frequency variation remains uncontrolled.

Opening reads are measured separately on cold rows as
`opening_range_requests`, `opening_range_bytes` and `opening_range_trace`.
`range_requests`, `range_bytes` and `range_trace` cover fetch only on both
cold and warm rows; cold latency includes opening plus fetch. Required HTTP
prefetch planning and JSON parsing are inside the timed CoreAV fetch.
The separate post-fetch plan supplies diagnostics only. Warm rows have zero
opening counters and empty opening traces. Cold opening observations are
identical for both loaders, all trials and both delay controls:

| File | Opening requests | Opening bytes |
|---|---:|---:|
| small | 3 | 693,891 |
| large | 3 | 621,603 |
| fragmented | 1 | 262,144 |


### H7: building the index over HTTP

| File | Range requests | Bytes read | Fraction of object | Largest read | Native HTTP build ms | Build ms with 2 ms added per request |
|---|---:|---:|---:|---:|---:|---:|
| small | 5 | 344,250 | 0.532% | 344,186 B | 83.288 | 25.535 |
| large | 5 | 237,021 | 0.086% | 236,957 B | 18.455 | 30.880 |
| fragmented | 897 | 148,165 | 0.204% | 773 B | 1607.512 | 4053.175 |

These build latencies are one observation per file/control; every individual
request latency and range is retained. Ordinary MP4 needs five bounded
reads. Fragmented MP4 reads only 148,165 bytes but needs **897 requests**
to inspect its fragment metadata, yielding **4,053.175 ms** under the
explicit 2 ms per-request model. Byte efficiency does not establish a
small request count or low storage latency. These loopback results are the
prior record; the R2 results below supersede them for H7.

### R2 object storage (WAN, CPU)

| Run | Samples | Worker | Outcome |
|---|---|---|---|
| `r2-20261012T233643Z` | small only | us-west, AWS | Local orchestration ended during the large sample; partial record kept |
| `r2-20261013T015332Z` | small, large | us-west, Azure | The cloud host cancelled the input during the fragmented index builds, with no application error, out-of-memory or timeout logged; both completed samples pass every oracle |
| `r2-20261013T021306Z` | fragmented | us-west, GCP | Completed; worker exit 0 |

The tables combine the two later runs; each sample is measured exactly once,
and every comparison is paired inside one container. The two containers ran
on different cloud providers inside the cloud host's us-west region, so absolute
latencies are not comparable between samples. Both runs pass the black-box
packet check on all three files (**47,607 / 47,607 packets**). The timed RGB/PTS/DLPack
oracle passes on every sample (**418 unique frames**). In each run, the client's
request and byte totals equal the sum of the saved traces.

**Request latency (round-trip proxy).** Twenty fresh one-byte range GETs per
sample, 60 in total: median **261.6 ms**, p90 **325.7 ms**, p95 **454.3 ms**,
p99 **599.5 ms**, range 208.2–618.8 ms. Per-run medians are 284.7 / 258.9 ms
(Azure: small / large) and 226.4 ms (GCP: fragmented). Each value includes TCP and TLS setup,
SigV4 verification and Cloudflare routing. It is an HTTP latency proxy, not
a network round-trip time, and the bucket location does not identify the
datacenter that served a request.

**Index building over R2 (H7), before and after the read-ahead policy.**
Five paired builds per file with the policy order shuffled; medians with IQR.
Both policies produce byte-identical serialized indexes in every trial.

| File | Policy | Requests | Bytes | Largest read | Build ms [IQR] | Paired after/before ratio (95% CI) |
|---|---|---:|---:|---:|---:|---:|
| small | before | 5 | 344,250 | 344,186 | 1,327.0 [1,291.3–1,342.1] | |
| small | after | **3** | 352,378 | 344,186 | 792.8 [777.1–838.7] | 0.591 (0.575–0.650) |
| large | before | 5 | 237,021 | 236,957 | 1,509.3 [1,384.1–1,537.1] | |
| large | after | **4** | 245,207 | 236,957 | 1,060.3 [1,049.5–1,137.5] | 0.766 (0.693–1.712) |
| fragmented | before | 897 | 148,165 | 773 | 208,864.9 [207,874.9–211,683.0] | |
| fragmented | after | **299** | 1,220,624 | 4,096 | 70,883.1 [70,466.5–72,858.8] | **0.336 (0.317–0.345)** |

The fragmented file has one moof per fragment, and each moof is separated from
the next by about 240 KB of media data. One read per fragment is therefore the
floor for this policy without multi-range requests, which the S3 `GetObject`
API does not support, or concurrent reads. Even after the change, building the
fragmented index takes **about 71 seconds** at this request latency. The large
file's ratio interval is wide because of one slow before/after pair; its
request count falls in every trial.

**Paired loaders over R2.** Five paired trials; CoreAV/baseline medians in
milliseconds with IQR; ratio intervals as above.

| File | Request | Cache | CoreAV ms [IQR] | Mapped baseline ms [IQR] | Paired ratio (95% CI) | Fetch requests, both paths |
|---|---|---|---:|---:|---:|---:|
| small | clips | warm | 794.0 [603.4–842.3] | 849.1 [622.5–880.7] | 0.969 (0.934–1.012) | 3 |
| small | random | warm | 2,600.2 [2,331.2–2,693.3] | 2,764.4 [2,364.4–3,003.0] | 0.980 (0.930–1.005) | 8 |
| large | clips | warm | 5,122.0 [2,796.3–5,380.0] | 4,144.7 [2,727.7–5,075.5] | 1.025 (1.007–1.151) | 12 |
| large | random | warm | 20,324.0 [19,437.1–20,532.6] | 19,187.0 [17,026.5–26,819.1] | 1.070 (0.801–1.194) | 53 |
| fragmented | clips | warm | 1,382.0 [1,145.4–1,395.9] | 1,315.7 [1,143.4–1,332.9] | 1.047 (0.923–1.054) | 5 |
| fragmented | random | warm | 5,276.2 [4,973.8–5,468.5] | 5,362.6 [4,646.0–5,376.0] | 1.022 (0.997–1.054) | 19 |
| small | clips | cold | 1,529.0 | 1,562.0 | 1.003 (0.974–1.043) | 2 + 3 opening |
| small | random | cold | 3,350.3 | 3,666.2 | 1.028 (0.960–1.365) | 9 + 3 opening |
| large | clips | cold | 6,344.7 | 6,366.7 | 0.997 (0.971–1.014) | 14 + 3 opening |
| large | random | cold | 23,263.9 | 20,583.0 | 1.191 (0.795–1.234) | 54 + 3 opening |
| fragmented | clips | cold | 1,663.1 | 1,849.0 | 0.924 (0.851–0.976) | 5 + 1 opening |
| fragmented | random | cold | 6,031.6 | 6,065.5 | 0.953 (0.927–1.024) | 22 + 1 opening |

**0 of 6** warm R2 cells are outside measured noise in CoreAV's favor; large/clips warm is
slower (interval 1.007–1.151). One cold cell, fragmented/clips, has an
interval below 1 (0.924, 0.851–0.976). The pass criterion is about
warm access, so this cell does not count. **Request and byte counts are identical between the two
loaders in every cell.** At about 260 ms per request, remote time is mostly the
number of sequential range requests that torchcodec issues, and the CoreAV plan
does not change that number. Full statistics are in
[evidence/wan/summary.json](evidence/wan/summary.json).

### H7 against a full download

| Run | Worker | Outcome |
|---|---|---|
| `h7-us-west-20261013T042238Z` | us-west, Azure | Small and large complete. Fragmented stopped after three trials: the cloud client cancelled its blocking call after 550 s, with no application error |
| `h7-us-west-20261013T052157Z` | us-west, Azure | Same cancellation at the same point, after 556 s |
| `h7-us-west-20261013T055552Z` | us-west, Azure | Complete; the call is spawned and polled in 60 s steps |
| `h7-ap-20261013T062245Z` | ap-northeast, GCP | Complete |

The tables use the two complete runs. Each run made 3,425 R2 requests and
read 4,161,572,400 bytes through the counting client. Every concurrent and
sequential index equals the local index byte for byte in every trial, and
all byte, request and trace totals reconcile. One-byte range latency medians
are about **248 ms** (us-west) and **174 ms** (ap-northeast). Times are
medians in milliseconds over five paired trials. Ratios are paired
index/best-download medians with the bootstrap 95% CI of the mean.

| Region | File | Index requests | Index bytes (max share) | Concurrent index ms [IQR] | Sequential index ms | Single / parallel download ms | Best download ms | Index/download (95% CI) | H7 |
|---|---|---:|---:|---:|---:|---:|---:|---:|---|
| us-west | small | 3 | 352,378 (0.54%) | 775 [765–802] | 771 | 952 / 982 | 814 | 0.970 (0.885–1.089) | **Fail** |
| us-west | large | 4 | 245,207 (0.09%) | 1,031 [997–1,032] | 1,058 | 3,306 / 17,749 | 3,306 | 0.312 (0.281–0.344) | **Pass** |
| us-west | fragmented | 300 | 1,224,720 (1.68%) | 157,731 [157,485–160,137] | 74,415 | 1,130 / 1,748 | 1,130 | 139.326 (127.193–182.199) | **Fail** |
| ap-northeast | small | 3 | 352,378 (0.54%) | 535 [531–560] | 549 | 1,303 / 1,450 | 807 | 0.639 (0.452–0.732) | **Pass** |
| ap-northeast | large | 4 | 245,207 (0.09%) | 727 [705–742] | 730 | 5,100 / 19,498 | 5,100 | 0.138 (0.120–0.247) | **Pass** |
| ap-northeast | fragmented | 300 | 1,224,720 (1.68%) | 170,209 [169,909–173,088] | 52,616 | 1,522 / 1,375 | 1,375 | 125.875 (105.529–162.530) | **Fail** |

The byte bar holds everywhere: at most **1.68%** of an object, against the
5% limit. The time bar fails on the fragmented file in both regions and on
the small file in us-west, whose interval crosses 1. **H7 fails.**

In both complete containers, concurrency made things slower, for the index
and for downloads alike. Concurrent fragment windows took 15–22 s each
(10th–90th percentile, both containers), against 169–240 ms median for
sequential requests on the same workers. That gap is
consistent with dropped connection attempts being retried. The parallel
download of the large file (33 parts) took 17.7 s and 19.5 s, against
3.3 s and 5.1 s for one GET. The two cancelled us-west containers, on the
same cloud, did not show this. Their three completed fragmented trials each
built the concurrent index in about **10.6 s** (median), against 74–79 s
sequential and 0.8–1.3 s for the downloads. From a host outside that cloud, the same
concurrent build took 3.7 s (p50 284 ms per request). This record does not
isolate the cause. The verdict does not depend on it: even the best
concurrent fragmented build observed is about **ten times** slower than
downloading the whole object, because one window per fragment is still 297
TLS requests.

The regular files' index needs no concurrency, because it has no fragment
hints: three or four sequential reads, the same under both settings.

**Torchcodec opening the same remote object.**

| Region | File | URL approximate ms | URL exact ms (interface bytes) | Reader approximate ms (requests / bytes) | CoreAV index ms (sequential) | Index/URL exact (95% CI) |
|---|---|---:|---:|---:|---:|---:|
| us-west | small | 875 | 1,699 (65.9 MB) | 836 (3 / 693,891 B) | 771 | 0.487 (0.384–0.509) |
| us-west | large | 875 | 4,107 (278.1 MB) | 833 (3 / 621,603 B) | 1,058 | 0.251 (0.208–0.285) |
| us-west | fragmented | 290 | 1,150 (73.1 MB) | 270 (1 / 262,144 B) | 74,415 | 139.391 (119.799–144.359) |
| ap-northeast | small | 664 | 1,870 (68.1 MB) | 630 (3 / 693,891 B) | 549 | 0.291 (0.220–0.308) |
| ap-northeast | large | 668 | 5,181 (287.6 MB) | 600 (3 / 621,603 B) | 730 | 0.133 (0.114–0.180) |
| ap-northeast | fragmented | 203 | 1,339 (75.6 MB) | 195 (1 / 262,144 B) | 52,616 | 127.100 (106.565–189.893) |

The ratio column uses the concurrent CoreAV index. All 30 torchcodec opens per
region succeed with the expected frame counts. Exact mode streams the whole
object (the interface counter is close to the object size) and takes 0.9–1.8
times as long as a single GET. On regular MP4, the CoreAV index is 2–8 times faster
than exact mode and within about 20% of approximate mode, which reads only
the movie header and builds no exact index. On fragmented MP4, approximate
mode reads one 256 KiB block and finishes in 0.2–0.3 s, while exact mode
finishes in 1.1–1.3 s. Both are far faster than CoreAV's fragment-by-fragment
index on this input.

Raw records and logs: [evidence/h7/](evidence/h7/);
[summary](evidence/h7/summary.json), generated by `summarize_h7.py`.

### Warm L4 access (exploratory: RGB oracle failed)

| File | Request | CoreAV ms [IQR] | Mapped baseline ms [IQR] | Paired ratio (95% CI) |
|---|---|---:|---:|---:|
| small | clips | 5.563 [4.450–5.763] | 5.297 [3.856–5.404] | 1.067 (1.031–1.127) |
| small | random | 12.784 [11.942–12.958] | 12.973 [11.759–12.973] | 0.999 (0.974–1.018) |
| large | clips | 215.185 [153.782–224.591] | 214.587 [152.794–223.944] | 1.003 (0.984–1.020) |
| large | random | 1020.437 [734.527–1173.451] | 1030.923 [732.889–1198.347] | 1.002 (0.986–1.004) |
| fragmented | clips | 91.894 [77.167–94.295] | 90.983 [80.212–93.779] | 1.006 (0.940–1.008) |
| fragmented | random | 339.867 [333.212–342.809] | 337.719 [333.344–339.238] | 1.000 (0.976–1.008) |

All CUDA requests complete with CUDA-resident tensors and **zero CPU
fallback**. PTS and DLPack pointer checks pass, and **every paired CoreAV
GPU batch has exactly the same RGB hashes as its mapped GPU baseline**.
However, both paths fail the static CPU-reference RGB oracle: **320
mismatching observations per file**, **960 total**. Maximum absolute
channel error is **2** for all three files. No tolerance is introduced;
`gpu.raw.json` has `success: false`, and the GPU worker exits **1**.
These timings support no output-equivalence speed claim.

Final CPU: **AuthenticAMD, family 25, model 1**, brand masked as unknown,
affinity `[0]`. Final GPU: **L4,
23,034 MiB, driver 610.57.04, CUDA runtime 13.0**. Full visibility and package/linkage details
are in the environment records.

### Verdicts

| Gate/path | Verdict | Evidence and practical limit |
|---|---|---|
| H1, local torchcodec loader | **Fail for the measured local CPU cases** | 0/6 warm cells outside noise; GPU RGB equivalence fails |
| H1, remote loader | **Fail on these inputs** | R2 across the Pacific: 0/6 warm cells outside noise, identical request and byte counts in every cell. Loopback: four small wins in twelve cells. No clear remote win |
| H1, full ML slice | **Inconclusive** | Direct Spike B NVDEC unmeasured; representative four-scenario gate incomplete |
| H7 (threshold relative to the download) | **Fail** | Bytes pass everywhere (at most 1.68% of an object). Time passes for large in both regions and for small in ap-northeast; fails for small in us-west (ratio 0.970, CI up to 1.089) and for fragmented in both regions (126–139 times the download) |
| H7, regular MP4 | **Pass on large; small passes in ap-northeast only** | 3 and 4 sequential reads; 0.5–1.0 s index against 0.8–5.1 s downloads; 2–8 times faster than torchcodec exact mode on the same object |
| H7, fragmented MP4 | **Fail on this input** | 300 requests (3 sequential + one batch of 297); 10.6–170 s depending on the container, against a 1.1–1.4 s download; torchcodec exact mode opens it in 1.1–1.3 s |
| H7, general object-storage claim | **Inconclusive beyond this fail** | One R2 bucket, two worker regions, three encodings of one film |

The measured loader does not justify a faster-local-access claim. The kill
rule's narrowing, to a mapping supplier and a safe, remote-capable demux and
index, is the conservative direction to review; no automatic production scope
change or graduation follows from this PR.

H7 evidence: [run directories and logs](evidence/h7/) and
[summary](evidence/h7/summary.json).
R2 evidence: [run directories and logs](evidence/wan/),
[upload record](evidence/wan/upload.json) and [summary](evidence/wan/summary.json).
Evidence from the earlier runs: [CPU raw](evidence/cpu.raw.json), [CPU environment](evidence/cpu.environment.json),
[CPU status](evidence/cpu.status.json), [GPU raw](evidence/gpu.raw.json),
[GPU environment](evidence/gpu.environment.json), [GPU status](evidence/gpu.status.json),
[paired summary](evidence/summary.json), [superseded dependency CPU run](evidence/pre-upgrade/cpu.raw.json),
[prior CPU measurement](evidence/pre-measurement-fix/cpu.raw.json)
and the failed first startup (its platform log is not published).

## Oracle and scope

The local packet oracle uses the pinned **FFprobe 7.0.2** binary as a black
box: **47,607 / 47,607** video packets match byte position, size, edited PTS/DTS,
duration and keyframe flag. Raw DTS remains on the media timeline and the
adapter applies the edit shift for the comparison. Evidence:
[evidence/local-packets.json](evidence/local-packets.json). This is a selected
video packet check, not whole-stream/corpus harness conformance.

The timed runs independently decode their selected targets sequentially with
the checksum-pinned **FFmpeg 7.0.2** static oracle and hash complete RGB24
frames. The method is equivalent to RGB24 `framemd5` output; selected indices
and reference MD5s are retained in raw JSON. PTS returned by the floating-point
API must round back to the exact expected media tick. There is no RGB error
tolerance and failed GPU hashes remain failures.

A separate local correctness-only fixture generated by the same pinned binary
has **1,440 VFR frames, including 927 B-frames**, with presentation steps of
256 and 768 ticks at timescale 15,360. Reverse-order requests return **all
1,440 exact PTS/RGB hashes**, with identical DLPack producer/consumer addresses
and valid consumers after producer deletion. However, **479 mapping duration
fields differ**: Rust preserves stored sample decode deltas, while the black-box
decoded-frame table reports 256 for those frames. These remain uncovered;
`conformance.py` exits 1 and `passed` is false even though `frame_exact_passed`
is true. No oracle exemption or alternative duration rule was inferred from
FFmpeg behavior. [evidence/conformance.json](evidence/conformance.json) retains
all frames, mismatches, commands and full `framemd5` output. This fixture is
not a fourth ML scenario, a representative corpus, or a production gate pass.

## Unsafe surface for PyO3/DLPack

Exactly **zero CoreAV-authored unsafe blocks, unsafe functions or extern
blocks/functions** are needed by this measured torchcodec → DLPack slice.
There are consequently no file:line entries for explicit unsafe operations.
`src/mp4.rs:2` uses `#![forbid(unsafe_code)]`; parsing remains safe Rust.

The delegated boundaries to review before a follow-up production decision are:

| Location | Boundary and reason |
|---|---|
| `src/lib.rs:11`, `src/lib.rs:15` | PyO3 `FrameIndex` class/method macros; generated CPython FFI is in the vetted dependency |
| `src/lib.rs:110`, `src/lib.rs:115` | PyO3 tensor producer class/method macros and Python reference ownership |
| `src/lib.rs:118` | Safe Python call to tensor `__dlpack__`; stream argument forwarded, producer owns capsule/deleter/storage |
| `src/lib.rs:129` | Safe Python call to `__dlpack_device__`; framework owns device handling |
| `src/lib.rs:145` | PyO3 module macro and generated CPython initialization |

No raw DLPack structures, manually created capsules, raw device pointers,
CUDA context/stream calls, custom deleters or handwritten C FFI are present.
This confines rather than removes dependency/runtime unsafe. Pointer and
lifetime checks exercise CPU/CUDA exchange; they do not prove its soundness.
**Direct Spike B NVDEC-owned CUDA storage was not measured**; this record
cannot establish its required unsafe surface. That boundary must be listed and
reviewed after Spike B integration, before graduation under the unsafe policy. Production
shared-memory worker transfer is also outside this task and remains a Stage 1
follow-up requirement.

## Reproduction

The spike's scripts are not part of the published tree. The runs used a cloud host
with 2 physical cores and 8,192 MiB (CPU runs) or one NVIDIA L4 (GPU runs), the
Stage 0 inputs on a read-only volume and, for the R2 and H7 runs, a read-only
bucket credential that appears in no argument, log or evidence file. Outputs were
written only to the spike's own volume and evidence directory, and no recurring
app was deployed.

The R2 worker timeout was 3,600 s, with a 180 s startup timeout and a 3,300 s
measurement subprocess; the local wall timeout was 4,200 s; `retries=0`. The H7
worker timeout was 2,400 s, with a 180 s startup timeout and a 2,200 s
measurement subprocess; each torchcodec open has 420 s and each R2 request
10 s (60 s for download requests). The local wall timeout was 3,000 s and
`retries=0`. The client spawns the call and polls it in 60 s steps, records
the function call ID, and stops polling after the function and startup
timeouts. The R2 client caps a run at 20,000 requests and 6 GiB.

No earlier output was overwritten: each explicit rerun wrote to a new directory
and kept every previous attempt. The function timeouts were CPU 1,800 s / GPU
1,200 s, startup 600 s, workers 1,450 s / 1,000 s, oracle calls 240 s, one
container per function, `retries=0`, and idle window 2 s. Calls were
sequential. Platform startup recovery can still occur independently of
application retries; the failed startup log records that behavior.

Local packet verification requires the downloaded Stage 0 media in a directory
and the pinned FFprobe binary:

```bash
.venv/bin/python check_index.py --data-dir /path/to/bench/data --ffprobe /path/to/ffprobe --output ../../docs/spikes/spike-a/evidence/recheck-packets.json
```

The optional local VFR check uses the same CPU packages and checksum-verified
oracle binaries. These commands expose PyAV's wheel libraries inside the
spike and regenerate the fixture:

```bash
.venv/bin/pip install --only-binary=:all: --extra-index-url https://download.pytorch.org/whl/cpu \
    av==19.0.1 numpy==2.5.4 torch==2.14.1+cpu torchcodec==0.17.0+cpu
.venv/bin/python local_setup.py
../../scripts/install-oracle-ffmpeg.sh "$PWD/build/oracle"
LD_LIBRARY_PATH="$PWD/build/shared" .venv/bin/python conformance.py --ffmpeg "$PWD/build/oracle/ffmpeg" --ffprobe "$PWD/build/oracle/ffprobe" --output ../../docs/spikes/spike-a/evidence/recheck-vfr.json
```

Exit 1 is currently expected because of the uncovered duration differences. Generated
media stays under ignored `data/`. No local timings are performance evidence.

## Verification

Workspace formatting, clippy with `-D warnings`, tests and full cargo-deny
checks pass. The excluded spike separately passes formatting, clippy, 16 Rust
boundary/ordering/read-ahead/concurrency tests and full cargo-deny using the repository
configuration. Twenty Python regressions cover ownership/stream forwarding, stale
indexes, HTTP/cache integrity, paired-request rejection, R2 range integrity, SigV4 header and presigned-URL signing, secret and URL
redaction, request/byte caps, merging per-sample runs,
bounded and ordered concurrent batches, download integrity, and the H7
verdict rule. The Rust tests cover sidx, sparse tfra and mfra hints, malformed
hints, zero and small speculative byte budgets, concurrency 0 and 65
rejection, and one batch per hint set. Every index is identical to the
exact-read policy. The same 20 Python tests also ran inside each H7 worker.
A presigned URL and an 8-part parallel download were checked against the
real bucket before the run. Cargo-deny reports unused allowlist entries and duplicate syn versions as warnings. Exact local
command/result records are retained in `evidence/verification.json`.

No production API, no fuzz campaign, no adversarial GPU decoder isolation,
no open-GOP/recovery-point parser, no one-hour index reload gate, and no full
100-file corpus/backend conformance gate are claimed.

## Open questions and follow-up work

- Integrate Spike B, measure its loader and list its direct CUDA/DLPack unsafe
  surface; record the follow-up decision before graduation.
- Resolve the stored-sample versus decoded-frame duration contract on VFR with
  spec-derived policy and oracle evidence. The mismatch is not waived here.
- Name the four representative scenarios and add robot-learning and phone VFR
  inputs before stage 4's stage-gate verdict. Repeat on S3 or GCS and
  with objects of other sizes, an open question of the H7 threshold.
- Whether the measured H1 path merits the kill rule's narrowing of the
  loader and samplers is the maintainer's stage-gate decision, not taken yet. The H1
  re-measurement passed (`docs/spikes/h1-gpu/`), and stage 4 (ML) is
  deferred with that result as its starting point (`37d81753`).
- Fragmented remote indexing is still one request per fragment. Concurrent
  windows cut the build from 52–79 s to about 10.6 s in some containers, but
  made it two to three times slower in others, and it never beat a 1.1–1.4 s full
  download. A fragmented H7 pass would need far fewer requests per object,
  such as connection reuse, a stored CoreAV index, or reading large contiguous
  spans, or else a decision that fragmented inputs are indexed after download.
  Isolating the concurrent connection-setup delays seen in two cloud containers
  is a separate follow-up.
  The hardened crate now routes fragmented files: a usable
  segment index gives one bounded window per subsegment under a hard 5% byte
  cap, and other heavily fragmented files are routed to download-then-index,
  outside the H7 claim. The byte-accounting census on corpus v1 and the
  time measurement are in
  [`h7-sidx/`](h7-sidx/README.md). The time bar still fails there (1 of 11
  held-out files passes in each region): the index is bound by about
  160–225 ms per R2 round trip.
- Remote loading time follows torchcodec's sequential range requests;
  a remote win would need fewer or concurrent requests, not a better plan.

## Clean-room sources

Besides the project's own documents, only public specifications
and documented dependency APIs were used. No multimedia implementation source, repository history, old tag,
or archived prototype was read or copied. Existing CoreAV benchmark/oracle
scripts are test orchestration patterns, not parser design specifications.

- [ISO/IEC 14496-12:2012 document](https://ossrs.io/lts/zh-cn/assets/files/ISO_IEC_14496-12-base-format-2012-b70dd5f101daecd072700609842c9649.pdf), sections 6.2, 8.2–8.8, 8.8.9–8.8.11 and 8.16.3: boxes, sample tables, edits, fragments, mfra/tfra/mfro and sidx. Only the standard PDF was consulted at this host.
- [Cloudflare R2 S3 API](https://developers.cloudflare.com/r2/api/s3/api/) and [AWS Signature Version 4](https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html) with [query-string authentication](https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-query-string-auth.html): range GET, conditional requests, request signing and presigned URLs.
- [FFmpeg TLS protocol options](https://ffmpeg.org/ffmpeg-protocols.html#tls): peer verification and CA file, used only to configure the black-box decoder's trust store.
- [Apple QuickTime File Format](https://developer.apple.com/documentation/quicktime-file-format): atoms and sample-table semantics.
- [Torchcodec custom mappings API](https://meta-pytorch.org/torchcodec/main/generated_examples/decoding/custom_frame_mappings.html) and [VideoDecoder](https://meta-pytorch.org/torchcodec/main/generated/torchcodec.decoders.VideoDecoder.html): public mapping and file-like interfaces, tested on pinned 0.17.0 binaries.
- [PyO3 0.29.3 API](https://docs.rs/pyo3/0.29.3/pyo3/), [PyTorch DLPack](https://docs.pytorch.org/docs/stable/dlpack.html), [DLPack Python protocol](https://data-apis.org/array-api/latest/design_topics/data_interchange.html): safe forwarding, ownership and stream protocol.
- [FFmpeg CLI/framemd5 documentation](https://ffmpeg.org/ffmpeg-formats.html#framemd5): black-box test oracle only.

# 0001: Stage 0 tool baselines on a cloud host

This run measures a subset of the Stage 0 baselines. No CoreAV
library performance is measured and Stage 0 remains incomplete. The previous
shared-VM observations have been replaced completely by this cloud run.

Persisted mappings took **0.282×**, **0.939×**, and **2.311×** exact-mode time
for cold eight-frame CPU access on the small, large, and fragmented inputs.
These are medians of five matched-request ratios: mappings help two inputs and
slow the third. There is no universal faster-access claim or pass/kill verdict.

## Reproduction and evidence

The run's harness (the Stage 0 scripts that were under `bench/`: the benchmark
app, the worker, the ratio, publication and verification scripts, the sample
manifest and the pinned requirements) has been removed from the repository:
it measured the earlier tools, not CoreAV. Its results stay here, as the run
published them, and the raw reports below record the commands, tool versions,
sample URLs and hashes of every run. The exact oracle
URL/checksum were read from the authorized Stage 0 installer script; no multimedia implementation source was used.

- [CPU raw evidence](0001-baselines.raw.json): all repeats, resource usage, commands,
  tool versions, metadata, hashes, requests, timing splits and environment.
- [CPU environment](0001-baselines.cpu-environment.json) and [completion status](0001-baselines.cpu-status.json).
- [CPU target-frame oracle](0001-baselines.oracle.json).
- [Full trace stderr](0001-baselines.traces.json.gz): lossless compressed JSON, linked
  by sample and SHA-256 from the raw report. Selected box diagnostics remain inline.
- The platform logs and cost records of the run are not published.

The publication step moved only verbose trace stderr into gzip and recorded
its hash; it changed no timings or observations. The raw schema is version 2.
Paths inside the checkout are repository-relative; Volume paths identify the
actual cloud-host mount. Media verification checked all saved media hashes before
decoding.

## Inputs

Complete, hash-pinned encodings of Big Buck Bunny; no trimming or remuxing.
The raw reports' `samples` record each input's URL, SHA-256, license and
attribution. Blender grants [CC BY 3.0](https://peach.blender.org/about/).
Attribution: **(c) copyright 2008, Blender Foundation / www.bigbuckbunny.org**.
The complete fragmented representation is identified by its [DASH manifest](https://dash.akamaized.net/dash264/TestCases/1a/qualcomm/1/MultiRate.mpd).

| Sample | Media bytes | Video | Frames / keyframes |
|---|---:|---|---:|
| small | 64,657,027 | h264, 320×180 | 14,315 / 1,194 |
| large | 276,134,947 | h264, 1920×1080 | 19,036 / 143 |
| fragmented | 72,756,184 | h264, 1280×720 | 14,256 / 297 |

These are three encodings of one film, not three independent real-world
workloads. Resolution, GOP length and encoder settings differ; this is not a
controlled test of fragmentation. No robot episode or VFR phone input ran.

## Environment

CPU measurements began on 2026-10-12.
The cloud host requested and limited **2 physical CPU cores and 8,192 MiB RAM** per
function. The CPU process used affinity `[0]`, and `nproc` after affinity was
**1**; the container exposes two CPUs before affinity. CPU vendor is
**AuthenticAMD, family 25, model 1**. Both `/proc/cpuinfo` and `lscpu` report
`model name: unknown`; the cloud host masks the brand name. This is a recorded hardware
visibility gap, not an inferred processor model. CPU frequency is not pinned.

Base: **Python 3.13.5, Linux amd64, Debian bookworm**.
Pinned OCI base digest:

```text
python:3.13.5-slim-bookworm@sha256:a7cb177dc60dfa77b223192cd0d9e6ce1aac7cf9a569023d68e7c91b6371a718
```

The cloud platform identifies derived images by an image ID rather than a final
OCI SHA-256. The base digest, pinned packages and complete source checksums are
saved in the environment.

CLI FFmpeg/ffprobe: **7.0.2 static**, archive:

```text
https://johnvansickle.com/ffmpeg/releases/ffmpeg-7.0.2-amd64-static.tar.xz
abda8d77ce8309141f83ab8edf0596834087c52467f6badf376a6a2a4c87cf67
```

Tools: **PyAV 19.0.1**, **PyTorch 2.14.1+cpu**, **torchcodec 0.17.0+cpu**,
**NumPy 2.5.4**, and **cloud client 1.6.1**. Shared libraries for PyAV and
CPU torchcodec come from the pinned PyAV wheel: **libavcodec/libavformat
63.1.102, libavutil 61.1.102, libswscale 10.1.102**. These differ from the
static CLI oracle. Full version banners, `pip freeze`, `pip check`, library
linkage, `/proc/cpuinfo`, `nproc`, `lscpu`, memory information, affinity and
platform information are in the evidence. No Rust dependency changed.

## Method

Five trials (seed 20261012) fetch eight random frames or two four-frame clips.
Cold means a fresh decoder/index; warm means a different request on the same
object. Whole-file hash checks precede timing. Files are on a cloud volume;
no filesystem cache is flushed. Storage-cold behavior and explicit remote
range requests are not measured. Imports, CUDA context initialization, hashing
and oracle verification are outside request timing.

CPU tools share one container and use one CPU affinity and one decoder thread.
Mode/workload ordering is shuffled within each trial, but tools run in separate
sequential workers. Ratios pair the same trial and requested frame indices;
bootstrap intervals include request/GOP variation and timing variation. They
do not remove temporal drift, interference from other tenants, or host-to-host
uncertainty. The cloud host resource requests and limits are fixed; CPU frequency and
host placement are not pinned. No decoder-parity claim or Stage 0 pivot rule
is evaluated.

CLI FFmpeg/ffprobe and the RGB oracle use the exact 7.0.2 static archive pinned
by an earlier pull request. Torchcodec cannot load a static CLI as a shared library. Both PyAV and
torchcodec therefore use the libraries in the pinned PyAV 19.0.1 wheel, with
libavcodec/libavformat 63.1.102 and libavutil 61.1.102. These ratios compare
complete tool stacks. The binary benchmark tools are outside the Cargo
workspace and are not CoreAV runtime dependencies.

Torchcodec batches all eight indices in `get_frames_at`; exact scans on open,
approximate trusts header metadata, and mappings reads the saved JSON bytes.
The persisted mapping is built once by a full FFmpeg CPU decode; its cost is
recorded separately and excluded from repeated mapped access. PyAV reads the
saved PTS table, seeks to explicit preceding keyframes, decodes until each
requested PTS, and converts targets to RGB24. It does not implement torchcodec's
GOP batching. The packet-table proxy includes payload I/O, Python allocations,
JSON serialization and file writes; it is not a CoreAV persistent frame index.

GPU request time includes synchronized decode and conversion to RGB device
memory. Transfer to CPU is timed separately before hashing. Torchcodec's
CUDA device uses its documented default NVDEC backend. DALI uses the mixed
H.264 decoder with a preloaded encoded buffer and dynamically selected frames,
`build_index=True`, batch size one and one CPU thread. Warm DALI requests reuse
the pipeline, but the in-memory decoder rebuilds its index for each input;
this is not equivalent to torchcodec's cached index. DALI does not expose PTS.


## CPU comparisons

Ratios show median (bootstrap 95% CI for the mean of five paired ratios).
Values below 1 mean the numerator took less time. Requests match by trial and frame indices.

| Sample | Request | Cache | Comparison (numerator / denominator) | Time ratio |
|---|---|---|---|---:|
| small | random | cold | torchcodec/mappings ÷ torchcodec/exact | 0.282 (0.269–0.349) |
| small | random | cold | pyav/cached_pts ÷ torchcodec/exact | 0.296 (0.269–0.982) |
| small | random | warm | torchcodec/mappings ÷ torchcodec/exact | 1.134 (0.947–1.147) |
| small | random | warm | pyav/cached_pts ÷ torchcodec/exact | 1.159 (1.058–1.177) |
| small | clips | cold | torchcodec/mappings ÷ torchcodec/exact | 0.258 (0.240–0.269) |
| small | clips | cold | pyav/cached_pts ÷ torchcodec/exact | 0.236 (0.212–0.264) |
| small | clips | warm | torchcodec/mappings ÷ torchcodec/exact | 0.986 (0.806–2.595) |
| small | clips | warm | pyav/cached_pts ÷ torchcodec/exact | 1.151 (0.872–1.193) |
| large | random | cold | torchcodec/mappings ÷ torchcodec/exact | 0.939 (0.915–0.946) |
| large | random | cold | pyav/cached_pts ÷ torchcodec/exact | 0.943 (0.931–0.973) |
| large | random | warm | torchcodec/mappings ÷ torchcodec/exact | 1.006 (0.992–1.008) |
| large | random | warm | pyav/cached_pts ÷ torchcodec/exact | 1.017 (1.005–1.022) |
| large | clips | cold | torchcodec/mappings ÷ torchcodec/exact | 0.866 (0.749–0.870) |
| large | clips | cold | pyav/cached_pts ÷ torchcodec/exact | 0.842 (0.769–0.858) |
| large | clips | warm | torchcodec/mappings ÷ torchcodec/exact | 1.010 (0.997–1.054) |
| large | clips | warm | pyav/cached_pts ÷ torchcodec/exact | 1.042 (1.023–1.076) |
| fragmented | random | cold | torchcodec/mappings ÷ torchcodec/exact | 2.311 (2.197–2.576) |
| fragmented | random | cold | pyav/cached_pts ÷ torchcodec/exact | 2.509 (2.215–2.606) |
| fragmented | random | warm | torchcodec/mappings ÷ torchcodec/exact | 2.947 (2.630–3.305) |
| fragmented | random | warm | pyav/cached_pts ÷ torchcodec/exact | 2.692 (2.635–3.355) |
| fragmented | clips | cold | torchcodec/mappings ÷ torchcodec/exact | 1.606 (1.465–1.727) |
| fragmented | clips | cold | pyav/cached_pts ÷ torchcodec/exact | 1.652 (1.491–1.788) |
| fragmented | clips | warm | torchcodec/mappings ÷ torchcodec/exact | 2.786 (2.608–4.372) |
| fragmented | clips | warm | pyav/cached_pts ÷ torchcodec/exact | 3.063 (2.867–4.660) |

Probe ratios are ratios of medians (95% bootstrap CI for the ratio of means),
resampling the ten CLI and five in-process observations independently.
Packet rebuild/load ratios pair the five trials and use the interval method above.

| Sample | ffprobe CLI probe / PyAV open | PyAV packet rebuild+save / JSON load |
|---|---:|---:|
| small | 5.086 (3.080–5.806) | 9.937 (9.537–10.570) |
| large | 3.213 (2.177–3.662) | 16.737 (14.736–17.947) |
| fragmented | 9.733 (3.842–13.499) | 8.912 (7.847–10.093) |

The CLI probe includes process launch and JSON output; PyAV open includes
container parsing, stream discovery and codec setup. This does not isolate
parsing cost or compare equal FFmpeg builds. The packet proxy is a local
Python/JSON cache comparison, not the one-hour native-index requirement.

### Optimistic construction ceiling

The median of `open_ms / total_ms` for exact cold requests is the most that
could be removed by eliminating **all** observed construction. It is optimistic
because some open/setup work remains necessary. It excludes other H1 levers.

| Sample | Eight random frames | Two four-frame clips |
|---|---:|---:|
| small | 89.9% | 94.6% |
| large | 7.2% | 19.8% |
| fragmented | 14.7% | 34.7% |

## Accuracy

CPU exact, mapped and indexed PyAV results had **zero PTS mismatches** and
**zero RGB mismatches** against the pinned FFmpeg 7.0.2 reference: **418 unique
file/frame pairs**, and 160 observations per backend/mode per file (duplicates
from repeated workloads count as observations). Approximate mode had **one PTS
and RGB mismatch on large**, at index 2043, and none on the other files.
Approximate timings are informational raw evidence, not an exact-access baseline.
This is below the separate 1,000-frame backend conformance gate.

## GPU experiment (exploratory; RGB equivalence failed)

The final run used **one NVIDIA L4, 23,034 MiB reported device memory**,
**driver 580.95.05**, **CUDA driver support 13.0**, and **PyTorch runtime
CUDA 13.0**. L4 was chosen as an inexpensive NVDEC GPU compatible with the
pinned CUDA 13 wheels and large enough for full-resolution H.264 batches.
CPU request/limit and memory request/limit remained 2 cores and 8,192 MiB.
The GPU container reported **GenuineIntel, family 6, model 85, model name
unknown**; two visible CPUs before affinity, effective worker affinity `[0]`.
GPU drivers and CPU placement are platform-managed rather than pinned.

GPU packages: **torch 2.14.1+cu130, torchcodec 0.17.0+cu130, DALI 2.3.0
(nvidia-dali-cuda130), NumPy 2.5.4**. Full versions and linkage are recorded.
The GPU image has the same pinned OCI base digest as the CPU image.

- [Final GPU raw evidence](0001-baselines.gpu.raw.json): hardware, every row,
  GPU tensor location, CPU-fallback status, transfer and pixel-error diagnostics.
- [Final GPU RGB oracle](0001-baselines.gpu.oracle.json).
- [First GPU raw attempt](0001-baselines.gpu.first.raw.json) and [its oracle](0001-baselines.gpu.first.oracle.json).

The first GPU attempt used driver **610.57.04**, CUDA driver support **13.3**,
and runtime CUDA **13.0**. DALI 1080p hit its 180-second timeout. The final
GPU attempt kept five trials and extended workers to 300 seconds and the
function timeout from 900 to 1,200 seconds. **DALI 1080p still timed out and is
not run as a completed baseline**. Both exit-code-124 attempts remain visible;
neither failed process time is used in a summary. No third paid GPU attempt ran.

Torchcodec completed all three inputs with **zero CPU fallback**, CUDA-resident
RGB tensors and **zero PTS mismatches** in exact/mapped modes. DALI completed
small and fragmented, but its operator does not expose PTS. All observed GPU
RGB hashes differ from the static CPU reference, including exact/mapped
torchcodec and indexed DALI. The GPU oracle exits **1**, and the overall GPU
`success` flag is **false** because DALI large did not complete. This is not a
green GPU correctness or conformance result. The completed API timing rows
below are exploratory observations, with no output-equivalence speed claim.

Fresh FFmpeg 7.0.2 reference RGB pixels were decoded **outside timed requests**
and compared per frame. The diagnostics set no acceptance threshold and do
not diagnose the cause of differences. Exact/mapped torchcodec errors are
bounded by 2 on a 0–255 channel scale; DALI differs more. Approximate-mode
errors are excluded from this table because that mode may return another frame.

| Sample | Backend | Maximum absolute RGB error | Mean absolute RGB error |
|---|---|---:|---:|
| small | torchcodec_cuda | 2 | 0.666 |
| small | dali_gpu | 62 | 1.963 |
| large | torchcodec_cuda | 2 | 0.609 |
| large | dali_gpu | Not run: 300-second timeout | Not run |
| fragmented | torchcodec_cuda | 2 | 0.654 |
| fragmented | dali_gpu | 57 | 1.682 |

Ratios show median (bootstrap 95% CI for the mean of five paired ratios).
Values below 1 mean the numerator took less time. Requests match by trial and frame indices.

| Sample | Request | Cache | Comparison (numerator / denominator) | Time ratio |
|---|---|---|---|---:|
| small | random | cold | dali_gpu/indexed ÷ torchcodec_cuda/exact | 22.531 (20.276–24.146) |
| small | random | cold | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 0.334 (0.323–0.346) |
| small | random | warm | dali_gpu/indexed ÷ torchcodec_cuda/exact | 224.903 (188.903–235.907) |
| small | random | warm | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 0.986 (0.969–1.020) |
| small | clips | cold | dali_gpu/indexed ÷ torchcodec_cuda/exact | 23.479 (12.876–24.729) |
| small | clips | cold | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 0.288 (0.220–0.295) |
| small | clips | warm | dali_gpu/indexed ÷ torchcodec_cuda/exact | 377.480 (205.030–504.758) |
| small | clips | warm | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 0.952 (0.938–1.014) |
| large | random | cold | dali_gpu/indexed ÷ torchcodec_cuda/exact | Not run: timeout |
| large | random | cold | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 0.783 (0.745–0.814) |
| large | random | warm | dali_gpu/indexed ÷ torchcodec_cuda/exact | Not run: timeout |
| large | random | warm | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 1.002 (0.995–1.006) |
| large | clips | cold | dali_gpu/indexed ÷ torchcodec_cuda/exact | Not run: timeout |
| large | clips | cold | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 0.547 (0.439–0.605) |
| large | clips | warm | dali_gpu/indexed ÷ torchcodec_cuda/exact | Not run: timeout |
| large | clips | warm | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 1.000 (0.987–1.010) |
| fragmented | random | cold | dali_gpu/indexed ÷ torchcodec_cuda/exact | 50.690 (43.275–51.791) |
| fragmented | random | cold | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 1.820 (1.693–1.882) |
| fragmented | random | warm | dali_gpu/indexed ÷ torchcodec_cuda/exact | 84.115 (78.632–99.471) |
| fragmented | random | warm | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 3.085 (2.734–3.176) |
| fragmented | clips | cold | dali_gpu/indexed ÷ torchcodec_cuda/exact | 74.816 (44.113–87.552) |
| fragmented | clips | cold | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 1.002 (0.768–1.017) |
| fragmented | clips | warm | dali_gpu/indexed ÷ torchcodec_cuda/exact | 215.748 (114.033–465.622) |
| fragmented | clips | warm | torchcodec_cuda/mappings ÷ torchcodec_cuda/exact | 2.834 (2.687–4.798) |

Retrieval ends after GPU synchronization and leaves RGB data on device.
`total_ms` includes cold construction and retrieval; transfer, hashing and
pixel diagnostics are outside it. Median GPU-to-CPU transfer times below
aggregate exact/mapped torchcodec rows or indexed DALI rows, each returning
eight images at the same resolution. No cross-container CPU/GPU speedup is
published, and DALI ratios do not represent a cached `fn.readers.video` baseline.

| Sample | torchcodec GPU-to-CPU transfer (ms) | DALI GPU-to-CPU transfer (ms) |
|---|---:|---:|
| small | 0.435 | 2.271 |
| large | 22.625 | Not run |
| fragmented | 4.483 | 15.299 |

Worker peak RSS includes imports, all trials and, on GPU, transfer/reference
validation. It is not incremental decoder memory per concurrent request.

## Runtime

The observed remote-call durations were 745.0 s (CPU) and 1714.5 s (GPU) across
both attempts. The first image packaging failed before any benchmark ran; its log
is not published. No recurring app or scheduled job was deployed.

## Gaps and hypothesis scope

Not run: DALI 1080p GPU as a completed baseline (180/300-second timeouts);
CPU DALI H.264 (mixed/GPU backend required); cached DALI video readers;
robot-learning episodes; VFR phone footage; a representative
multi-source corpus; one-hour or 2 GB inputs; explicit object-storage range
reads and request/byte counts; storage-cold measurements; a minimal whitelist
FFmpeg build; sandboxed FFmpeg; thumbnails and image encode; concurrent request
memory/throughput; production p99; native CoreAV code; non-reference discard;
explicit GOP-aware planning; isolated seek/discard/target costs inside
torchcodec; NVDEC session-cache controls; sustained sequential decode
throughput; 1,000-frame backend conformance; native decoder parity; hostile
media/fuzzing; demand scan and consumer adoption.

H1/H9 receive local mapping-consumer evidence. H4 receives CLI versus in-process
probe context. H2/H3 (native NVDEC/native decoder implementations), H5 (sandboxed
thumbnail cost), H6 (safe-parser corpus agreement), H7 (range reads) and H8
(adoption) receive no gate evidence. Stage 0 remains incomplete.

## Verification

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, and `cargo deny check` passed. The placeholder has no
Rust tests; deny reports existing unused-license-allowance warnings. Python
compilation and 17 unittest regressions passed. Local cloud-host venv `pip check`
passed, as did the image package checks. CPU benchmarks and the separate CPU
RGB oracle passed. GPU timing completion is partial and GPU RGB equivalence
failed as detailed above. No fuzzing or native CoreAV conformance was run.


Hardware visibility gaps remain: masked CPU brand names, unpinned frequencies,
platform-managed GPU drivers, and no host-to-host replication.

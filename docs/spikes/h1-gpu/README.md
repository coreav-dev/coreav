# Stage 2 W2: H1 with direct NVDEC in the Spike A loader

Throwaway spike (spikes live outside the workspace, thresholds are fixed before a
run). It re-measures **H1**, "a persistent index and seek plans remove a material share of
end-to-end random-access time in real ML workloads", with Spike B's decoder
replacing torchcodec decoding in Spike A's loader (index, seek plan,
DLPack export).

Scenarios, corpus rule, baseline, timed span, statistic and pass rule were fixed before any
cloud run (commit `0c784ba`).
The pass rule:

- a scenario **wins** when the upper end of the 95% interval of the geometric-mean paired time
  ratio (CoreAV / faster torchcodec mode for that file) is below **0.90**;
- **H1 passes** if at least 2 of S1–S4 win, S2 (32 frames) is one of them, and no scenario's ratio
  is above **1.10**.

## Result: H1 passes its pre-registered rule

**Verdict (pre-registered rule): PASS.** All four scenarios win, S2 among them, and none is
slower. This is a spike result. Graduation and any scope change are the maintainer's
stage-gate verdict, not given yet: stage 4 (ML) is deferred while the order of
work toward a drop-in replacement has priority, with this result as its starting point (`37d81753`).

Ratio = geometric mean of per-trial CoreAV time / time of the file's faster torchcodec mode,
over both machines; interval = 95% percentile bootstrap resampling videos (10,000 draws,
seed 20261013). Times are medians in milliseconds per request, construction included.

| Scenario | Paired trials | CoreAV ms | torchcodec mappings ms | torchcodec exact ms | Ratio (95% CI) | Machine 0 / 1 | Ratio vs exact mode | Verdict |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| S1, 16-frame clip, stride 4 (H.264 720p, 15 videos) | 60 | 111.2 | 135.3 | 155.6 | **0.814 (0.756–0.868)** | 0.809 / 0.819 | 0.698 | **win** |
| S2, 32 evenly spaced (H.264 1080p long, 13) | 52 | 1199.6 | 1417.9 | 1595.3 | **0.840 (0.800–0.877)** | 0.839 / 0.841 | 0.765 | **win** |
| S3, 1–4 by timestamp (AV1 LeRobot, 13) | 52 | 23.5 | 58.9 | 247.3 | **0.392 (0.387–0.397)** | 0.393 / 0.391 | 0.107 | **win** |
| S4, 64 consecutive (H.265 1080p, 13) | 52 | 78.4 | 96.3 | 96.5 | **0.842 (0.831–0.853)** | 0.834 / 0.850 | 0.799 | **win** |

S2 with 8 frames: 0.800 (0.760–0.844); with 64 frames: 0.862 (0.838–0.888).

Win bar: upper end below 0.90. Slower bar: ratio above 1.10 (no scenario). The faster
torchcodec mode was the mapping mode for every file except 7 of 26 file-machine pairs in S4,
where exact mode was faster. Full statistics: [evidence/summary.json](evidence/summary.json),
from `summarize.py` over the two raw results.

**What the times contain.** Each request opens the file from scratch on both sides: CoreAV
loads its saved index and plans; torchcodec constructs a `VideoDecoder` (reading the mapping
JSON, or scanning in exact mode). Both then decode on NVDEC and return synchronized CUDA RGB.
In S3 (1–4 frames from LeRobot files that concatenate many episodes, 18,750–88,716 frames per
file, GOP 2) almost all time is that per-request setup, which is why its ratio (0.39) is far
lower than the others and why exact mode, which scans the whole file per request, takes about four times as long as mapping mode there. In S1, S2 and S4 decoding
dominates, and CoreAV is 16–19% faster end to end while decoding the planned packets with Spike B's
pipelined decoder, which maps only target pictures. The measurement does not separate setup
from decode, so it does not attribute the gain between the index and the decoder.

**Correctness before timing.** On each machine every frame of every planned request was
decoded by the CoreAV loader and compared with FFmpeg 7.0.2's CPU decode in NV12: 3,428 and
3,377 frames, **0 mismatches**, so no video was excluded. Every returned timestamp of both
torchcodec modes and of CoreAV equalled the index in every trial; torchcodec reported no CPU
fallback. CoreAV and torchcodec use different YUV-to-RGB conversion: per video the maximum
channel difference is 1 on 53 of 54 videos; on `S1-01` it is 172 and 98 (mean 1.1 and 0.8),
which was not investigated further (CoreAV's NV12 for those frames matches the CPU oracle).

**Machines.** Two separate cloud runs, each one container with one NVIDIA L4 (driver
610.57.04, 23,034 MiB), both placed by the cloud host in GCP us-east, torch
2.14.1+cu130, torchcodec 0.17.0+cu130 with the FFmpeg libraries of the PyAV 19.0.1 wheel
(libavcodec 63.1.102), oracle FFmpeg 7.0.2 static. CPU model, clocks and host placement are not
pinned. Raw records: [machine 0](evidence/measure-0-3-20261013T190338Z/result.json),
[machine 1](evidence/measure-1-3-20261013T192050Z/result.json).

**Corpus.** 54 videos ([manifest](evidence/prepare-20261013T134143Z/manifest.json)):
S1–S2 unmodified NASA public-domain H.264 MP4s, S3 unmodified LeRobot AV1 MP4s (Apache-2.0
datasets, 224×224 or 640×480), S4 libx265 re-encodes of NASA 1080p sources. Eleven ranked
candidates were skipped by the pre-registered eligibility checks (nine edit lists that the
spike index cannot represent exactly in media ticks, two packet tables that differ from
FFprobe), all recorded in the manifest.

## AV1 bring-up (before any timing)

The pre-registered plan left the header's open AV1 conventions to an exactness-only bring-up.
Every attempt is kept under `evidence/`.

1. `bringup-20261013T143349Z`: every convention process exited without output; the pinned
   FFmpeg 7.0.2 build has no SVT-AV1, so libaom fixtures replace the SVT-AV1 ones.
2. Diagnostics `debug-*` (one request per process, `faulthandler`, driver-call tracing):
   - H.264 and H.265 requests decode and match the CPU decode.
   - AV1 key frames are bit-exact with (start, end) tile offset pairs into the tile-group
     payload (4-tile frames included); start-only offsets are not.
   - AV1 inter frames crashed inside `cuvidDecodePicture`. Field overrides showed the crash
     needs both a nonzero decode index and a nonzero reference index. `ref_frame[i].index`
     must hold the reference's decode surface (slot indices crash at once).
3. `debug-1-20261013T182557Z`: NVIDIA's own AV1 parser from the driver, run as a black box
   through the public `nvcuvid.h` interface on the same streams, with its decode callback only
   recording what it passes (parser_probe.py; nothing is decoded). It passes
   the **decode surface of the primary reference in `primary_ref_frame` (0xFF for none)**,
   not the specification's 0–7 index, surface indices in `ref_frame_map` and
   `ref_frame[i].index` (0xFF for empty slots), and identity global motion on intra frames.
   On the compared frames the rest of our structure matched the parser's byte for byte, except
   fields the parser carries over into key frames that key frames do not use (high-precision
   motion vectors, interpolation filter).
   Only this interface convention was taken from the probe; no decoding logic.
4. `bringup-20261013T184223Z`: with those conventions, the remaining open choice is the CDEF
   secondary strength. Coded values (0–3) are exact on **960 of 960** frames: three libaom
   fixtures (GOP 2; GOP 60 with hidden and shown-existing frames; four tiles at 1280×720) and
   the first 60 frames of all 13 S3 files. The adjusted values (0, 1, 2, 4) are not
   (763/960). The restoration-type coding and empty-slot value make no difference on these
   inputs. Measurement uses convention set 3.

## What was built

- **Loader** (feature `nvdec`; Python class `GpuLoader`). It loads
  the saved `.cvx.json` index, plans one decode run per anchor, reads only those packets with
  positioned reads, converts length-prefixed samples to Annex B (parameter sets from `avcC` and
  `hvcC`, ISO/IEC 14496-15), and decodes with Spike B. Anchors are decode-exact: the nearest IDR
  picture for H.264, the nearest IRAP sync sample for H.265 (one further back when the target is
  a RASL picture of it), the nearest shown key frame for AV1. Pictures that are not targets are
  never mapped. Targets are converted by Spike B's RGB kernel straight into a tensor that PyTorch
  allocated, then exported through Spike A's DLPack producer. NVDEC sessions are pooled by codec
  and coded size and reused across files.
- **Spike B changes**: an AV1 header path (`src/av1.rs`) written from the
  AV1 specification (OBUs, sequence header, uncompressed frame header, tile groups, reference
  state), its picture parameters (`Decoder::picture_av1`), an external RGB destination, a mode
  that skips mapping unused pictures, session rebinding for pooled H.264 sessions, and the
  current public NVIDIA header (`sdk/cuviddec.h`; provenance and the layout check in
  [evidence/header-provenance.json](evidence/header-provenance.json)). The H.264 and H.265 paths
  are otherwise unchanged; their structure layouts are identical under both headers
  (`abi_tests`).
- **Index changes**: `hvc1`, `hev1` and `av01` sample entries, and
  the codec configuration record stored in the index.
- **Harness** (this directory): `prepare.py` (corpus selection, S4 re-encode, index, packet
  oracle, parse check), `oracle.py` (FFmpeg 7.0.2 NV12 hashes), `fixtures.py` and `bench.py`
  (AV1 bring-up, exactness, warm-up, shuffled trials), `summarize.py`,
  `remote_app.py` and `run.py` (timeouts, detached calls).

## Unsafe surface added

All in spike code, none in the workspace:

| Location | Operation |
|---|---|
| Spike B's `src/nvdec.rs`, `picture_av1` | `cuvidDecodePicture` with AV1 picture parameters; zeroed ABI structures |
| Spike B's `src/nvdec.rs`, `Rgb::convert` | kernel launch writing into a caller-provided device address (the PyTorch tensor) instead of the internal ring; slot bound checked against the registered frame count |
| Spike B's `src/nvdec.rs`, `map_existing` | maps an already decoded surface (AV1 `show_existing_frame`) |
| Spike B's `src/nvdec.rs`, `picture_av1` (`W2_DUMP_PARAMS` only) | diagnostic read of the parameter structure's own bytes for the parser comparison; off by default |
| this spike's `parser_probe.py` | `ctypes` calls into NVIDIA's parser and reads the parameter structure it passes; diagnostic only, never on a measured path |

The loader module itself (`gpu.rs`) is `deny(unsafe_code)` with no exceptions. It reads the
tensor's address through PyTorch's public `data_ptr()` after checking device, dtype, contiguity
and shape; it does not create DLPack capsules. This surface needs its sites listed and a scoped
decision under the `unsafe` policy before any production use.

Diagnostic switches (`W2_TRACE`, `W2_AV1_FORCE`, `W2_SURFACE`, `W2_DECODE_SURFACES`,
`W2_DUMP_PARAMS`) stay in the spike for reproducing the bring-up; none is set in a
measurement run.

## Clean room

Besides the project's own documents, only the AV1 specification, ISO/IEC 14496-12 and -15,
NVIDIA's public header and documentation, and the public documentation of torchcodec, the cloud host and
the ML loaders were used. No multimedia implementation source was read. FFmpeg 7.0.2, libdav1d,
libx265, SVT-AV1 and libaom (through FFmpeg) and torchcodec are black boxes. ML loader parameters
were checked in papers, model cards, configs and frame-index code only
([evidence/loader-sources.md](evidence/loader-sources.md)).

Two disclosures, for review:

- **NVIDIA's parser as a black box.** The AV1 picture-parameter conventions that the public
  header leaves open (above) were confirmed by recording what NVIDIA's own parser passes,
  through its public interface. No parser code was read; only field values were compared.
  The Stage 1 gate used the same parser as a diagnostic for H.265.
- **A search result summary.** While looking for NVIDIA documentation of those conventions,
  one web search returned FFmpeg source pages (`nvdec_av1.c`) and its automatic summary quoted
  two assignment lines. No page was opened, and nothing from it was used: the conventions were
  established by the diagnostics above.

## Reproduction

The spike's code is not part of the published tree. The stages
were: `prepare` (corpus, S4 re-encode, index, packet and parse checks, on a CPU
host), `oracle` (FFmpeg 7.0.2 NV12 hashes of every planned frame, CPU),
`bringup` (AV1 conventions, exactness only, L4), and `measure 0 3` and
`measure 1 3` (machines 0 and 1, convention set 3, L4); `summarize.py` wrote
`evidence/summary.json`. The build used Rust 1.85+ and the build tools of Spike A's
virtual environment (`maturin` 1.8.7, `ziglang` 0.13.0, `libclang` 18.1.1; manylinux 2.28
wheel). The cloud app had no schedules and was stopped after each run; each
function had a hard timeout and `retries=0`.

## Verification

Local runs on 2026-10-14 (no GPU locally):

| Command | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check` | pass (workspace untouched) |
| `cargo fmt --check`, `cargo clippy --release [--features gpu] --all-targets -- -D warnings`, `cargo test --release` in Spike B's directory | pass; 32 tests, including AV1 descriptors, OBU framing, reference selection, the structure-layout check and local AV1 streams |
| the same in Spike A's directory with and without `--features nvdec` | pass; 16 Rust tests, 20 Python tests |
| `cargo-deny --all-features --manifest-path <spike>/Cargo.toml --config deny.toml check` for Spike A and Spike B | advisories, bans, licenses, sources ok (duplicate `syn` and unused-license warnings only) |
| `python3 -m unittest discover -s tests` here | 6 tests pass |

The measured extension is `coreav_spike_a-0.0.1-cp313-abi3-manylinux_2_28_x86_64.whl`, SHA-256
`87c6f1eaaab45679583a7d46017d52357b720d586caa9623e1a2ea527ec25322`, built with
`--features extension-module,nvdec` from commit `fix(spikes): pass the AV1 primary reference as a
surface`; later commits change only Python harness code, formatting and documentation.

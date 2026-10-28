# Thumbnails on CoreAV's own H.264 decoder

Measurement code for step 4 of `docs/design/h264.md`: `coreav thumbnail`
with the native H.264 backend (`crates/coreav-backend-h264`) against the
same command with the sandboxed FFmpeg fallback, and against the `ffmpeg`
command line. Outside the Cargo workspace.

The selection, the time `T`, FFmpeg's time for the chosen keyframe and
the image comparison come from `docs/spikes/thumbnail-bench/`, so this run
chooses frames exactly as the thumbnail measurement did.

## Hypothesis

CoreAV's own H.264 decoder, decoding in the calling process, makes a
thumbnail faster than the sandboxed FFmpeg fallback — which starts a
worker, starts an `ffmpeg`, and copies every decoded frame through a
pipe — and no slower than the `ffmpeg` command line a user would run,
while returning the same image.

## Bars

Committed before the measured run.

| Bar | Set | Threshold |
|---|---|---|
| A1 same image | every held-out H.264 file both builds thumbnail | the native build's PNG **byte-identical** to the fallback build's, in keyframe mode and with `--exact`, on at least **99%** of them; every difference is reported with its file |
| A2 against the fallback | the same files | median per-file time ratio native / fallback at most **1.00**, in keyframe mode (`N1-keyframe`) and with `--exact` (`N2-exact`); a "faster" claim needs the upper bound of the bootstrap 95% confidence interval below 1.00 |
| A3 against FFmpeg | the same files | bars **S1, S2 and S3** of `docs/design/thumbnails.md`, "Speed", with the native backend: median per-file ratio at most **1.00** for the H.264 class |
| A4 coverage | the 212 held-out H.264 files of corpus v1 | the share the native decoder decodes is reported; a file it refuses falls through to the fallback in a build that has both, and is listed with its reason |

Kill: if both rows of A2 are above 1.00 — the native backend is slower
than the sandboxed fallback it was meant to replace — the native backend
does not become the H.264 path for thumbnails, and the result goes to the
maintainer with the numbers. A missed A3 row is reported as missed, here
and in `docs/design/h264.md`; nothing claims it.

## Method

- Set: the held-out H.264 files of corpus v1 (`docs/spikes/thumbnail-bench/`'s
  selection, `holdout: true`), 212 files, measured once. They were never
  used to tune the decoder: the decoder's tuning and held-out sets are
  elementary streams drawn from the same corpus in
  `docs/spikes/h264-n2/sets/`, and no file of this run's set is in
  `docs/spikes/h264-n2/sets/tuning.json`.
- Two builds of the same commit, each with one backend, so that what ran
  is never in doubt:
  `--features native-h264` and `--features ffmpeg-fallback`.
- `speed.py`: one file at a time, downloaded from R2 (SHA-256 checked)
  and deleted after it. Both CoreAV builds and FFmpeg run in the same
  rounds on the same machine: one warm-up of every command, then 5 rounds
  with the commands in rotated order; a command's time is the median of
  its rounds. Sources in the page cache, outputs on a tmpfs, every
  process timed from outside (start-up, sandbox and worker included).
- Untimed, each file's PNG from both builds is hashed, which is bar A1.
- `summarize.py`: the tables below, with bootstrap 95% confidence
  intervals (10,000 resamples).
- `scope.py`: what every file of the set declares in its first sequence
  parameter set, which is what the backend's scope rules read. Ranged
  reads where the configuration record is an `avcC`, the elementary
  stream otherwise; no decoding, no timing.

Reproduce from the repository root, with `S` naming the spike's directory:

    cargo build --release -p coreav-cli --features native-h264 --target-dir target/native
    cargo build --release -p coreav-cli --features ffmpeg-fallback --target-dir target/fallback
    python3 -I "$S/speed.py" \
        --native target/native/release/coreav --fallback target/fallback/release/coreav \
        --out docs/spikes/h264-native-thumbnail/evidence/speed.jsonl
    python3 -I "$S/summarize.py" \
        --speed docs/spikes/h264-native-thumbnail/evidence/speed.jsonl
    # what the set declares, for the scope rules measured against it:
    python3 -I "$S/scope.py" \
        --out docs/spikes/h264-native-thumbnail/evidence/scope.json

## Results

212 held-out H.264 files of corpus v1, measured once on the development
machine (8 cores, 16 GB, Linux x86-64) with nothing else running; the
`ffmpeg` and `ffprobe` of the pinned FFmpeg 7.0.2. 201 files were timed:
of the other 11, ten are streams the decoder does not decode (bar A4) and
one has no frame at the time the selection asks for, in both builds.

Evidence: [`evidence/speed.jsonl`](evidence/speed.jsonl), one object per
file; the tables are `summarize.py` over it.

| Bar | Files | Median ratio | 95% CI | Verdict |
|---|---|---|---|---|
| **A2** `N1-keyframe` native / fallback, keyframe mode | 201 | **0.986** | 0.888 to 1.094 | met on the median; the interval crosses 1.00, so neither is "faster" |
| **A2** `N2-exact` native / fallback, `--exact` | 201 | **5.612** | 4.506 to 5.958 | **missed** |
| **A3** `S1` native / FFmpeg, keyframe mode against `-ss T` | 201 | **0.437** | 0.410 to 0.471 | met, and faster: the interval is below 1.00 |
| **A3** `S2` native / FFmpeg, the same keyframe (`-ss K`) | 201 | **0.611** | 0.586 to 0.643 | met, and faster |
| **A3** `S3` native / FFmpeg, `--exact` | 201 | **4.623** | 4.057 to 6.093 | **missed** |
| `S1` fallback / FFmpeg | 201 | 0.450 | 0.415 to 0.484 | for comparison |
| `S2` fallback / FFmpeg | 201 | 0.673 | 0.608 to 0.756 | for comparison |
| `S3` fallback / FFmpeg | 201 | 0.993 | 0.963 to 1.017 | for comparison |

Share of files where the native backend is faster: 101 of 201 (50.2%)
than the fallback in keyframe mode, 4 of 201 (2.0%) with `--exact`; 199
of 201 (99.0%) than the `ffmpeg` command line in keyframe mode, 191 of
201 (95.0%) on the same keyframe, 19 of 201 (9.5%) with `--exact`.

**A1 same image: met, 201 of 201 in both modes.** The two builds write
byte-identical PNGs for every file. Two files differed in a first run:
their containers declare fewer samples than the bitstream codes (an MP4
whose sample entry is 854 wide for a stream the parameter set crops to
864, and a QuickTime file that trims one column and one row), which
FFmpeg honours and the native backend did not. The decoded samples agree
with FFmpeg frame for frame on both — `scripts/h264_agreement.py` on
their elementary streams, 362 and 50 frames, every frame equal — so this
was the frame's size, not its samples. The backend now honours
`StreamConfig.width` and `StreamConfig.height` where they are inside the
coded picture and within one macroblock of it
(`docs/design/decode.md`, "Selection"), and this table is the run after
that fix.

**A4 coverage: 201 of 211.** The ten the decoder refuses are refused
before a decoder is opened, from the configuration record alone, so in a
build with both backends the sandboxed fallback takes them:

| Reason | Files |
|---|---|
| `profile_idc` 244 (High 4:4:4 Predictive) | 6 |
| 10-bit | 3 |
| not 4:2:0 | 1 |

**What the set contains, for the scope rules measured against it.** The
backend refuses a stream that declares it may code field pictures
(`frame_mbs_only_flag` 0), which was added after this run. Every one of
the 212 files declares `frame_mbs_only_flag` 1 and
`mb_adaptive_frame_field_flag` 0, so the rule moves no file of this set
from timed to refused and the numbers above describe the shipped code.
The sequence parameter sets were read without decoding, by `scope.py`:
a ranged read of each object's head and tail for the 202 whose
configuration record is in an `avcC` (111 at the head, 91 at the tail),
and the ten Matroska files by reading their first sequence parameter set
out of the elementary stream. Evidence:
[`evidence/scope.json`](evidence/scope.json), one object per file.

| `profile_idc` | Files |
|---|---|
| 66 (Baseline, Constrained Baseline) | 23 |
| 77 (Main) | 45 |
| 100 (High) | 134 |
| 122 (High 4:2:2) | 3 |
| 244 (High 4:4:4 Predictive) | 7 |

**The kill criterion did not trigger**: it asked for both rows of A2
above 1.00, and the keyframe row is 0.986.

### What the numbers say

A thumbnail at a keyframe decodes one picture, and there CoreAV's own
decoder is the right path: it is 0.437 of the `ffmpeg` command line the
task names, 0.611 against FFmpeg seeking to the same keyframe, and level
with the sandboxed fallback, which it beats on half the files while
needing no `ffmpeg` binary, no worker process and no frame copied back
through shared memory.

`--exact` decodes every picture from the keyframe to the frame asked
for, and there the single thread is what costs: 4.6 times FFmpeg, which
uses every core, and 5.6 times the fallback, which is FFmpeg in a
sandbox. That is the multithreading gap, not a per-picture one — the
decoder is within 1.5 of FFmpeg single-threaded (`docs/spikes/h264-n2/`). So
`coreav thumbnail` orders the backends by the work it is about to ask
for (`docs/design/decode.md`, "Selection"), and frame and slice
threading is step 7 of `docs/design/h264.md`.

## Spend

R2 reads only (egress is free); no compute is bought. The run is local.

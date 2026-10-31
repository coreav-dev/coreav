# Thumbnails and the decode boundary against FFmpeg

Measurement code, outside the Cargo workspace.
It checks `coreav thumbnail` and the decode boundary under it
(`crates/coreav-decode`, the sandboxed FFmpeg fallback in
`crates/coreav-worker` and `crates/coreav-backend-ffmpeg`) against the
`ffmpeg` command line on corpus v1.

## Hypothesis

CoreAV produces the same thumbnail as FFmpeg for the same frame and size,
with decoded frames that agree bit for bit with FFmpeg's, and it is no
slower than the `ffmpeg -ss T -i in -frames:v 1 -vf scale=...` command a
user would run, with the sandbox and worker start-up counted.

## Bars

Committed before any measured run, in the design notes; not restated here
so they cannot drift:

- frames against FFmpeg's `framemd5`: `docs/design/decode.md`, "Bars", bar 1
  (99% of streams and 99.9% of frames over corpus v1, 100% on fixtures);
- images I1 to I3 and speed S1 to S5: `docs/design/thumbnails.md`, "Bars";
- the hostile suite (no crash, no hang, limits hold): `docs/design/decode.md`,
  "Bars", run by `scripts/cli_hostile_check.py --thumbnail --mutate` in the
  fixtures workflow.

Kill criterion: a bar missed in the final run is reported as missed, here
and in the design note, with its cause; nothing claims it.

## Method

- `common.py`: the selection. Corpus v1 `real` files, classed by the codec
  of the first video stream; encrypted files and codecs off the default
  decode allowlist are counted apart. The oracle functions (the time `T`,
  FFmpeg's frame times, PSNR, reference images, `framemd5`) are shared with
  the fixtures gate in `scripts/thumbnail_oracle.py`.
- `oracle.py`: correctness on every selected file, run locally (`--jobs N`):
  each file is downloaded from R2 (SHA-256 checked), checked and deleted.
  A stream is fed whole to the decode boundary (`tools/decode-oracle`) and
  its frames' MD5s compared with `ffmpeg -noautorotate -noautoscale -f
  framemd5`; images are compared with FFmpeg's of the same frame (on
  FFmpeg's own timeline) and size. The tools run without the R2
  credentials, and the log shows counts only.
- `remote_oracle.py`: the same check on a cloud host, optionally, one container per
  file (2 CPU, 8 GiB), with the pinned static FFmpeg 7.0.2. R2 credentials
  go in as an ephemeral secret from the local environment.
- `speed.py`: speed on the held-out files only, on one local machine, both
  programs timed from outside as processes, sources in the page cache,
  outputs on tmpfs, one warm-up then 5 rounds in rotated order.
- `summarize.py`: the tables below (per-class medians, bootstrap 95%
  confidence intervals with 10,000 resamples, files apart by reason).

Reproduce from the repository root, with `S` naming the spike's directory:

    cargo build --release --features ffmpeg-fallback -p coreav-cli
    cargo build --release -p decode-oracle
    python3 -I "$S/oracle.py" --coreav target/release/coreav \
        --decode-oracle target/release/decode-oracle --out docs/spikes/thumbnail-bench/evidence/oracle.jsonl
    python3 -I "$S/speed.py" --coreav target/release/coreav \
        --out docs/spikes/thumbnail-bench/evidence/speed.jsonl
    python3 -I "$S/summarize.py" \
        --oracle docs/spikes/thumbnail-bench/evidence/oracle.jsonl \
        --speed docs/spikes/thumbnail-bench/evidence/speed.jsonl

## Preliminary runs

Two correctness runs on a cloud host over all selected files came before the final
one, while the code was being fixed; they found the faults listed in the
pull request (the resampling kernel, chroma siting, rotation order, the
file's start time, per-codec colour precedence, H.264 reorder delay, the
`ctts` wrap and ProRes alpha in the oracle). The bars were not changed
after them. They used the development and held-out files alike for
correctness, which the held-out rule allows: it guards speed claims, and no
speed run on held-out files came before the final one. Their outputs are
not kept.

## Results

**What was measured.** Commit `8a4d603`, kept on a branch, since the pull request's history was rebased and squashed afterwards.
- FFmpeg 7.0.2 is the pinned static build (`scripts/install-oracle-ffmpeg.sh`).
- CoreAV is the release `coreav` with `ffmpeg-fallback`, so every class ran on the sandboxed FFmpeg fallback.

**How the merged code differs** (all review fixes made after the run):
- the worker's write path: writes go through a writer thread with a deadline, and the parent tracks which slots it holds;
- the CLI compiles `thumbnail` only with a decoder feature, which does not change the timed command;
- each sandboxed decoder process has a memory budget (8 GiB address space). FFmpeg's decoding threads are fitted to it, only ever lowered. On the 8-CPU measuring host the estimate keeps FFmpeg's own 9 threads up to about 34 megapixels for ProRes 4444, and about 110 megapixels for 8-bit 4:2:0 codecs with reference frames; only larger frames get fewer threads. Its filter and output threads are pinned to one each (`docs/design/decode.md`, "Memory").

The speed numbers are those of the measured commit.

**Correctness.** All 1,058 files ran on a cloud host, one container per file (`remote_oracle.py`), counted as fixed before the run (`docs/design/decode.md`, bar 1).
- Three ProRes files failed in their 8 GiB containers. The cloud host's error carries no reason, but their peaks in the local re-run, 7.6 to 15.4 GiB, exceed that. They count as disagreeing, and their images as failing.
- They were re-run on larger hosts after the run (below): two locally, and the largest in a 64 GiB cloud container with the decode oracle's memory budget raised. All three agree there. The re-runs change no verdict.

**Speed.**
- One local machine (8 cores, 16 GB), held-out files only, measured once.
- The run was resumed once after its shell's time limit; files already in the output were skipped.
- "Faster than FFmpeg" is the share of files whose ratio is below 1.00, reported for information beside the bar.

### Against the bars

| Bar | Result | Verdict |
|---|---|---|
| Decode bar 1, streams (99%) | 99.43% (1052 of 1058) | met |
| Decode bar 1, frames (99.9%) | 99.876% | **missed** |
| I1 and I2 (class medians ≥ 40 dB, 98% of files ≥ 35 dB) | lowest class median 42.0 dB; 99.52% of all files | met |
| I3 (class median delta ≥ 0 dB, 98% of files ≥ 28 dB) | every class ≥ 0.97 dB; 99.24% of all files | met |
| S1 keyframe, `-ss T` | 0.425 (CI 0.394 to 0.452), faster on 97% | met in every class |
| S2 same keyframe, `-ss K` | 0.655 (CI 0.600 to 0.737), faster on 89% | met in every class |
| S3 `--exact`, `-ss T` | 1.018 (CI 0.993 to 1.039), faster on 46% | **missed** overall and for H.264 (1.013), AV1 (1.205) and VP8 (1.275); met for HEVC, VP9 and ProRes |
| S4 PNG | 0.834, faster on 61% | met overall; **missed** for VP8 (1.248) |
| S5 WebP | 0.196, faster on 100% | met in every class |

"98% of files" in I1 to I3 is read over all files, as the bar's text puts it. The per-class shares are in the tables below: ProRes is at 84.0% (I1), 82.6% (I2) and 80.0% (JPEG ≥ 28 dB), because of the three runs above, and AV1 JPEG is at 97.7%.

**Why the frame bar is missed**, by cause:

| Cause | Streams | FFmpeg frames counted unequal |
|---|---:|---:|
| Malformed files CoreAV refuses by design: two truncated HEVC files (`truncated_structure`) and an H.264 file with a second `moov` box (`invalid_structure`) | 3 | 148 |
| Runs whose 8 GiB measuring containers failed (memory, by their local peaks): three ProRes files. Re-run since, all three agree (the largest with a raised budget, below) | 3 | not counted (their runs ended); 336 of 336 equal in the re-runs |
| Frames CoreAV decoded differently from FFmpeg | 0 | 0 of 118,755; 0 of 119,091 with the re-runs |

**The frame bar's verdict: missed.** The counting fixed before the run, which this run keeps (`docs/design/decode.md`, bar 1), counts a refused file as disagreeing in full. Under it, the 148 frames of the three malformed files CoreAV refuses by design are more than the bar's 0.1% allows, so no re-run could lift the bar. The re-runs of the three ProRes files (below) answer the remaining question, whether any of them was decoded wrongly: none was. Their 336 frames agree, and with them every stream CoreAV decoded agrees with FFmpeg on every compared frame (1,055 streams, 119,091 frames). The largest file agrees only with the decode oracle's memory budget raised from 8 to 32 GiB, a test-tool setting. With the default budget, the oracle, which sends 120 packets ahead of a slow consumer, ends with `Limit` (below).

How refusals by design are counted is set, for the next measurement, in `docs/design/decode.md`, bar 1.

**Why S3 is missed.** `--exact` decodes from the keyframe to the frame at `T`. Through the fallback, FFmpeg writes every decoded frame to the worker, and the worker drops the unwanted ones; `ffmpeg -ss T` drops them inside the decoder. The cost grows with the distance to the keyframe, and it shows most in AV1 and VP8, whose FFmpeg decoders are fast.

**Next steps** (developed on the development set, then re-measured on these held-out files, with disclosure):
- not decoding frames that nothing depends on before the target: H.264 `nal_ref_idc` 0, and VP8 and VP9 frames that refresh no reference;
- dav1d for AV1, in the worker;
- a faster PNG path for VP8's large frames.

### Correctness (all `real` files; the result)

#### Frames against FFmpeg framemd5 (decode bar 1: 99% of streams, 99.9% of frames)

| Class | Streams | Agree | Frames | Equal | Not decoded (reason) |
|---|---:|---:|---:|---:|---|
| h264 | 699 | 698 | 77835 | 77715 | {'decode_failed': 1} |
| hevc | 119 | 117 | 13748 | 13720 | {'decode_failed': 2} |
| av1 | 88 | 88 | 10543 | 10543 |  |
| vp9 | 74 | 74 | 8408 | 8408 |  |
| vp8 | 53 | 53 | 6322 | 6322 |  |
| prores | 25 | 22 | 2047 | 2047 | {'run container_error': 3} |
| all | 1058 | 1052 (99.43%) | 118903 | 118755 (99.876%) | |

A stream CoreAV fails to decode, or gets no frame from, counts as disagreeing in full: in Streams and not in Agree, and FFmpeg's first 120 frames of it in Frames and not in Equal. A run that hung, crashed or ran out of memory counts as disagreeing too (its frames unknown).

#### Images against FFmpeg's of the same frame and size (I1, I2: median per class ≥ 40 dB, 98% of files ≥ 35 dB; I3: median per class of ours − FFmpeg's ≥ 0 dB, 98% of files ≥ 28 dB)

| Class | Files | I1 median | I1 ≥ 35 | I2 median | I2 ≥ 35 | I3 median delta | JPEG ≥ 28 | Bytes ours / FFmpeg | Apart |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| h264 | 699 | 43.64 | 100.0% | 43.60 | 100.0% | 1.54 | 99.9% | 1.70 | {'refused': 3} |
| hevc | 119 | 43.19 | 100.0% | 43.25 | 100.0% | 1.64 | 100.0% | 1.68 | {'refused': 2, 'ffmpeg_discards': 1} |
| av1 | 88 | 43.38 | 100.0% | 43.38 | 100.0% | 1.72 | 97.7% | 1.72 |  |
| vp9 | 74 | 42.91 | 100.0% | 42.94 | 100.0% | 1.03 | 100.0% | 1.60 |  |
| vp8 | 53 | 44.00 | 98.1% | 43.71 | 98.1% | 0.97 | 100.0% | 1.70 | {'I2 refused': 1} |
| prores | 25 | 42.00 | 84.0% | 42.00 | 82.6% | 1.21 | 80.0% | 1.33 | {'run container_error': 3, 'I2 refused': 2} |
| all | | 43.57 | 99.52% | 43.52 | 99.52% | | 99.24% | | |

Apart (not on the allowlist, encrypted, or not run because the corpus could not be read): {'encrypted': 21, 'not_allowlisted': 106}


### With the re-runs of the three files

Not the result, and no verdict changes. The three ProRes files whose cloud containers failed, re-run on larger hosts. Their rows replace theirs here, a later file's over an earlier one's (`summarize.py --oracle evidence/oracle.jsonl evidence/oracle-rerun.jsonl evidence/oracle-rerun-2.jsonl`).
- **First re-run** (`evidence/oracle-rerun.jsonl`): all three, locally with `oracle.py`, on the 16 GB host. The decode oracle (test-only) was changed for it: it allows 16 GiB of packets sent ahead of the decoder, since it sends 120 packets at once and 8K ProRes does not fit the per-request default of 1 GiB. Two files agreed. The third, 8809×6314 ProRes 4444 at 12 bits with alpha (`ed60cdd24ba9`), did not finish: the kernel killed FFmpeg under its frame threads, and FFmpeg's own `framemd5` stopped after 9 frames.
- **Second re-run** (`evidence/oracle-rerun-2.jsonl`): that file alone, on a cloud host (`remote_oracle.py --sha ed60cdd24ba9 --cpu 8 --memory-gib 64 --decode-memory-gib 32`), with the pinned FFmpeg 7.0.2.
  - **Code:** `main` at `3548e48` with the tool change of an earlier change, release build with `ffmpeg-fallback`; not the measured commit `8a4d603` (above).
  - **Disclosed test-tool setting:** the decode oracle's memory budget per decoder process is raised from the default 8 GiB to 32 GiB (`--max-memory-gib 32`). With the default it ends with `Limit`, since it sends all 120 packets ahead of a slow consumer (`docs/design/decode.md`, "Memory"). A thumbnail keeps the default budget.
  - **Result:** the stream has 96 frames, and all 96 agree with FFmpeg's `framemd5`. Its images meet I1 to I3, as in the first re-run (I1 51.4 dB, I2 51.4 dB, JPEG 40.6 dB against FFmpeg's 35.0 dB).

#### Frames against FFmpeg framemd5 (decode bar 1: 99% of streams, 99.9% of frames)

| Class | Streams | Agree | Frames | Equal | Not decoded (reason) |
|---|---:|---:|---:|---:|---|
| h264 | 699 | 698 | 77835 | 77715 | {'decode_failed': 1} |
| hevc | 119 | 117 | 13748 | 13720 | {'decode_failed': 2} |
| av1 | 88 | 88 | 10543 | 10543 |  |
| vp9 | 74 | 74 | 8408 | 8408 |  |
| vp8 | 53 | 53 | 6322 | 6322 |  |
| prores | 25 | 25 | 2383 | 2383 |  |
| all | 1058 | 1055 (99.72%) | 119239 | 119091 (99.876%) | |

A stream CoreAV fails to decode, or gets no frame from, counts as disagreeing in full: in Streams and not in Agree, and FFmpeg's first 120 frames of it in Frames and not in Equal. A run that hung, crashed or ran out of memory counts as disagreeing too (its frames unknown).

#### Images against FFmpeg's of the same frame and size (I1, I2: median per class ≥ 40 dB, 98% of files ≥ 35 dB; I3: median per class of ours − FFmpeg's ≥ 0 dB, 98% of files ≥ 28 dB)

| Class | Files | I1 median | I1 ≥ 35 | I2 median | I2 ≥ 35 | I3 median delta | JPEG ≥ 28 | Bytes ours / FFmpeg | Apart |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| h264 | 699 | 43.64 | 100.0% | 43.60 | 100.0% | 1.54 | 99.9% | 1.70 | {'refused': 3} |
| hevc | 119 | 43.19 | 100.0% | 43.25 | 100.0% | 1.64 | 100.0% | 1.68 | {'refused': 2, 'ffmpeg_discards': 1} |
| av1 | 88 | 43.38 | 100.0% | 43.38 | 100.0% | 1.72 | 97.7% | 1.72 |  |
| vp9 | 74 | 42.91 | 100.0% | 42.94 | 100.0% | 1.03 | 100.0% | 1.60 |  |
| vp8 | 53 | 44.00 | 98.1% | 43.71 | 98.1% | 0.97 | 100.0% | 1.70 | {'I2 refused': 1} |
| prores | 25 | 44.28 | 96.0% | 42.03 | 95.7% | 1.57 | 92.0% | 1.33 | {'I2 refused': 2} |
| all | | 43.57 | 99.81% | 43.53 | 99.81% | | 99.52% | | |

Apart (not on the allowlist, encrypted, or not run because the corpus could not be read): {'encrypted': 21, 'not_allowlisted': 106}


### Speed (held-out files; bar: median of per-file ratios ≤ 1.00 overall and per class)

#### S1

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.452 | 0.418 to 0.493 | 98% | 1.880 | ffmpeg | 0 |  |
| hevc | 27 | 0.344 | 0.254 to 0.407 | 100% | 0.827 | ffmpeg | 0 |  |
| av1 | 26 | 0.310 | 0.129 to 0.479 | 85% | 2.419 | ffmpeg | 0 |  |
| vp9 | 28 | 0.275 | 0.180 to 0.351 | 100% | 0.868 | ffmpeg | 0 |  |
| vp8 | 19 | 0.576 | 0.328 to 0.730 | 100% | 0.999 | ffmpeg | 0 |  |
| prores | 6 | 0.492 | 0.457 to 0.544 | 100% | 0.563 | ffmpeg | 0 |  |
| all | 317 | 0.425 | 0.394 to 0.452 | 97% | 2.419 | ffmpeg | 0 |  |

#### S2

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.677 | 0.601 to 0.749 | 93% | 1.901 | ffmpeg | 0 | 0 |
| hevc | 27 | 0.540 | 0.449 to 0.596 | 96% | 1.057 | ffmpeg | 0 | 0 |
| av1 | 26 | 0.914 | 0.700 to 1.110 | 62% | 2.505 | ffmpeg | 0 | 0 |
| vp9 | 28 | 0.544 | 0.510 to 0.622 | 93% | 1.096 | ffmpeg | 0 | 0 |
| vp8 | 19 | 0.865 | 0.801 to 1.113 | 58% | 1.680 | ffmpeg | 0 | 0 |
| prores | 6 | 0.512 | 0.455 to 0.575 | 100% | 0.590 | ffmpeg | 0 | 0 |
| all | 317 | 0.655 | 0.600 to 0.737 | 89% | 2.505 | ffmpeg | 0 | 0 |

#### S3

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 1.013 | 0.984 to 1.031 | 46% | 1.986 | ffmpeg | 0 | 1 |
| hevc | 27 | 0.926 | 0.727 to 1.029 | 59% | 1.257 | ffmpeg | 0 | 1 |
| av1 | 26 | 1.205 | 1.088 to 1.287 | 23% | 2.050 | ffmpeg | 0 | 0 |
| vp9 | 28 | 0.963 | 0.888 to 1.074 | 61% | 1.536 | ffmpeg | 0 | 0 |
| vp8 | 18 | 1.275 | 1.169 to 1.366 | 6% | 2.446 | ffmpeg | 1 | 0 |
| prores | 6 | 0.481 | 0.443 to 0.541 | 100% | 0.562 | ffmpeg | 0 | 0 |
| all | 316 | 1.018 | 0.993 to 1.039 | 46% | 2.446 | ffmpeg | 1 | 2 |

#### S4

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.854 | 0.788 to 0.988 | 58% | 1.977 | ffmpeg | 0 |  |
| hevc | 27 | 0.560 | 0.507 to 0.695 | 89% | 1.863 | ffmpeg | 0 |  |
| av1 | 26 | 0.959 | 0.784 to 1.247 | 54% | 2.443 | ffmpeg | 0 |  |
| vp9 | 28 | 0.726 | 0.577 to 0.960 | 75% | 1.632 | ffmpeg | 0 |  |
| vp8 | 19 | 1.248 | 1.073 to 1.418 | 21% | 2.030 | ffmpeg | 0 |  |
| prores | 6 | 0.519 | 0.472 to 0.621 | 100% | 0.669 | ffmpeg | 0 |  |
| all | 317 | 0.834 | 0.788 to 0.942 | 61% | 2.443 | ffmpeg | 0 |  |

#### S5

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.184 | 0.174 to 0.195 | 100% | 0.765 | ffmpeg | 0 |  |
| hevc | 27 | 0.330 | 0.283 to 0.378 | 100% | 0.761 | ffmpeg | 0 |  |
| av1 | 26 | 0.240 | 0.213 to 0.292 | 100% | 0.677 | ffmpeg | 0 |  |
| vp9 | 28 | 0.235 | 0.188 to 0.316 | 100% | 0.623 | ffmpeg | 0 |  |
| vp8 | 19 | 0.136 | 0.121 to 0.158 | 100% | 0.423 | ffmpeg | 0 |  |
| prores | 6 | 0.482 | 0.364 to 0.537 | 100% | 0.544 | ffmpeg | 0 |  |
| all | 317 | 0.196 | 0.184 to 0.212 | 100% | 0.765 | ffmpeg | 0 |  |

Median times per file (ms): ff_k_jpg 73.9, ff_k_png 79.7, ff_k_webp 227.8, ff_t_jpg 107.4, ours_exact_jpg 102.7, ours_key_jpg 43.1, ours_key_png 65.8, ours_key_webp 41.7

Not timed, by reason: {'not_allowlisted': 33, 'refused': 2, 'encrypted': 5}


## Second speed measurement: `--exact` skipping pictures nothing depends on

This is round 2, after a failed round 1 (S3 missed at 1.018), on the same held-out partition (`docs/design/thumbnails.md`, "Frames nothing depends on").
- A set used to tune after a failed round is no longer held out, so this is a development measurement.
- Its verdicts on the bars stand as round 2 after a fix.
- No claim that CoreAV is faster than FFmpeg is made from it. That claim waits for a fresh held-out set that neither round has measured, with the implementation frozen first: the `v2-heldout` top-up being collected under an earlier change.
- The first round's results above stand.

**What changed between the runs:** `--exact` stopped decoding H.264 and HEVC pictures before the target that nothing depends on. The change was tuned on the development set only. Same method and host as the first run.
- **Measured commit:** `d2e57f5`, kept as branch `bench/thumbnail-skip-measured`, with `evidence/speed-2.jsonl`.
- **The rule changed after the run, on review:**
  - HEVC's highest temporal layer is read from the packets decoded, from their units' headers, instead of `hvcC`;
  - SEI that persists past its picture keeps a unit;
  - the packet decoding starts from is never skipped.
- **Effect of the change:** it can skip fewer packets (persistent SEI, the start) or more (HEVC, where the packets decoded never reach the highest temporal layer, or `hvcC` gives none). For example, one development HEVC file goes from 49 of 49 packets decoded to 25 of 49.
  - The round 2 numbers above were measured on the earlier rule.
  - On the shipping rule, the 27 held-out HEVC files were timed again (`evidence/speed-2-hevc.jsonl`): S3 0.748, 85% of files below 1.00, S1 0.276. One file is flagged as possibly a different frame (same-frame PSNR under 30 dB).
  - This is a third timing of a partition already used for development, so it backs no claim. H.264 has no rule change that skips more, and was not timed again.
- **Development set** (80 H.264 and HEVC files, `--limit 80`, `evidence/speed-dev-2.jsonl`, on the measured commit's rule): S3 0.770 overall, 85% of files below 1.00.
- **Same image:** byte-identical with skipping on and off. On the tightened rule:
  - corpus v1's development set, at `T` (one time per file), on the code of `5e9ad64` (whose first pass read whole packets for the same TemporalIds): 575 of 578 H.264 and HEVC files identical, and 3 refused alike (two malformed, one out of range). 36% fewer packets decoded (`evidence/skip-dev-2.jsonl`);
  - the fixtures at 8 times each and their Matroska copies: 112 identical, 4 refused alike (`evidence/skip-fixtures.json`);
  - 15 adversarial files from the review at 20 times each: all identical. These cover B-pyramid, open GOP, MBAFF, in-band parameter sets, slices, several temporal layers, and an `hvcC` that understates them. They are review scratch files, not committed; `crates/coreav-thumbnail` has a `decode()`-level test of the understated `hvcC`.
  
  On the first rule, 575 of 578 development files were identical and 3 were refused either way (`evidence/skip-dev.jsonl`).

### S3 against its bar, second run

| Class | Median (95% CI) | Faster than FFmpeg | Bar | First run |
|---|---|---|---|---|
| all | 0.854 (0.833 to 0.888) | 72% | met (round 2) | 1.018, 46% |
| H.264 | 0.830 (0.803 to 0.847) | 84% | met | 1.013, 46% |
| HEVC | 0.751 (0.711 to 0.820) | 100% | met | 0.926, 59% |
| ProRes | 0.473 | 100% | met | 0.481, 100% |
| AV1 | 1.186 | 19% | **missed** (not skipped; dav1d is next) | 1.205, 23% |
| VP8 | 1.228 | 0% | **missed** (not skipped) | 1.275, 6% |
| VP9 | 1.010 (0.952 to 1.175) | 46% | **missed** in this run (not skipped; same code as round 1, which had 0.963) | 0.963, 61% |

**Noise:** per-class medians moved by about ±5% between the rounds on identical code: VP9 went from 0.963 to 1.010, and S1–S5 changed nothing else for VP9. A per-class verdict within about 5% of 1.00 is therefore a close call, not a clear result either way.

S1, S2, S4 and S5 do not use the change. In the second run they are 0.416, 0.636, 0.833 and 0.199 overall, and they meet or miss their bars as in the first run (S4 misses for VP8).


#### S1

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.443 | 0.411 to 0.482 | 98% | 1.969 | ffmpeg | 0 |  |
| hevc | 27 | 0.305 | 0.235 to 0.382 | 100% | 0.705 | ffmpeg | 0 |  |
| av1 | 26 | 0.301 | 0.116 to 0.484 | 85% | 2.107 | ffmpeg | 0 |  |
| vp9 | 28 | 0.263 | 0.182 to 0.348 | 100% | 0.912 | ffmpeg | 0 |  |
| vp8 | 19 | 0.494 | 0.413 to 0.620 | 100% | 0.972 | ffmpeg | 0 |  |
| prores | 6 | 0.477 | 0.469 to 0.555 | 100% | 0.566 | ffmpeg | 0 |  |
| all | 317 | 0.416 | 0.386 to 0.446 | 97% | 2.107 | ffmpeg | 0 |  |

#### S2

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.631 | 0.582 to 0.695 | 94% | 1.941 | ffmpeg | 0 | 0 |
| hevc | 27 | 0.444 | 0.425 to 0.591 | 96% | 1.141 | ffmpeg | 0 | 0 |
| av1 | 26 | 0.887 | 0.642 to 1.035 | 62% | 2.101 | ffmpeg | 0 | 0 |
| vp9 | 28 | 0.532 | 0.488 to 0.621 | 93% | 1.184 | ffmpeg | 0 | 0 |
| vp8 | 19 | 0.853 | 0.757 to 1.023 | 68% | 1.193 | ffmpeg | 0 | 0 |
| prores | 6 | 0.479 | 0.463 to 0.570 | 100% | 0.574 | ffmpeg | 0 | 0 |
| all | 317 | 0.636 | 0.592 to 0.686 | 90% | 2.101 | ffmpeg | 0 | 0 |

#### S3

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.830 | 0.803 to 0.847 | 84% | 2.017 | ffmpeg | 0 | 1 |
| hevc | 27 | 0.751 | 0.711 to 0.820 | 100% | 0.987 | ffmpeg | 0 | 1 |
| av1 | 26 | 1.186 | 1.113 to 1.305 | 19% | 2.270 | ffmpeg | 0 | 0 |
| vp9 | 28 | 1.010 | 0.952 to 1.175 | 46% | 4.036 | ffmpeg | 0 | 0 |
| vp8 | 18 | 1.228 | 1.167 to 1.301 | 0% | 2.323 | ffmpeg | 1 | 0 |
| prores | 6 | 0.473 | 0.466 to 0.555 | 100% | 0.568 | ffmpeg | 0 | 0 |
| all | 316 | 0.854 | 0.833 to 0.888 | 72% | 4.036 | ffmpeg | 1 | 2 |

#### S4

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.841 | 0.789 to 0.918 | 63% | 2.049 | ffmpeg | 0 |  |
| hevc | 27 | 0.528 | 0.494 to 0.649 | 93% | 1.555 | ffmpeg | 0 |  |
| av1 | 26 | 0.933 | 0.841 to 1.241 | 54% | 2.512 | ffmpeg | 0 |  |
| vp9 | 28 | 0.734 | 0.589 to 0.866 | 79% | 1.684 | ffmpeg | 0 |  |
| vp8 | 19 | 1.260 | 1.094 to 1.515 | 26% | 1.832 | ffmpeg | 0 |  |
| prores | 6 | 0.537 | 0.494 to 0.619 | 100% | 0.640 | ffmpeg | 0 |  |
| all | 317 | 0.833 | 0.779 to 0.891 | 65% | 2.512 | ffmpeg | 0 |  |

#### S5

| Class | Files | Median | 95% CI | Faster than FFmpeg | Max | Backend | Left out | Other frame |
|---|---:|---:|---|---:|---:|---|---:|---:|
| h264 | 211 | 0.187 | 0.174 to 0.199 | 100% | 0.689 | ffmpeg | 0 |  |
| hevc | 27 | 0.348 | 0.296 to 0.417 | 100% | 0.726 | ffmpeg | 0 |  |
| av1 | 26 | 0.268 | 0.208 to 0.337 | 100% | 0.670 | ffmpeg | 0 |  |
| vp9 | 28 | 0.255 | 0.201 to 0.337 | 100% | 0.628 | ffmpeg | 0 |  |
| vp8 | 19 | 0.146 | 0.113 to 0.164 | 100% | 0.437 | ffmpeg | 0 |  |
| prores | 6 | 0.483 | 0.407 to 0.563 | 100% | 0.575 | ffmpeg | 0 |  |
| all | 317 | 0.199 | 0.190 to 0.214 | 100% | 0.726 | ffmpeg | 0 |  |

Median times per file (ms): ff_k_jpg 59.6, ff_k_png 66.2, ff_k_webp 173.6, ff_t_jpg 92.8, ours_exact_jpg 75.7, ours_key_jpg 35.5, ours_key_png 56.1, ours_key_webp 33.2

Not timed, by reason: {'not_allowlisted': 33, 'refused': 2, 'encrypted': 5}


## AV1 through dav1d

`coreav thumbnail` built with `dav1d` decodes AV1 with dav1d in the sandboxed worker instead of FFmpeg's libdav1d in a second process (`docs/design/decode.md`, "dav1d"). The method for these runs was committed before them (`docs/design/decode.md`, bar 5). Same host and scripts as above. Evidence is in `evidence/*dav1d*`.

**Correctness** (bar 1, every corpus v1 `real` AV1 file, dav1d backend, `evidence/oracle-dav1d.jsonl`):
- 88 of 88 streams agree with FFmpeg's libdav1d, 10,543 of 10,543 frames;
- every thumbnail is at least 40.9 dB (I1) and 41.1 dB (I2) against FFmpeg's;
- I3 (JPEG): the median of CoreAV's PSNR minus FFmpeg's JPEG's is +1.72 dB. 86 of 88 files (97.7%) are at least 28 dB, the same two files and share as through the fallback; the bar's 98% is read over all files, as above.

**First run, then a fix.** The first run, on `dd638cd` (`evidence/*-first.jsonl`), missed S3: 1.251 on the development set, 8% of files below 1.00, and 1.250 on the held-out files. The cause was in the backend. It asked dav1d for pictures until dav1d had none, once per packet, and a second request without new data makes dav1d wait for its oldest frame, which empties its frame pipeline. `a4f48bf` asks once per packet and sets a frame delay of 8 (the design note has the times). The runs below are on `a4f48bf`. The fix was found on development files. The measured commits (`2b441ea`, the method; `dd638cd`; `a4f48bf`) are kept as branch `bench/dav1d-measured`.

**Speed, development set** (62 AV1 files, `evidence/speed-dav1d-dev.jsonl`):

| Bar | Median (95% CI) | Files below 1.00 | Bar met | First run |
|---|---|---|---|---|
| S1 | 0.367 (0.240 to 0.567) | 74% | yes | 0.358 |
| S2 | 0.842 (0.770 to 0.910) | 69% | yes | 0.840 |
| S3 | 0.927 (0.843 to 1.009) | 60% | yes | 1.251 |
| S4 | 1.082 (0.909 to 1.256) | 44% | **no** | 1.156 |
| S5 | 0.238 (0.193 to 0.327) | 100% | yes | 0.247 |

**Speed, held-out AV1 files** (26, `evidence/speed-dav1d-holdout.jsonl`). Both earlier rounds timed these files through the fallback, so this is a development measurement. Its verdicts stand, and it backs no claim that CoreAV is faster than FFmpeg. That claim waits for a fresh held-out draw.

| Bar | Median (95% CI) | Files below 1.00 | Bar met | Fallback, round 2 |
|---|---|---|---|---|
| S1 | 0.270 (0.105 to 0.382) | 85% | yes | 0.301 |
| S2 | 0.781 (0.590 to 0.879) | 77% | yes | 0.887 |
| S3 | 0.934 (0.778 to 1.030) | 62% | yes | 1.186 |
| S4 | 0.869 (0.725 to 1.150) | 58% | yes | 0.933 |
| S5 | 0.256 (0.188 to 0.282) | 100% | yes | 0.268 |

Both S3 intervals reach above 1.00, and run-to-run noise is about ±5%, so S3 for AV1 is met but close.

S4 (PNG) misses on the development set. The cause is the PNG writer, not decoding: S2, the same frame as JPEG, is 0.842. On small images, CoreAV's PNG takes up to about 25 ms longer than its JPEG, most of it in the zlib stream at level 6. FFmpeg's PNG of the same frame is faster and larger. That is a change to the PNG writer, outside this one.

**Worker overhead** (bar 5: under 15%; development set, keyframe mode, `evidence/overhead-dav1d.jsonl`): median 14.0% on `a4f48bf`, and 10.3% in the first run. Met. Most of these thumbnails take 20 to 40 ms, and the worker adds 2 to 3 ms to each (starting the worker process). The two runs differ by run-to-run noise at that scale; the per-file times barely moved.

## PNG at deflate level 3

The PNG writer's deflate level went from 6 to 3 (`docs/design/thumbnails.md`, "Formats"; the plan was committed before these runs). PNG is lossless, so the images are unchanged; only time and size change. Each run is a development measurement on the development set's AV1 and VP8 files, the classes where S4 had missed. The held-out partition was not timed again.

| Class (backend) | Files | S4 at level 6 | S4 at level 3 | Files below 1.00 |
|---|---:|---|---|---|
| VP8 (fallback) | 34 | 1.006 (0.933 to 1.099) | 0.899 (0.852 to 0.950) | 47% → 76% |
| AV1 (fallback) | 62 | 1.192 (0.977 to 1.415) | 1.002 (0.894 to 1.123) | 39% → 50% |
| AV1 (dav1d) | 62 | 1.082 (0.909 to 1.256) | 0.895 (0.837 to 0.991) | 44% → 65% |

- With level 3, S4 is met for VP8 and for AV1 through dav1d, the build that decodes AV1 since an earlier change.
- AV1 through the fallback misses by 0.002 (1.002), within the ±5% run-to-run noise. S2 (the same frame as JPEG, unchanged code) moved by 0.055 between the two runs.
- CoreAV's PNG took a median 5.4 ms less per file in the paired fallback runs. The dav1d pair is not paired the same way, since its level 6 run is from an earlier change's session.
- By FFmpeg's own time for S4 (AV1 through dav1d):

  | FFmpeg's time | Level 6 | Level 3 |
  |---|---|---|
  | under 50 ms | 1.432, 14% faster | 1.162, 39% faster |
  | 50 to 300 ms | 0.793, 81% | 0.823, 100% |
  | over 300 ms | 0.630, 100% | 0.615, 100% |

  The short thumbnails still lose on the fixed cost that `docs/design/decode.md` (bar 5) lists.
- Evidence (one JSON line per file):
  - `evidence/speed-png-base-dev.jsonl` and `evidence/speed-png-dev.jsonl`: `5572eaa` and the change on it (`fec9b1c`, kept as branch `bench/png-level-measured`), fallback builds;
  - `evidence/speed-dav1d-dev.jsonl` and `evidence/speed-png-dav1d-dev.jsonl`: dav1d builds, before and on `80f37f3`.

## Planning from a window

`coreav thumbnail` on MP4 and MOV files plans from the packets around the time, found in the sample tables, instead of listing every packet (`docs/design/seek.md`; the bars were committed before these runs in `2d18917`, measured on its code; both are kept as branch `bench/seek-measured`). All of this is development measurement on corpus v1's development set, with no "faster than FFmpeg" claim. A fresh held-out draw confirms this change and the conversion change together.

**Same choice** (bar 1, `evidence/seek-dev.jsonl`):
- all 611 development MP4 and MOV video files, at 16 times plus the edges each, in both modes;
- 26,880 choices, all identical to planning from every packet (the first and target packets, the frame's time, the packet count, the first keyframe);
- 21,222 of them (79%) were made from a window and kept; the rest fell back to the full listing;
- four files first failed for lack of disk space on the measuring host and were checked again.

The fixtures give 10,640 choices, all identical (in CI).

**Instructions** (bar 4): on the 24-minute file profiled in `docs/design/decode.md` (bar 5), keyframe thumbnail in-process:
- planning: 40.9M → 1.0M instructions;
- the whole thumbnail: 104.0M → 59.4M;
- the image is byte-identical.

**Speed** (bar 3, S1 and S3 on the 608 development MP4 and MOV files timed; 3 refused alike; `evidence/speed-seek-before.jsonl` on `79b0d47`, `evidence/speed-seek-after.jsonl` on this change; builds with `ffmpeg-fallback` and `dav1d`):

| Bar | Class | Files | Before | After | Files below 1.00 |
|---|---|---:|---|---|---|
| S1 | all | 608 | 0.441 (0.423 to 0.466) | 0.419 (0.396 to 0.441) | 92% → 97% |
| S1 | H.264 | 464 | 0.465 | 0.437 | 94% → 97% |
| S1 | HEVC | 85 | 0.260 | 0.259 | 99% → 100% |
| S1 | AV1 | 40 | 0.648 | 0.536 | 62% → 98% |
| S1 | ProRes | 19 | 0.486 | 0.506 | 89% → 89% |
| S3 | all | 606 | 0.807 (0.793 to 0.821) | 0.780 (0.764 to 0.794) | 82% → 89% |
| S3 | H.264 | 464 | 0.815 | 0.793 | 82% → 87% |
| S3 | HEVC | 85 | 0.745 | 0.739 | 93% → 95% |
| S3 | AV1 | 40 | 1.043 | 0.780 | 45% → 90% |
| S3 | ProRes | 17 | 0.494 | 0.494 | 100% → 100% |

By FFmpeg's own time, as the mean of the two runs so that each bucket holds the same files before and after:

| Bar | FFmpeg's time | Files | Before | After |
|---|---|---:|---|---|
| S1 | under 50 ms | 165 | 0.746, 79% faster | 0.683, 92% |
| S1 | 50 to 300 ms | 323 | 0.406, 97% | 0.377, 99% |
| S1 | over 300 ms | 120 | 0.263, 98% | 0.257, 98% |
| S3 | under 50 ms | 165 | 0.931, 65% | 0.882, 81% |
| S3 | 50 to 300 ms | 323 | 0.791, 85% | 0.769, 90% |
| S3 | over 300 ms | 118 | 0.655, 97% | 0.653, 96% |

- CoreAV's own time fell by a median 4.6 ms (S1) and 6.4 ms (S3) per file. At the 90th percentile it fell by 35 ms and 70 ms: the long files, whose sample tables were the longest to list.
- The S1 and S3 bars are met overall and per class, before and after.
- The two runs are minutes apart on the same host, and per-class medians move about ±5% from run to run, so a change within that (HEVC, ProRes) is not one.

## Faster rendering and JPEG

The RGB conversion chooses its matrix once per row, and the JPEG encoder uses the AAN DCT, a single write per Huffman code, and a per-pixel colour loop (`docs/design/thumbnails.md`, "Faster rendering and JPEG"; the bars were committed before these runs in `8da5507`, measured on its code; kept as branch `bench/render-measured`). This is a development measurement on corpus v1's development set, with no "faster than FFmpeg" claim. Corpus v2-heldout draw 4 confirms this change and `docs/design/seek.md`'s together.

**Bar 1, same images:** all 540 PNG thumbnails of the fixtures are byte-identical to before (both modes, two times, display size and 320 wide). 196 runs are refused alike.

**Bar 2, JPEG against FFmpeg** (I3, 735 development files with images; `evidence/images-render-before.jsonl`, `evidence/images-render-after.jsonl`):

| | Before | After |
|---|---|---|
| Files at least 28 dB | 732 of 735 (99.59%) | 732 of 735 (99.59%) |
| Median of ours − FFmpeg's, per class | AV1 1.586, H.264 1.515, HEVC 1.669, ProRes 1.670, VP8 1.122, VP9 0.933 | AV1 1.586, H.264 1.516, HEVC 1.669, ProRes 1.670, VP8 1.122, VP9 0.921 |

Met before and after. I1 is unchanged: median 43.56 dB, every file at least 35 dB.

**Bar 3, speed** (S1 and S3 on the 608 development MP4 and MOV files timed, 3 refused alike; before is `main` at `0522c63`, with planning from a window; `evidence/speed-render-before.jsonl`, `evidence/speed-render-after.jsonl`):

| Bar | Class | Before | After | Files below 1.00 |
|---|---|---|---|---|
| S1 | all | 0.418 (0.403 to 0.441) | 0.413 (0.389 to 0.428) | 98% → 98% |
| S1 | H.264 | 0.437 | 0.426 | 98% → 98% |
| S1 | AV1 | 0.597 | 0.546 | 100% → 100% |
| S3 | all | 0.772 (0.754 to 0.786) | 0.760 (0.745 to 0.773) | 90% → 92% |
| S3 | H.264 | 0.784 | 0.769 | 88% → 90% |
| S3 | HEVC | 0.727 | 0.721 | 94% → 96% |
| S3 | AV1 | 0.829 | 0.797 | 90% → 92% |
| S3 | ProRes | 0.477 | 0.498 | 100% → 100% |

By FFmpeg's own time (the mean of the two runs, the same files before and after):

| Bar | FFmpeg's time | Files | Before | After |
|---|---|---:|---|---|
| S1 | under 50 ms | 194 | 0.656, 95% faster | 0.620, 96% |
| S1 | 50 to 300 ms | 302 | 0.356, 100% | 0.345, 100% |
| S1 | over 300 ms | 112 | 0.261, 98% | 0.259, 98% |
| S3 | under 50 ms | 194 | 0.870, 84% | 0.834, 87% |
| S3 | 50 to 300 ms | 302 | 0.743, 91% | 0.745, 93% |
| S3 | over 300 ms | 110 | 0.651, 97% | 0.639, 97% |

- CoreAV's own time fell by a median 2.1 ms (S1) and 3.0 ms (S3) per file.
- ProRes and HEVC moved within run-to-run noise (about ±5%).

**Bar 4, instructions** (the 24-minute file of `docs/design/seek.md`, keyframe thumbnail in-process):
- whole thumbnail: 59.4M → 48.8M;
- conversion and resize: 25.3M → 21.2M;
- JPEG encoder: 18.5M → 11.9M.

Resampling (13M of it) is unchanged.

## Joint confirmation on draw 4

The held-out confirmation of planning from a window, faster rendering and JPEG through `jpeg-encoder` 0.6.1 at quality 91, on corpus v2-heldout draw 4. The bars and the method are in `docs/design/thumbnails.md`, "Joint confirmation on held-out draw 4", committed before the draw was opened (`f7b0d03` on an earlier change's branch, merged as `090b5c1`).
- **Code:** `main` at `0f1c3c4`, `--features ffmpeg-fallback,dav1d`, release build.
- **Inputs:** 52 held-out files: H.264 31, HEVC 7, VP8 7, VP9 4, AV1 3. One H.264 file is refused; 51 are timed.
- **Run:** one run, alone on the host. No file was run again.
- **Evidence:** `evidence/speed-draw4.jsonl`, `evidence/images-draw4.jsonl`, `evidence/seek-draw4.jsonl`.
- **The draw is now spent**.

**How the refused file counts.** The bars say no file is left out once the run starts, except for a failure the host causes. They do not say how a file CoreAV refuses counts in a ratio, and the runner leaves it out of the medians.
- The verdict below counts the refused file as slower than FFmpeg on every bar, the stricter reading.
- The count as run, with it left out, is given beside.
- Neither rule was chosen after seeing a ratio: the refusal came before any summary was computed, and both counts meet every bar.

**Speed** (median per-file ratio against `ffmpeg`, 95% CI, share of files below 1.00). The bars are judged overall and for H.264, the only class with 10 files or more; the other classes are reported, not judged.

| Bar | All (52, refused file counted as slower) | H.264 (31, likewise) | As run (51 / 30, refused file left out) | Bar | Faster than FFmpeg (CI upper bound below 1.00) |
|---|---|---|---|---|---|
| S1, keyframe JPEG against `-ss T` | 0.399 (0.314 to 0.444), 98% | 0.342 (0.310 to 0.404), 97% | 0.397, 100% / 0.329, 100% | met | yes, overall and H.264 |
| S2, keyframe JPEG against `-ss K` | 0.587 (0.497 to 0.617), 96% | 0.499 (0.414 to 0.592), 97% | 0.587, 98% / 0.497, 100% | met | yes, overall and H.264 |
| S3, `--exact` JPEG against `-ss T` | 0.740 (0.665 to 0.846), 79% | 0.664 (0.588 to 0.714), 87% | 0.714, 80% / 0.653, 90% | met | yes, overall and H.264 |
| S4, PNG | 0.668 (0.572 to 0.739), 92% | 0.579 (0.500 to 0.675), 94% | 0.661, 94% / 0.560, 97% | met | yes, overall and H.264 |
| S5, lossless WebP | 0.184 (0.170 to 0.208), 98% | 0.193 (0.168 to 0.218), 97% | 0.182, 100% / 0.187, 100% | met | yes, overall and H.264 |

S3's share faster is 79% under the stricter count, against an aim of 80% (an aim, not a bar).

Reported for the smaller classes (S1 / S3 medians, share faster on S3):

| Class | Files | S1 | S3 | Faster on S3 |
|---|---:|---|---|---:|
| HEVC | 7 | 0.629 | 0.798 | 100% |
| AV1 (dav1d) | 3 | 0.777 | 0.766 | 100% |
| VP9 | 4 | 0.370 | 0.963 | 50% |
| VP8 | 7 | 0.403 | **1.411** | 29% |

VP8's `--exact` is slower than FFmpeg's on these files, as it was on corpus v1. Its pictures are not skipped ("Frames nothing depends on"), and every decoded frame goes through the fallback's pipe.

By FFmpeg's own time for each bar's command (the count as run, 51 files):

| Bar | Under 50 ms | 50 to 300 ms | Over 300 ms |
|---|---|---|---|
| S1 | 12 files, 0.590, 100% faster | 36, 0.329, 100% | 3, 0.200, 100% |
| S2 | 24, 0.708, 96% | 26, 0.424, 100% | 1, 0.323, 100% |
| S3 | 12, 0.873, 83% | 36, 0.677, 81% | 3, 0.584, 67% |
| S4 | 19, 0.828, 84% | 31, 0.525, 100% | 1, 0.331, 100% |
| S5 | none | 46, 0.184, 100% | 5, 0.151, 100% |

**Images** (I1 to I3, the 51 files with a thumbnail): all met.

| Class | I1 median, files ≥ 35 dB | I2 median, files ≥ 35 dB | I3: ours − FFmpeg's (median), files ≥ 28 dB |
|---|---|---|---|
| all | 43.35 dB, 51 of 51 | 43.27 dB, 51 of 51 | +1.42 dB, 51 of 51 |
| H.264 | 42.95 dB, 30 of 30 | 42.93 dB, 30 of 30 | +1.46 dB, 30 of 30 |

The other classes are each at least 42.7 dB (I1 and I2), and their I3 medians are positive (AV1 the smallest, +0.19 dB, 3 files). Counting the refused file as failing, every "98% of files" share is 51 of 52 (98.1%).

**Same choice** (`docs/design/seek.md`, bar 1): all 39 MP4 and MOV files, at 16 times plus the edges, both modes. All 1,716 choices are identical to planning from every packet, 1,243 of them made from a window. Met.

**Refused:** one H.264 Matroska file whose first pictures reference a PPS the decoder has not been given. `ffmpeg -ss T` makes an image of it (skipping the undecodable pictures); CoreAV refuses with `decode`. That is an earlier change, a correctness gap, not a speed result.

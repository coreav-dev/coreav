# AV1 thumbnails: the fixed cost of short decodes

Status: measured. The bars and the method were committed before the run (`4881c37`).

`docs/design/decode.md` (bar 5, "Where the AV1 gap is") lists AV1 thumbnails that FFmpeg makes in under 50 ms at 1.609 of FFmpeg's time for `--exact` (S3), and reads it as a fixed cost of about 8 ms per thumbnail. This page measures that again on current `main` and splits the time of a short AV1 thumbnail into its parts, so the largest part can be cut or shown to be gone.

The 1.609 was measured before planning from a window of the packets (`docs/design/seek.md`) and the faster rendering and JPEG (`docs/design/thumbnails.md`). Those runs timed only the 40 MP4 AV1 files of the development set, where the class under 50 ms went from 1.575 to 0.764 (`docs/spikes/thumbnail-bench/evidence/speed-seek-before.jsonl`, `speed-seek-after.jsonl`). The 22 WebM development files and the 26 held-out AV1 files were not timed on the current code.

## What is measured

- **Code:** `main` at `55d126b` (55d126b542b65dd2676322731f3ab0ed601b5955). No CoreAV code changes before the run.
- **Builds:** `cargo build --release -p coreav-cli --features ffmpeg-fallback,dav1d`, and `decode-oracle` with `--features dav1d` (`tools/decode-oracle`), linked against the host's libdav1d (Debian's package; its version is recorded with the host).
- **FFmpeg:** the pinned static binary, 7.0.2 (`scripts/install-oracle-ffmpeg.sh`), which decodes AV1 with its libdav1d.
- **Host:** one Compute Engine `c4-standard-16` with one thread a core (8 cores), not preemptible, by `scripts/gcp_vm_job.py`. Its CPU and kernel are recorded. The run is alone on the host. Processes are not pinned to cores: dav1d and FFmpeg each use the cores they choose, as for a user.
- **Inputs:** every corpus v1 `real` file whose first video stream is AV1, the selection of the thumbnail measurements: 62 development files (40 MP4, 22 WebM) and 26 held-out files. The held-out files were timed in earlier rounds, so this is a development measurement on both sets. It backs no claim that CoreAV is faster than FFmpeg.

## Method

1. **The thumbnail bars**, S1 to S5 of `docs/design/thumbnails.md` ("Speed"), by the thumbnail spike's speed runner (`speed.py --classes av1`, `--set dev` and `--set holdout`), unchanged: one warm-up, 5 rounds per command in rotated order, the median per file, ratios CoreAV over FFmpeg. A file's time class is FFmpeg's own time for S3's command (`ffmpeg -ss T`): under 50 ms, 50 to 300 ms, over 300 ms.
2. **The arms** ([`av1-thumbnail-fixed-cost/arms.py`](av1-thumbnail-fixed-cost/arms.py)), on the same files after the bars, the same rounds and rotation:
   - `key` and `exact`: S1's and S3's CoreAV commands, as the bars run them;
   - `exact_aa`: S3's command again, a second name for the same command (the host's A/A check);
   - `key_t1`, `key_t2`, `key_t4`, `exact_t1`, `exact_t2`, `exact_t4`: the same with `--threads 1`, `2` and `4` (dav1d's threads; the default lets dav1d choose, one per core);
   - `worker` and `in_process`: `decode-oracle thumbnail --keyframe --backend dav1d`, in the sandboxed worker and in-process (decode bar 5's method);
   - `ff_t`: S3's FFmpeg command.

Medians over files of per-file ratios, with a bootstrap 95% confidence interval (10,000 resamples, seed 207), as in the thumbnail bars.

## Bars

1. **Host.** The host has 8 cores with one thread each, and the median over all files of `exact_aa` over `exact` is between 0.97 and 1.03. Otherwise the run is void (a host fault) and is run again once.
2. **The fixed cost.** Over all 88 files, S3's median ratio is at most 1.00 in each time class of at least 10 files, the class under 50 ms included. S1's is reported beside it.
3. **The thumbnail bars** for AV1, S1 to S5, median at most 1.00, on the development files and on the held-out files, each with the share of files faster.
4. **Worker overhead** (decode bar 5): the median over the development files of `worker` over `in_process`, minus 1, is under 0.15.

Reported, not judged:
- for the files under 50 ms, the median difference in milliseconds of S3 (CoreAV minus FFmpeg), of `worker` minus `in_process`, and of each threads arm minus the default;
- per time class, the median of each threads arm over the default.

**What follows from the result.**
- If bar 2 is met, the fixed cost of the design note is gone on current `main`, and this change updates the design note and changes no code.
- A threads setting whose median over the default is below 0.95 for the files under 50 ms, and at most 1.02 for the files over 300 ms, is proposed as a separate change, with its own bars and a fresh held-out draw; it is not adopted on this run.
- If bar 2 misses, the arms say which part to cut, and that change gets its own bars here before it is measured.

**Rules of the run.** One run. A file the host fails to finish (shown by its logs) is run again once with the same build. No other file is run again. Results are reported as they come out.

## Results

One run, on `main` at `55d126b`, on a Compute Engine `c4-standard-16` with one thread a core (Intel Xeon Platinum 8581C, 8 cores), Debian 13 with libdav1d 1.5.1, FFmpeg 7.0.2. All 88 files ran; no command failed and no file was run again. Every thumbnail was decoded by dav1d, and CoreAV's frame matched FFmpeg's (at least 40.9 dB for S2's frame and 41.2 dB for S3's). Evidence: [`av1-thumbnail-fixed-cost/`](av1-thumbnail-fixed-cost/) (`host.txt`, `job.sh`, `job.log`, `speed-dev.jsonl`, `speed-holdout.jsonl`, `arms.jsonl`); the tables come from `summarize.py`.

| Bar | Result |
|---|---|
| 1. Host | **met**: 8 cores, one thread each; A/A 1.001 (1.000 to 1.003) |
| 2. The fixed cost | **met** in every time class (below) |
| 3. Thumbnail bars | **met**: S1 to S5 on both sets |
| 4. Worker overhead | **not met**: median 0.156 on the development files, against a bar of under 0.15 |

### Bar 2: by FFmpeg's time for S3's command

All 88 files, medians of per-file ratios (95% CI), and the share of files faster:

| FFmpeg's time | Files | S3 | S1 | S3, CoreAV minus FFmpeg (median) |
|---|---:|---|---|---:|
| under 50 ms | 37 | 0.673 (0.631 to 0.719), 97% | 0.541 (0.437 to 0.616), 100% | −5.3 ms |
| 50 to 300 ms | 36 | 0.827 (0.736 to 0.929), 81% | 0.212 (0.140 to 0.231), 100% | −16.5 ms |
| over 300 ms | 15 | 0.744 (0.610 to 0.930), 80% | 0.098 (0.058 to 0.131), 100% | −270.0 ms |

The design note's 1.609 for the short class (19% of files faster) is now 0.673 (97% faster). A short AV1 thumbnail takes a median 9.6 ms against FFmpeg's 14.9 ms. The 8 ms was the planning of long MP4 files, which `docs/design/seek.md` removed. The WebM files were never part of it: the 3 short ones are at 0.649. More files are under 50 ms than in the design note's table (37 against 27), because this host and this FFmpeg build are faster.

### Bar 3: the thumbnail bars for AV1

| Bar | Development (62) | Held-out (26) |
|---|---|---|
| S1 | 0.249 (0.214 to 0.466), 100% | 0.244 (0.123 to 0.403), 100% |
| S2 | 0.680 (0.651 to 0.721), 97% | 0.654 (0.609 to 0.702), 96% |
| S3 | 0.726 (0.686 to 0.757), 92% | 0.748 (0.631 to 0.930), 77% |
| S4 | 0.753 (0.706 to 0.805), 100% | 0.715 (0.640 to 0.794), 100% |
| S5 | 0.195 (0.147 to 0.272), 100% | 0.222 (0.154 to 0.305), 100% |

The held-out files were timed in earlier rounds, so these are development numbers and back no claim that CoreAV is faster than FFmpeg.

### Bar 4 and the arms

The worker's overhead is 0.156, so bar 4 is missed on this run; the earlier run had 0.140. The overhead grows with the frame, as a fixed cost would not:

| FFmpeg's time | Files | `worker` minus `in_process` | `key_t1` | `key_t2` | `key_t4` | `exact_t1` | `exact_t2` | `exact_t4` |
|---|---:|---:|---|---|---|---|---|---|
| under 50 ms | 37 | +1.0 ms | 0.875 (−1.0 ms) | 0.878 (−1.0 ms) | 0.920 (−0.7 ms) | 0.894 (−0.9 ms) | 0.905 (−0.8 ms) | 0.929 (−0.5 ms) |
| 50 to 300 ms | 36 | +3.4 ms | 0.966 (−1.0 ms) | 0.958 (−1.1 ms) | 0.972 (−0.7 ms) | 2.346 (+98.6 ms) | 1.570 (+50.9 ms) | 1.131 (+14.1 ms) |
| over 300 ms | 15 | +18.8 ms | 1.060 (+6.9 ms) | 0.991 (−1.0 ms) | 0.993 (−0.7 ms) | 4.235 (+2,429 ms) | 2.668 (+1,200 ms) | 1.533 (+302 ms) |

Each threads cell is the median of the arm over the default (one thread a core), with the median difference.
- **Threads.** dav1d's default costs about 1 ms on a thumbnail that decodes one frame. With `--exact`, fewer threads are much slower once more than a few frames are decoded, so `--exact` keeps the default.
  - In keyframe mode, 2 threads (0.878 under 50 ms, 0.991 over 300 ms) and 4 threads (0.920, 0.993) pass this page's rule for a proposal.
  - That change would be a separate one, with its own bars and a fresh held-out draw. It is about 1 ms, and short thumbnails are already 0.541 of FFmpeg's time in that mode.
- **Worker.** The worker adds 1.0 ms on short files and grows with the decoded frame: by the source's size (from `ffprobe`, not part of the run's evidence), a median 1.0 ms under 0.4 megapixels (35 files), 2.0 ms up to 1 (17), 3.8 ms up to 2.5 (12) and 18.5 ms above (24). That is a cost per byte of the frame, not of starting the worker, and it is what moves bar 4.

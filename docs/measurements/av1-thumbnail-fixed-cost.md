# AV1 thumbnails: the fixed cost of short decodes

Status: bars and method committed before the run; results below once it has run.

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

Not yet run.

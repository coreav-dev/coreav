# `coreav transcode`: the `x264` arm on 10-bit sources

Status: planned. The method and the criteria below were committed before the run.

Round 2 of [`transcode.md`](transcode.md) missed B5 on the `x264` arm in HEVC (1.200) and ProRes (1.890), where the `ffmpeg` arm took 1.057 and 1.108 on the same files. That page's profile after the run pointed at 10-bit input to the x264 program, on synthetic frames, and was not measured on the corpus. This page asks one question: is the `x264` arm's extra time on 10-bit sources spent inside the x264 program, on a thread that runs serially, so that no form of input CoreAV could pipe removes it?

## What round 2 already shows

Round 2's per-file results ([`transcode/round-2/speed/files.jsonl`](transcode/round-2/speed/files.jsonl)), split by the source's pixel format (from [`transcode/speed-selection.json`](transcode/speed-selection.json)), with no new run. The ratio is the `x264` arm's time over the `ffmpeg` arm's on the same file; both arms decode the same way and differ only in the encoder program.

| Sources | Files | `x264` arm over `ffmpeg` arm, median | Range |
|---|---|---|---|
| 8-bit | 56 | 0.991 | 0.878 to 1.004 |
| 10-bit | 10 | 1.428 | 1.129 to 2.062 |

Every file whose source is 10-bit is slower on the `x264` arm, and no 8-bit file is. The 10-bit files are 5 of the 8 HEVC files, 1 of the 6 AV1 files and all 4 timed ProRes files:

| Class | Source | `x264` arm | `ffmpeg` arm | `x264` over `ffmpeg` |
|---|---|---|---|---|
| HEVC | 3840x2160 `yuv420p10le` | 1.599 | 1.077 | 1.484 |
| HEVC | 3840x2160 `yuv420p10le` | 1.175 | 1.041 | 1.129 |
| HEVC | 5728x3024 `yuv420p10le` | 1.225 | 1.063 | 1.153 |
| HEVC | 3840x2160 `yuv420p10le` | 1.265 | 1.054 | 1.201 |
| HEVC | 1920x1080 `yuv420p10le` | 1.408 | 1.058 | 1.331 |
| AV1 | 3840x2160 `yuv420p10le` | 1.461 | 1.065 | 1.373 |
| ProRes | 1920x1080 `yuv422p10le` | 2.015 | 1.084 | 1.860 |
| ProRes | 3840x2160 `yuv422p10le` | 2.363 | 1.146 | 2.062 |
| ProRes | 4096x2160 `yuv422p10le` | 1.660 | 1.103 | 1.504 |
| ProRes | 4096x2160 `yuv422p10le` | 1.764 | 1.113 | 1.585 |

A check on a development VM (2 vCPUs; not a measurement) gave the hypothesis this run tests. With 10-bit input, the x264 program's main thread spent about a fifth of all its samples in one function, `get_frame`, which does not run at all with 8-bit input. It did so with every input form tried: raw `i420` and `nv12`, `--demuxer y4m`, `--input-depth 10` and `16`, and `--thread-input`.

## What is measured

- **Code:** `main` at `45bc5a2`. No CoreAV code changes. `coreav` built with `cargo build --release -p coreav-cli --features native-h264,dav1d,ffmpeg-fallback`, the features of round 2.
- **x264:** the program of round 2, commit 4613ac3, built by `scripts/install-x264.sh` on the development VM. Its SHA-256 is recorded with the host; it differs from round 2's build, which another compiler made.
- **FFmpeg:** the pinned static binary, 7.0.2 (`scripts/install-oracle-ffmpeg.sh`), with its libx264 (core 164, the same commit).
- **Host:** one Compute Engine `c4-standard-8`, not preemptible, two threads a core, as in round 2, by `scripts/gcp_vm_job.py`. Its CPU and kernel are recorded. `perf` is Debian's `linux-perf`.
- **Profiles:** function level only. The x264 program by thread and function; FFmpeg by thread name only (its static build has no symbols). No source line is read for either program.
- **Inputs:** three corpus v1 development files, none held out: a ProRes 3840x2160 `yuv422p10le` file (129 frames), a ProRes 1920x1080 `yuv422p10le` file (120 frames) and an HEVC 1920x1080 `yuv420p` file (598 frames).

## Method

The runner is [`transcode/x264-10bit/profile.py`](transcode/x264-10bit/profile.py), run by [`job.sh`](transcode/x264-10bit/job.sh).

1. **The encoder programs on the same frames.** For each case, the pinned `ffmpeg` decodes the source's frames once to a raw file on a tmpfs. Then, 3 rounds in rotated order, the x264 program with the arguments `coreav transcode` gives it (with `--fps` for the timecode file) and `ffmpeg -f rawvideo ... -c:v libx264 -preset medium -crf 23` read the raw file through a pipe. Each run is timed from outside, and the CPU time of each of its threads is sampled from `/proc` every 50 ms. The cases:
   - `prores-4k-422p10`: the 4K ProRes frames as `yuv422p10le` (x264 `i422`, depth 10);
   - `prores-4k-420p10`: the same frames as `yuv420p10le` (`i420`, depth 10);
   - `prores-4k-420p8`: the same frames as `yuv420p` (`i420`, depth 8), the 8-bit control on the same pictures;
   - `prores-1080-422p10`: the 1080p ProRes frames as `yuv422p10le`;
   - `hevc-1080-420p8`: the HEVC frames as `yuv420p`.
2. **Profiles.** One `perf record -e cpu-clock -F 999` of each program on each case.
3. **Other input forms**, on `prores-4k-420p10`: `--demuxer y4m`, `--input-depth 16` (the frames as `yuv420p16le`), and `--thread-input`, each timed once and profiled once.
4. **The command.** `coreav transcode IN OUT.mp4 --preset medium --crf 23 --audio none` with `--encoder x264` and with `--encoder ffmpeg`, and `ffmpeg -i IN -map 0:v:0 -an -c:v libx264 -preset medium -crf 23`, on each of the three sources, 3 rounds in rotated order.

A time is the median of its rounds.

## Criteria

Judged on this run, each case on its own:

1. **The program.** On each 10-bit case, the x264 program's time over libx264's is at least 1.25. On each 8-bit case it is at most 1.05.
2. **A serial stage.** On each 10-bit 4K case, the x264 program's main thread has CPU time of at least 85% of the program's wall time. On each 8-bit case it has at most 25%.
3. **The conversion.** In the profile of each 10-bit case, `get_frame` is the main thread's top function and has at least half of the main thread's samples. In each 8-bit case it has under 1% of all samples. Each other input form of step 3 meets the 10-bit half of this criterion.
4. **The command.** On the two 10-bit sources, `--encoder x264` over `--encoder ffmpeg` is at least 1.25. On the 8-bit source it is at most 1.05.

**What follows from the result.** If all four hold, the `x264` arm's extra time on 10-bit sources is the x264 program's own conversion of its input, on its main thread, which no input form tried avoids. CoreAV then changes no code for it, and this page records why the `x264` arm stays above the bar on 10-bit sources. If criterion 2 or 3 fails, the cause is elsewhere and is found before any code is written.

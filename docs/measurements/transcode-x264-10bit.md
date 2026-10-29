# `coreav transcode`: the `x264` arm on 10-bit sources

Status: measured. The method and the criteria were committed before the run (`e23d749`). The answer is yes. The `x264` arm's extra time on 10-bit sources is the x264 program's own conversion of its input, and no input that `coreav transcode` can stream avoids it. CoreAV's code is unchanged.

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

## Results

Evidence: [`transcode/x264-10bit/profile/`](transcode/x264-10bit/profile) (`results.jsonl`, one `perf-*.txt` table for each program and case, `host.txt`). The host had an Intel Xeon Platinum 8581C, the CPU of round 2, with SMT on.

**The encoder programs on the same frames** (median of 3 rounds; the main thread's share is its CPU time over the program's wall time):

| Case | Frames | x264 program | libx264 | x264 over libx264 | x264 main thread busy | `get_frame` in the main thread |
|---|---|---|---|---|---|---|
| `prores-4k-422p10` | 129 | 28.57 s | 19.04 s | 1.500 | 77% | 90% |
| `prores-4k-420p10` | 129 | 22.08 s | 15.42 s | 1.432 | 74% | 87% |
| `prores-1080-422p10` | 120 | 6.04 s | 3.20 s | 1.884 | 83% | 87% |
| `prores-4k-420p8` | 129 | 12.33 s | 12.34 s | 0.999 | 9% | none |
| `hevc-1080-420p8` | 598 | 16.33 s | 16.31 s | 1.001 | 7% | none |

The last column is an estimate. It is `get_frame`'s share of all the profile's samples, times the program's CPU time, over its main thread's CPU time, both taken from the timed runs. `get_frame` is the profile's top function in each 10-bit case, and all of its samples are in the main thread. In the 8-bit cases it is not among the 25 top entries, the lowest of which have under 0.5% of the samples. The conversion is also the x264 program's whole extra CPU time. At 4:2:2 the program took 143.4 CPU seconds and libx264 124.9, and `get_frame` took about 19.8. At 4:2:0 the figures are 116.9, 101.3 and 14.2.

**Other input forms**, on `prores-4k-420p10` (one run each): `--demuxer y4m` 21.91 s with `get_frame` at 91% of the main thread, `--thread-input` 22.20 s and 90%, `--input-depth 16` 18.22 s and 78%.

**The command** (median of 3 rounds, video only):

| Source | `--encoder x264` | `--encoder ffmpeg` | FFmpeg's command | `x264` over `ffmpeg` |
|---|---|---|---|---|
| ProRes 4K `yuv422p10le` | 30.71 s | 21.84 s | 20.81 s | 1.406 |
| ProRes 1080p `yuv422p10le` | 6.44 s | 3.84 s | 3.54 s | 1.676 |
| HEVC 1080p `yuv420p` | 16.62 s | 16.57 s | 16.03 s | 1.004 |

**Criteria:**

1. Met: 1.500, 1.432 and 1.884 on the 10-bit cases, 0.999 and 1.001 on the 8-bit cases.
2. **Missed.** The x264 program's main thread was busy 77% and 74% of the wall time on the 10-bit 4K cases, below 85%. It was busy 9% and 7% on the 8-bit cases. The conversion is therefore not the program's only limit: about a quarter of the time the main thread waits, and the encoding threads overlap part of the conversion.
3. Met: `get_frame` is the main thread's top function, with 87% to 90% of its samples on the 10-bit cases and 78% to 91% in the other input forms. It is absent on the 8-bit cases.
4. Met: 1.406 and 1.676 on the 10-bit sources, 1.004 on the 8-bit source.

The missed criterion 2 did not point at a cause outside the x264 program. Criterion 1 times the two encoder programs on the same frames with no CoreAV code involved, and its gap is about the size of the command's gap in criterion 4. So the work went on to the one finding that pointed at a change on CoreAV's side, `--input-depth 16`, instead of looking elsewhere. That is a departure from the rule above, and it is recorded here.

## Development check: 16-bit input

`--input-depth 16` took 17% less time in the run above. Checks on the development VM showed that 16-bit samples (each 10-bit sample shifted up 6 bits) give the same stream as the 10-bit input. That held for synthetic 4:2:0, 4:2:2 and 4:4:4 frames in both ranges, and through `coreav transcode` on a 10-bit ProRes 4:2:2 source and on 4:2:0 and full-range 4:4:4 10-bit sources made from it. A change that did this for the x264 program ([`transcode/x264-10bit/input-depth-16.diff`](transcode/x264-10bit/input-depth-16.diff)) was timed in two development runs on the same host class, with the same three files. Neither run is a round, and neither judges a bar.

**The depth, from a file** ([`ab.py`](transcode/x264-10bit/ab.py), [`ab/`](transcode/x264-10bit/ab); median of 3 rounds; every depth gave the same stream as depth 10):

| Input | 4:2:0 | 4:2:2 |
|---|---|---|
| `--input-depth 10` | 22.23 s | 28.59 s |
| `--input-depth 12`, samples shifted 2 bits | 22.19 s | |
| `--input-depth 14`, samples shifted 4 bits | 22.45 s | |
| `--input-depth 16`, samples shifted 6 bits | 18.02 s | 22.46 s |

**The command with the change**, in the same run: 30.95 s against 30.73 s on the 4K ProRes source, 6.50 s against 6.46 s on the 1080p one, and 16.81 s against 16.72 s on the 8-bit HEVC source. The output was the same stream, and the x264 program's main thread spent 22.2 s as before.

**File or pipe** ([`diag.py`](transcode/x264-10bit/diag.py), [`diag/`](transcode/x264-10bit/diag); 4K ProRes frames as 4:2:2, 2 rounds each):

| x264 program's input | `--input-depth 10` | `--input-depth 16` |
|---|---|---|
| Standard input redirected from the file | 29.38 s, 28.53 s | 22.95 s, 23.21 s |
| The file's path | 28.77 s, 29.08 s | 22.96 s, 23.31 s |
| A pipe (64 KiB) | 28.94 s, 29.13 s | 29.06 s, 28.78 s |
| A pipe of 1 MiB (`F_SETPIPE_SZ`) | 28.78 s, 29.36 s | 28.59 s, 28.81 s |
| A pipe, with `coreav transcode`'s timecode and colour options | 29.21 s, 29.51 s | 29.28 s, 28.80 s |

From a regular file the x264 program maps its input into memory (`strace`: 90 `mmap` calls and 2 `read` calls for 10 frames). Only that path gets the faster 16-bit conversion. Through a pipe, which is how `coreav transcode` streams frames to the encoder, 16-bit input costs what 10-bit input costs, at either pipe size. A regular file would mean writing the whole decoded video before encoding (about 56 GB for the 70 s 4K ProRes file of the speed selection), which the pipeline's design rules out ("Pipeline" in [`docs/design/transcode.md`](../design/transcode.md)). So the change was not kept.

## Conclusion

On the round-2 host class, the `x264` arm's extra time on 10-bit sources is the x264 program's conversion of its 10-bit input (the function `get_frame`). It runs on the program's main thread before each frame is encoded, and it does not run for 8-bit input. Every streamed input form tried pays the same: raw `i420` and `nv12`, `y4m`, input depths 10, 12, 14 and 16, `--thread-input`, and pipes of 64 KiB and 1 MiB. The `ffmpeg` arm gives libx264 the same frames and shows no such cost (criterion 1). CoreAV changes no code for it. The `x264` arm keeps missing B5 in the classes whose sources are 10-bit (HEVC, AV1 and ProRes in the speed selection) until the program changes or the design routes 10-bit frames differently.

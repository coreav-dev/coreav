# `coreav transcode`: the `x264` arm on 10-bit sources

Status: measured. The method and the criteria were committed before the run (`e23d749`). The answer is yes. The `x264` arm's extra time on 10-bit sources is the x264 program's own conversion of its input, and no input that `coreav transcode` can stream avoids it. CoreAV's code is unchanged.

Round 2 of [`transcode.md`](transcode.md) missed B5 on the `x264` arm in HEVC (1.200) and ProRes (1.890), where the `ffmpeg` arm took 1.057 and 1.108 on the same files. That page's profile after the run pointed at 10-bit input to the x264 program, on synthetic frames, and was not measured on the corpus. This page asks one question: is the `x264` arm's extra time on 10-bit sources spent inside the x264 program, on a thread that runs serially, so that no form of input CoreAV could pipe removes it?

## What round 2 already shows

Round 2's per-file results ([`transcode/round-2/speed/files.jsonl`](transcode/round-2/speed/files.jsonl)), split by the source's pixel format (from [`transcode/speed-selection.json`](transcode/speed-selection.json)), with no new run. The ratio is the `x264` arm's time over the `ffmpeg` arm's on the same file; both arms decode the same way and differ only in the encoder program.

| Sources | Files | `x264` arm over `ffmpeg` arm, median | Range |
|---|---|---|---|
| 8-bit | 56 | 0.991 | 0.878 to 1.004 |
| 10-bit | 10 | 1.428 | 1.129 to 2.062 |

Every file whose source is 10-bit is slower on the `x264` arm, by 13% or more. The 8-bit files are at most 0.5% slower than on the `ffmpeg` arm: 6 of them are slightly above it, at most 1.0044. The 10-bit files are 5 of the 8 HEVC files, 1 of the 6 AV1 files and all 4 timed ProRes files:

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

1. **The encoder programs on the same frames.** For each case, the pinned `ffmpeg` decodes the source's frames once to a raw file on a tmpfs. Then, 3 rounds in rotated order, the x264 program with the arguments `coreav transcode` gives it (with `--fps` for the timecode file) and `ffmpeg -f rawvideo ... -c:v libx264 -preset medium -crf 23` read the raw file on standard input. The plan said a pipe, but the runner opens the file and hands it to the program as standard input, so both programs read a regular file (see "Which input was a file"). Each run is timed from outside, and the CPU time of each of its threads is sampled from `/proc` every 50 ms. The cases:
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

**Which input was a file.** In steps 1 to 3 of this run the encoder programs read the raw file itself on standard input, a regular file and not a pipe, as the plan wrongly said. Step 4, the command, gives x264 its frames through a pipe, as `coreav transcode` always does. In the development checks below, the depth table read a regular file on standard input, the command rows went through a pipe, and the last table names its input on each row. The 16-bit result depends on that difference.

The missed criterion 2 did not point at a cause outside the x264 program. Criterion 1 times the two encoder programs on the same frames with no CoreAV code involved, and its gap is about the size of the command's gap in criterion 4. So the work went on to the one finding that pointed at a change on CoreAV's side, `--input-depth 16`, instead of looking elsewhere. That is a departure from the rule above, and it is recorded here.

## Development check: 16-bit input

`--input-depth 16` took 17% less time in the run above. Checks on the development VM showed that 16-bit samples (each 10-bit sample shifted up 6 bits) give the same stream as the 10-bit input. That held for synthetic 4:2:0, 4:2:2 and 4:4:4 frames in both ranges, and through `coreav transcode` on a 10-bit ProRes 4:2:2 source and on 4:2:0 and full-range 4:4:4 10-bit sources made from it. A change that did this for the x264 program ([`transcode/x264-10bit/input-depth-16.diff`](transcode/x264-10bit/input-depth-16.diff)) was timed in two development runs on the same host class, with the same three files. Neither run is a round, and neither judges a bar.

**The depth, from a regular file on standard input** ([`ab.py`](transcode/x264-10bit/ab.py), [`ab/`](transcode/x264-10bit/ab); median of 3 rounds; every depth gave the same stream as depth 10):

| Input | 4:2:0 | 4:2:2 |
|---|---|---|
| `--input-depth 10` | 22.23 s | 28.59 s |
| `--input-depth 12`, samples shifted 2 bits | 22.19 s | |
| `--input-depth 14`, samples shifted 4 bits | 22.45 s | |
| `--input-depth 16`, samples shifted 6 bits | 18.02 s | 22.46 s |

**The command with the change**, in the same run (x264 reads a pipe): 30.95 s against 30.73 s on the 4K ProRes source, 6.50 s against 6.46 s on the 1080p one, and 16.81 s against 16.72 s on the 8-bit HEVC source. The output was the same stream, and the x264 program's main thread spent 22.2 s as before.

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

On the round-2 host class, the `x264` arm's extra time on 10-bit sources is the x264 program's conversion of its 10-bit input (the function `get_frame`). It runs on the program's main thread before each frame is encoded, and it does not run for 8-bit input. From a regular file, raw `i420` and `nv12`, `y4m`, input depths 10, 12 and 14 and `--thread-input` all pay it in full (`nv12` was tried on the development VM only), and input depth 16 pays about half. Through a pipe, which is the only way `coreav transcode` streams frames, input depth 16 pays the same as depth 10, with a pipe of 64 KiB or 1 MiB. The `ffmpeg` arm gives libx264 the same frames and shows no such cost (criterion 1). CoreAV changes no code for it. The `x264` arm keeps missing B5 in the classes whose sources are 10-bit (HEVC, AV1 and ProRes in the speed selection) until the program changes or the design routes 10-bit frames differently.

## Why the conversion is slow

Status: measured. The questions and what each result would mean were committed before the first run (`e316c68`, with the host corrected in `256358a`), and the second run's additions before it (`3bca95f`). Questions 1, 2, 4, 5 and 6 are answered. Question 3 is answered for where the stall is and on which processor, and left open for its cause.

The sections above show where the time goes. This section asks why the conversion costs what it does. The method is the same as above: black-box binaries, `perf stat`, function-level `perf record` and `perf report`, `perf sched`, `strace` and timing. No x264 or FFmpeg source is read, and neither `perf annotate` nor disassembly is used. If a question cannot be settled without instruction-level annotation, the page says what is left open and why.

**Host and inputs.** One Compute Engine `c4-standard-8`, as above (SMT on, not preemptible), with the PMU's standard events, by `scripts/gcp_vm_job.py`. The standard events are every event inside a core; Compute Engine offers its enhanced events, which add those outside the cores, only on C4 machines of 144 or more vCPUs, a different processor. One `c4d-standard-8` (AMD EPYC) for question 3 only. Compute Engine has no PMU on that series, so it gives times and a profile sampled by the kernel's clock, and its VM is made by a copy of the same script with the PMU setting left out. The input is the 4K ProRes development file above, plus copies of its frames scaled by the pinned `ffmpeg`. Timing arms are run 3 times, and the median is taken. Counter groups are run once each, with at most 4 programmable events a group so that nothing is multiplexed.

**The arms.** The 4K ProRes frames as `yuv420p10le` go to the pinned x264 program with `--input-depth 10` or `--input-depth 16` (each sample shifted up 6 bits), from a regular file on standard input or through a pipe of 64 KiB. libx264 through the pinned `ffmpeg` reads the same 10-bit frames from the file.

1. **How much of the gap does the conversion explain?**
   - Observations: the x264 main thread's on-CPU time by function (sampled, function level); its off-CPU time by what it waits in (`perf sched timehist`, and `strace -T` on its reads and futex waits); and the slope of wall time against the main thread's `get_frame` time between the 10-bit and 16-bit file arms.
   - A slope of 0.8 or more means the conversion is almost all on the critical path, so the gap is the conversion.
   - A slope of 0.3 or less means the main thread mostly overlaps with encoding. The off-CPU split then has to show the second cause: input reads or waits for encoder threads.
   - A slope between the two is reported as a partial explanation, with the off-CPU split beside it.
2. **Is the conversion compute-bound or memory-bound?**
   - Observations: Top-down levels 1 and 2 on the main thread (`perf stat -M TopdownL1,TopdownL2` on that thread), and, for `get_frame`, sampled instructions per cycle, L1, L2 and L3 load misses, and cycles stalled on L3 misses.
   - Backend-bound with most of it memory-bound, and L3 misses for at least 1% of loads, means it is limited by memory bandwidth or latency.
   - Core-bound or retiring, with an IPC under 1 and few misses, means a long or slow instruction stream.
   - Retiring with an IPC of 2 or more means the work itself is large: many instructions a sample.
3. **Why is 16-bit input fast only from a file, and only on Intel?**
   - Observations: for `get_frame` in the four arms (10 or 16 bits, file or pipe), the sampled counts of microcode assists (all, and floating point), loads blocked by 4K aliasing, loads blocked on store forwarding, split loads and stores, and machine clears per sample converted. Then the four arms' times on the AMD host.
   - An event that is at least 10 times higher per sample in the three slow arms than in the fast one, the 16-bit file arm, is named as the mechanism.
   - Assists pointing at floating point mean value-dependent slow paths, such as denormal numbers.
   - Aliasing or split accesses mean the alignment of the input buffer, which a regular file and a pipe set differently.
   - The same times in all four arms on AMD mean the penalty is particular to this Intel core.
   - If no event separates the arms, the question stays open, and the page says that annotation would be needed to settle it.
4. **Is the conversion vectorised?**
   - Observations: `get_frame`'s instructions retired per sample converted, and the floating-point and integer vector instruction counts by width where the PMU has them.
   - Under 0.5 instruction a sample means vector code, and the width counts say which.
   - 2 or more a sample, with only scalar counts, means scalar code.
   - In between is reported as such.
5. **How does it scale?**
   - Observations: `get_frame` time per sample, and x264 over libx264, at 1280x720, 1920x1080, 3840x2160 and 7680x4320 (10-bit 4:2:0 from a file); and the x264 program at 4K with `--fps 24` and `--fps 60`.
   - A per-sample time within ±15% across sizes means a cost per sample.
   - None of these frames fits in the core's 2 MiB L2 (a 720p frame is 2.8 MB at 2 bytes a sample). A per-sample time that grows with frame size by more than 15% therefore means a cache or memory effect beyond L2, and the page says between which sizes the step falls.
   - `--fps` is expected to change nothing. A difference of more than 3% is a finding.
6. **Why does libx264 through FFmpeg not pay it?**
   - Observations: the pinned static `ffmpeg` has no symbols, so Debian's packaged `x264` and `ffmpeg` programs, which link Debian's one shared `libx264`, are profiled by binary and function on the same 10-bit file. The pinned binaries stay the reference for every timing on this page.
   - If Debian's `x264` program also spends its main thread in a function of the x264 program itself, with the library's functions taking the time libx264 takes under FFmpeg, then the conversion belongs to the x264 command-line program and not to the library.
   - What FFmpeg hands the library is then seen only as the absence of that step: no time in a conversion function outside `libx264`, or in FFmpeg's own scaling library.
   - If function names are not enough to tell, the question stays open here; a symbol-table build of the pinned FFmpeg would be a separate decision.

**Second run, added after the first.** The first run's counter groups left question 3 without its main candidate. The 4K-aliasing event was asked for by an older processor's name, which this one does not have, and `perf sched` could not read the kernel's trace events. The first run also showed that, in all four arms, `get_frame` retires the same instructions, loads and stores, and that the extra cycles of the three slow arms are stalls the top-down method counts as memory-bound, with no more L1 or L2 misses than the fast arm. The second run, by `why2.py` on another VM of the same type, takes the same four arms and adds:

- on the main thread, with `perf stat`: loads blocked by 4K aliasing or by a missing store-forward match, DTLB load and store walks and STLB hits, cycles bound on stores or loads, cycles stalled on a full store buffer or full fill buffers, memory-ordering machine clears, and the top-down level-3 split of memory-bound time (L1, L2, L3, DRAM, store) with its level-4 parts that this processor's metrics name;
- a function-level sampled record of the 4K-aliasing group, by thread;
- `perf sched`, run as root, for the main thread's run-queue delay beside its sleeps.

What each result means:

- The rule of question 3 applies unchanged: an event at least 10 times higher per sample in the three slow arms than in the 16-bit file arm is named as the mechanism.
- The top-down part that carries most of the slow arms' extra memory-bound time names where the stall is (an L1 hit blocked, a TLB walk, or stores), and its level-4 part names the cause if one part carries it.
- If no event or part separates the arms, question 3 stays open, and the page says so.

The evidence for this section goes under `transcode/x264-10bit/why/`. If a result points at something CoreAV could change, such as the layout of the frames it writes, that change gets its own pre-registered bars before any code.

### What the runs found

Each input is named as a file or a pipe. "File" is the regular file itself on the x264 program's standard input, which the program maps into memory. "Pipe" is a 64 KiB pipe that `why.py` fills. All runs use the 129 frames of the 4K ProRes file as `yuv420p10le`, 1.605 billion samples, except where a size is named. Times are medians of 3 runs. Counter values are for the x264 main thread, and per function where a function is named. The four x264 arms gave the same output stream (one hash), so they do the same work.

**Departure: some counters were multiplexed.** The plan said that nothing would be multiplexed. That holds for the explicit event groups: the first run's sampled groups, the second run's `perf stat` groups of at most 4 programmable events, and the first run's top-down levels 1 and 2. Their files show no running-time share. It does not hold for the second run's metric lists (`perf stat -M`), whose events perf grouped itself and the kernel scheduled in turn. Each level-3 metric ran about 25% of the time, and the level-4 lists ran 8 to 46%. Their values are scaled estimates, not exact counts. That covers the level-3 and level-4 shares below (L1-bound at 45.8%, and so on) and the slot counts derived from them, such as the ratio of about 12 (11.93) in L1-bound slots. The cycle counts that question 3's conclusion also rests on come from the explicit groups: all stall cycles, and cycles bound on loads.

| Arm (Intel `c4-standard-8`) | Wall (s) | Main thread on CPU (s) | `get_frame` cycles (billions) | `get_frame` instructions (billions) | IPC |
|---|---|---|---|---|---|
| x264, 10-bit, file | 22.11 | 16.46 | 48.8 | 33.7 | 0.69 |
| x264, 16-bit, file | 17.95 | 8.09 | 20.8 | 33.6 | 1.61 |
| x264, 10-bit, pipe | 22.32 | 16.54 | 48.9 | 33.7 | 0.69 |
| x264, 16-bit, pipe | 22.18 | 16.50 | 48.9 | 33.7 | 0.69 |
| libx264 through `ffmpeg`, file | 15.37 | 0.01 | – | – | – |

1. **How much of the gap does the conversion explain? Most of it, at about half a second of wall time per second of conversion.** From the 10-bit to the 16-bit file arm, the sampled `get_frame` time drops from 14.5 to 6.4 s, and wall time drops by 4.16 s. The slope is 0.51, between the two pre-registered bounds, so this is a partial explanation. The off-CPU split shows why it is half. The main thread sleeps on futexes for 5.3 to 5.4 s in the three slow arms and 9.1 s in the 16-bit file arm (`perf sched`, second run; `strace`, first run: 5.24 and 9.24 s in `futex`). It waits 0.6 to 0.9 s in the run queue, and it spends 0.01 to 0.02 s in `read`. So when the conversion shrinks, about half of the time saved becomes waiting for the encoding threads. Input reads cost nothing. A straight line through the two file arms reaches 14.67 s at zero conversion, against libx264's 15.37 s. If the relation stays linear below the 16-bit arm's conversion time, the conversion accounts for the whole wall-time gap, and no second cause is needed. That is an extrapolation from two points, not a run without the conversion.
2. **Compute-bound or memory-bound? Neither, in the pre-registered sense: it waits on loads that hit L1.** In the 10-bit file arm the main thread is 87% backend-bound: 54% memory-bound, 33% core-bound, and 12% retiring. The IPC in `get_frame` is 0.69. But 0.74% of its loads miss L1 and 0.03% miss L2. This host's PMU reads the L3-miss events as zero, and the L2 misses bound L3 misses at 0.03% of loads, far under the 1% that would mean bandwidth or latency to memory. So neither pre-registered outcome fits. The top-down level-3 split (second run, multiplexed, so a scaled estimate) puts about 45.8% of the main thread's slots in L1-bound, which is loads that hit L1 and still stall, against 0.5% in L2-bound, 3.7% in L3-bound, 0% in DRAM-bound and 3.4% in store-bound.
3. **Why from a file and not a pipe, and why Intel? Where: stalls on L1-hit loads on the Intel host, and the AMD host shows no file or depth timing penalty. Why: open.** `get_frame` retires the same instructions in all four arms: 21.0 a sample, with 4.0 loads, 2.0 stores and 1.0 branch a sample. The 16-bit file arm runs them at 1.61 IPC against 0.69. Its estimated L1-bound share is 7.6%, against 45.8 to 47.5% in the three slow arms. That is about 12 times fewer L1-bound slots (11.93), a scaled estimate. The explicit groups agree. Cycles with a load outstanding and nothing executing number 26.9 to 27.8 billion in the slow arms against 3.94 billion, about 7 times more, and all execution stalls in `get_frame` number 25.1 to 26.0 billion against 2.45 billion. Of the events that would name a cause, none separates the arms. Per million loads, the counts in the fast arm and in the three slow arms are as follows (in `get_frame` from the first run's sampled groups, and on the whole main thread from the second run's `perf stat` for 4K aliasing, clears and DTLB walks):
   - 4K-aliasing blocks: 263, and 224 to 228;
   - store-forward blocks: 748, and about 9;
   - split loads: 10, and 16 to 20;
   - microcode assists: 7, and 0;
   - memory-ordering machine clears: 8, and 4;
   - DTLB walk cycles: 3,488, and 1,059 to 1,121.

   The slow arms' counts are the same or lower. Cycles stalled on a full store buffer (2.4 to 2.5 billion) and on full fill buffers (2.0 to 2.2 billion) are the same in all four arms. The level-4 parts of L1-bound that this processor's metrics name (DTLB, store-forward blocks, split loads, lock latency and full fill buffers) carry an estimated 0 to 7% each and do not separate the arms either. Two general stall counts pass the 10-times rule: all execution stalls in `get_frame` (exact, 10.2 to 10.6 times) and L1-bound slots (scaled, about 12 times). They say where the time goes, not why, so they are not named as a mechanism. On the AMD host (`c4d-standard-8`, EPYC 9B45), the four arms take 11.73, 11.59, 11.75 and 11.70 s, with 5.9 to 6.0 s on the main thread. That is the same in all four arms, so the AMD host shows no file or depth timing penalty, and that penalty is particular to the Intel host. Compute Engine gives that host no PMU, so the run says nothing about whether it stalls on L1-hit loads. So the slow arms wait on loads that hit L1 and that no named blocking event explains. The cause is open. What is left is which load in `get_frame` waits, and on what. Only an instruction-level view (`perf annotate`) would show that, and this page does not use one.
4. **Is the conversion vectorised? No.** `get_frame` retires 21.0 instructions a converted sample. Its floating-point instructions of any width total under 0.9 million, and its integer vector additions, shuffles and multiplications under 0.08 million each, for 1.6 billion samples. That is 2 or more instructions a sample with no vector counts to speak of, the pre-registered sign of scalar code. It is also consistent with the 1.0 branch a sample.
5. **How does it scale? Per sample, with a step of about 20% between 1080p and 4K.** `get_frame` costs 7.7 ns a sample at 1280x720 and at 1920x1080, 9.0 ns at 3840x2160 and 9.2 ns at 7680x4320 (48 frames at 8K). The spread is 20%, above the 15% that would mean a pure cost per sample. So, as pre-registered, it is a cache or memory effect beyond L2, with the step between frames of 6.2 MB and 24.9 MB. Every one of these frames fits in this host's 260 MiB L3. The x264 arm over libx264 is 1.42, 1.37, 1.44 and 1.64 at the four sizes. `--fps 60` against `--fps 24` on 4K is a finding (wall 21.25 against 22.87 s, 7% less), but not in the conversion. The output stream changes, and the program uses 19% less CPU in all, all of it outside the main thread, which is the same (17.0 against 16.8 s).
6. **Why does libx264 through FFmpeg not pay it? Because the conversion is in the x264 command-line program, not in the library.** Debian's packages (x264 0.164.3108, ffmpeg 7.1.5, one shared `libx264.so.164`), profiled with their debug symbols for names only:
   - Debian's `x264` program spends 12.6% of all samples in `get_frame`, on its main thread, in the program's own binary. Its encoding functions are in `libx264.so.164`.
   - Debian's `ffmpeg` with the same library has no row from FFmpeg's own libraries among its 60 largest thread and function rows. Apart from two kernel rows, all of them are `libx264.so.164` functions.
   - Their totals match the pinned pair: 121.3 against 106.8 CPU-seconds and 22.94 against 16.22 s wall. That is a difference of 14.5 CPU-seconds (15.3 for the pinned pair) and a wall ratio of 1.41 (1.44).

**What this means for CoreAV.** No lever from outside the x264 program is confirmed. The conversion is scalar code in the program that stalls on L1-hit loads on this Intel core. Only 16-bit input from a regular file avoids most of the stall, and `coreav transcode` streams through a pipe. Writing every frame to a file first would trade the stall for spooling the whole video to disk, and it was not measured. The AMD host shows no file or depth timing penalty. The design's choice stays as it is (`docs/design/transcode.md`).

The raw results are in `transcode/x264-10bit/why/`: `run1/` (`why.py`, Intel), `run2/` (`why2.py`, Intel, a second VM of the same type) and `amd/` (`why.py --part amd`). The first run's `perf sched` step failed (no read access to the kernel's trace events, rc 129 in `run1/why.jsonl`), and its 4K-aliasing event was asked for by a name this processor does not have. The second run repeated both. In `perf report --group` tables, perf 6.12 leaves out the column of an event with no samples: L3-miss events in `mem` and `stall`, `assists.fp` in `assist`, and 2 of 4 events in `fp1` and all 4 in `fp2`. The `perf stat` files of the second run print every event, including those at zero.

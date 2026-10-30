# Media on standard output

Status: Accepted. Scope: every `coreav` command that writes its output to
`-`.

## What changes

`remux`, `sanitize`, `clip`, `extract-audio`, `decode-audio`,
`extract-subtitles`, `join`, `thumbnail` and `hls index` write `-` through
`write_output_core`; `hls get` has its own path. Both used to write the
media through `BufWriter<StdoutLock>`. Rust's `Stdout` is line-buffered
whatever it is connected to, a pipe or a file included: every write that
reaches it runs `memrchr` for a newline over the whole buffer, and a write
that holds a 0x0a byte is split there and flushed. Media are full of 0x0a
bytes, so `-` paid a scan per write and extra system calls.

Both paths now write through `media_stdout()` in `main.rs`: a
`std::fs::File` on a duplicate of standard output, behind a `BufWriter`
with std's default 8 KiB, the same buffer a file output has.

- **Unix:** `std::io::stdout().as_fd().try_clone_to_owned()` (`dup`), then
  `File::from(OwnedFd)`.
- **Windows:** `std::io::stdout().as_handle().try_clone_to_owned()`
  (`DuplicateHandle`), then `File::from(OwnedHandle)`. A console then gets
  the bytes through `WriteFile`. Before, `Stdout` refused bytes that are not
  UTF-8 on a console, so media to a console failed. A process with no
  standard output has a null handle. std would duplicate it into a handle
  that fails every write, so a null handle keeps `Stdout`, which discards
  the bytes as before.
- **WASI and other targets:** there is no duplicate (WASI has no `dup`), so
  they keep `StdoutLock`, scan included. The npm package's WebAssembly build
  is the fallback, not the fast path.

Both are safe Rust; the workspace keeps `forbid(unsafe_code)`. Before the
duplicate is taken, Rust's own stdout buffer is flushed, so nothing printed
through `Stdout` earlier can land after the media. Text keeps going through
`Stdout`, line-buffered: the JSON documents, the summaries, `--help` and
`--version`. When the output is `-`, the
summary goes to standard error after the media are flushed, as before.
Nothing else writes to standard output while the media are written.

Errors are unchanged. A duplicate refers to the same open pipe, so a
closed reader fails the write with `EPIPE` (Windows: `ERROR_NO_DATA`, which
std reports as `BrokenPipe`), and `Watched` still turns that into exit 141.
A full device is still exit 3. A standard output closed before the command
starts (`>&-`) is `/dev/null`, opened by the Rust runtime, and its duplicate
discards the bytes the same way.

### Alternatives considered

- **`unsafe { File::from_raw_fd(1) }` wrapped in `ManuallyDrop`.** It saves
  one `dup` per command, but needs `unsafe` in a crate that forbids it.
  Rejected.
- **A bigger `BufWriter` over `StdoutLock`.** Every flush would still scan
  the whole buffer and split it at the last newline. Rejected.
- **A larger buffer.** `BufWriter` copies every write smaller than its
  capacity and passes larger ones straight to the kernel from the caller's
  buffer. The remux writes sample runs of a few KiB to a few tens of KiB
  from its read window.
  - **64 KiB everywhere (the first version).** It copies most of the sample
    data once more. The CI guard (standard output `/dev/null`) counted
    +18.7% instructions on `remux-fragmented` and +11.1% on
    `join-fragmented`: memcpy grew by 1.03 million instructions, while the
    scan this change removes had cost 0.16 million. Rejected.
  - **64 KiB only into a pipe or a socket, 8 KiB elsewhere (run-04).** Into
    a pipe each write wakes the reader, so fewer writes save kernel time
    that instruction counts do not see. Against 8 KiB into a pipe, 64 KiB
    took 0.889, 0.889 and 0.920 of the time on three inputs, but 1.072 on
    the fourth (55 MB; 1.052 of the base), in 11 interleaved rounds
    (`buffer-ab.md`). The guard writes to `/dev/null` and would not have
    counted this path. A guard case with standard output a pipe would count
    about +19% user instructions against the base. Rejected for now: the
    gain is small and mixed, and the guard should count every path users
    take. A pipe buffer needs a guard that also counts kernel work, such as
    the number of writes, before it can be judged.
  - 1 MiB, as the H.264 decode example uses, was not tried. 64 KiB is
    a Linux pipe's default capacity, so a larger write only waits for the
    reader to drain it.
- **Kernel copies for `hls get -`.** `std::io::copy` from a `File` into an
  unwrapped `File` would use `splice`/`sendfile` into a pipe on Linux, but
  `Watched` needs a wrapper to see `EPIPE`. Left for later.

## Measurement

The operation is `coreav remux IN -`, the largest writer of the
commands. Inputs and outputs are on a RAM disk, so storage does not count.
`docs/measurements/cli-stdout/bench.py` runs it, `analyze.py` computes the
ratios, and the results are in `docs/measurements/cli-stdout/`.

- **Inputs.** From corpus v1's MP4/MOV set (`~/corpus-mp4`). An input has
  exactly one video and one audio stream (ffprobe), and both CoreAV's remux
  and FFmpeg's command must succeed on it. After run-01, inputs where CoreAV
  drops a stream are also refused (see the result). Taken in this order:
  the file closest to 1 MB, then 4 MB, 12 MB and 30 MB, then the largest. Ties go to
  the name. A file that fails is skipped and listed.
- **Arms.**
  - `base-pipe` and `base-file`: `origin/main` before this change,
    writing `-` piped through `cat` to `/dev/null`, and writing a file.
  - `new-pipe` and `new-file`: the same two with this change.
  - `ffmpeg-pipe`: FFmpeg 9.0.2 (`n9.0.2-17-g2a571b6068`, the build the
    H.264 rounds use) running `ffmpeg -nostdin -v error -i IN -map 0 -c copy
    -f mp4 -movflags frag_keyframe+empty_moov pipe:1`, piped through `cat`
    to `/dev/null`. FFmpeg's MP4 muxer refuses unseekable output unless it
    fragments: its `-f mp4 pipe:1` is a fragmented MP4, while CoreAV writes
    the movie box first. The two are the same operation (stream copy to MP4
    on a pipe) in different layouts.
- **Identity.** Before timing, on every input: `base-pipe`, `base-file`,
  `new-pipe` and `new-file` give the same SHA-256.
- **Timing.** One warm-up pass, then 21 rounds. In each round every input
  runs every arm, and the arms' order rotates by one each round. A time is
  wall clock from start until both the command and `cat` have exited. The
  command is pinned to cores 2 to 5, which leaves FFmpeg room for its
  threads, and `cat` to core 6. For each arm and input, the time is the
  median of its rounds.
- **Host-health gate**, as in `docs/design/h264.md`. FFmpeg 9.0.2 decodes
  the first 300 frames of the middle input's video on one thread, pinned
  to core 2, three times; the gate takes the median. The reference is three of these a minute
  apart, which must agree within ±5%. Each round needs a gate within ±5% of
  the reference before it starts, waiting a minute between tries, and
  another after it ends. If the gate after fails, the round is void, kept in
  `times.json` and run again. If a round is void three times, the run
  stops.
- **Interval.** Bootstrap 95% percentile interval of the median ratio over
  the inputs, with inputs and rounds both resampled (10,000 resamples, seed
  407).

### Bars (fixed before the run)

- **B1, the fix:** `new-pipe / base-pipe`. The median over the inputs is at
  most **1.00**, and no input is above **1.05**, the gate's tolerance (no
  regression beyond noise; the 1 MB input runs in a few milliseconds).
- **B2, `-` against a file:** `new-pipe / new-file`. The median over the
  inputs is at most **1.10**. The pipe carries one more copy (into the
  pipe, then out of it into `cat`) than a write to the RAM disk does, hence
  the 10%. `base-pipe / base-file` is reported beside it, and the gap must
  not widen: on each input, B2's ratio is at most the before ratio plus
  0.02.
- **B3, against FFmpeg:** `new-pipe / ffmpeg-pipe`. The
  median per-input ratio is at most **1.00**. CoreAV is called faster only
  if the interval's upper bound is below 1.00.
- **Regression guard.** The CI guard (`scripts/bench_guard.py`,
  `docs/design/benchmarks.md`) already counts instructions for every media
  command writing to `-`: `remux-*`, `clip`, `extract-audio`,
  `decode-audio-*`, `sanitize`, `extract-subtitles`, `hls-index`,
  `hls-segment`, `join*` and `thumbnail`. These counts include the scan,
  so this change lowers them, and the guard catches any regression.

### Result (`docs/measurements/cli-stdout/run-05`)

**Every bar is met.** run-05 timed the final code (8 KiB everywhere; its
binary's SHA-256 is recorded in `inputs.json`) against base `28b2b22`, on
the 2.90 GHz VM class (Xeon family 6, model 106). The gate failed after
one round, which was void and run again (kept in `times.json`). Inputs were
0.97, 3.9, 11.9, 27.0 and 55.1 MB. Two larger files were refused because
CoreAV drops their MPEG-4 Part 2 video (below). On every input, CoreAV's
output and FFmpeg's are within 1% in size.

| Ratio | 1 MB | 4 MB | 12 MB | 27 MB | 55 MB | Median (95% CI) |
|---|---|---|---|---|---|---|
| B1: `new-pipe / base-pipe` | 0.945 | 0.834 | 0.953 | 0.937 | 0.963 | 0.945 (0.807 to 1.007) |
| `base-pipe / base-file` | 0.988 | 1.092 | 0.975 | 0.970 | 0.895 | 0.975 (0.873 to 1.138) |
| B2: `new-pipe / new-file` | 0.796 | 0.899 | 0.899 | 0.915 | 0.863 | 0.899 (0.812 to 1.006) |
| `new-file / base-file` (same code) | 1.172 | 1.012 | 1.034 | 0.993 | 0.999 | 1.012 (0.942 to 1.141) |
| B3: `new-pipe / ffmpeg-pipe` | 0.350 | 0.305 | 0.238 | 0.214 | 0.637 | 0.305 (0.215 to 0.606) |

- **B1.** Writing to `-` takes 0.945 of its old time, and no input is
  above 0.963.
- **B2.** `-` takes 0.899 of the time of writing a file. On each input the
  ratio is below the one before the change, so the gap narrowed everywhere.
  The file arms run the same code in both builds, yet differ by up to 17%
  (1.172 on 1 MB). So part of the 1 MB input's 0.796 comes from a slow file
  arm, not from the pipe.
- **B3.** `coreav remux IN -` takes 0.305 of the time FFmpeg 9.0.2 takes
  to write MP4 to `pipe:1`. The interval's upper bound is 0.606, below
  1.00, so CoreAV is faster.
- **Guard.** With standard output `/dev/null`, which runs the same code a
  pipe does, `remux-fragmented` counts 4.3% fewer instructions than the
  base.

**run-04** used the same harness on the rejected design (64 KiB into a
pipe). B1 was 0.930, B3 0.347, and B2's median 0.871. B2's per-input rule
missed on two inputs, by more than the 0.02 allowance. There, the file arms,
which run the same code in both builds, ranged from 0.902 to 1.067 of each
other.

**Runs 01 to 03 are void.** They are kept for the record (`VOID.txt`).
`bench.py` waited for each command with `Popen.wait(timeout=...)`, which
polls with sleeps that double from 1 ms up to 50 ms. Each time was
therefore rounded up to the next poll: one binary on the 27 MB input showed
exactly 16.3 or 32.4 ms. Runs 04 and 05 block in `waitpid`. What else those
runs showed:
- **run-01** measured 64 KiB everywhere. Its largest input (65 MB) has
  MPEG-4 Part 2 video, which the remux does not allow by default. CoreAV
  dropped the video and exited 0 with the audio alone, while FFmpeg copied
  both streams. `bench.py` now refuses an input where CoreAV drops a stream.
- **run-02** measured 8 KiB everywhere.
- **run-03** measured 64 KiB into a pipe.

A missed bar is recorded with its cause. It blocks a speed claim, not the
merge, since the change makes no output slower by construction.

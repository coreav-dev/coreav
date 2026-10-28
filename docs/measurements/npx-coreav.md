# `npx coreav` against FFmpeg

The speed measurement of the npm package `coreav` (`packages/cli`,
`docs/design/npm-cli.md`, "Speed"). Round 1 measured the package's first
form, with the command built for WASI under Node as the installed command.
Round 2 measured the package as it ships: the platform's native binary,
installed as the `coreav` command. The bars, the inputs, the method and
the analysis were committed before the run:
- `d71abdf` holds the bars (`fe4c09e` before a rebase onto `main`, with the same text);
- `cdc5610` holds the counting rules and `bench/npx/summarize.py`;
- `0ee8ba2` adds `split`'s row, after `coreav split` reached `main`.

The run used corpus v1's 302 held-out files and took place on 2026-10-19
on this repository's development host (8 vCPU Intel Xeon at 2.9 GHz,
Linux, Node 24.14.1, npm 11.11.0, FFmpeg 7.0.2).

## Round 2: the native path

Round 2's bars were committed in `b30fbaf` before its run: round 1's bars,
applied to the command as the package now installs it. The timed command
is the installed `coreav`: the main package and `@coreav/cli-linux-x64`,
packed and installed with npm into a new project, where `coreav` is the
native binary itself (`bench/npx/run.py --path native`). It ran on the same
302 held-out files and the same host as round 1.

### Verdict

**Correctness passes and every speed bar passes.**
- **Correctness:** the installed command's stdout, exit code and output
  files equal the native workspace build's on all 3,016 cases compared,
  with no trap.
- **Speed:** it takes 0.09 to 0.20 times the FFmpeg command line's time per
  operation, and every class passes.
- **Faster than FFmpeg** (the 95% upper bound below 1.00 overall and in
  every class): probe, chapters,
  audio extraction, both clips, remux, sanitize and split.
- **The check** is faster overall (0.198 [0.179, 0.226]) but not in every
  class. Its Matroska/WebM class has a median of 0.803 and an upper bound
  of 1.621 over 56 files, so it is on par there.
- The check's reference, plain `ffprobe -show_format -show_streams` as
  fixed in round 1's bars, does less work than the check, which scans every
  video sample's headers for `web-upload.json`'s resolution limits. Against
  `ffprobe -count_packets`, which also reads every packet, the check would
  only compare better.

| Operation | Files | `coreav` | FFmpeg | `coreav` / FFmpeg, median [95% CI] | Installed / native build | With `npx` |
|---|---:|---:|---:|---|---:|---:|
| Probe | 299 | 0.8 ms | 6.8 ms | **0.120** [0.112, 0.129] | 0.90 | 32.3 |
| Check (web-upload) | 302 | 1.4 ms | 6.7 ms | **0.198** [0.179, 0.226] | 1.06 | 32.8 |
| Chapters | 243 | 0.7 ms | 6.9 ms | **0.092** [0.087, 0.097] | 0.82 | 31.6 |
| Audio extraction | 126 | 1.6 ms | 18.4 ms | **0.124** [0.119, 0.130] | 1.02 | 12.0 |
| Head clip | 188 | 1.7 ms | 11.1 ms | **0.160** [0.154, 0.170] | 1.02 | 19.9 |
| Middle clip | 188 | 1.9 ms | 11.7 ms | **0.165** [0.159, 0.182] | 1.03 | 18.9 |
| Remux | 261 | 3.9 ms | 24.0 ms | **0.166** [0.157, 0.176] | 1.05 | 9.2 |
| Sanitize | 223 | 3.5 ms | 20.4 ms | **0.165** [0.157, 0.178] | 1.06 | 10.9 |
| Split | 188 | 3.1 ms | 17.5 ms | **0.170** [0.160, 0.183] | 1.06 | 12.7 |
| Subtitle extraction | 1 | 2.4 ms | 37.1 ms | 0.07, not judged (fewer than 10 files) | 1.49 | 6.0 |

Per class: median [95% CI upper bound] (files).

| Operation | ISO base media | Matroska/WebM | Under 16 MiB | 16 MiB or more |
|---|---|---|---|---|
| Probe | 0.114 [0.124] (243) | 0.237 [0.250] (56) | 0.117 [0.126] (213) | 0.138 [0.166] (86) |
| Check (web-upload) | 0.177 [0.194] (246) | 0.803 [1.621] (56) | 0.178 [0.194] (214) | 0.412 [0.785] (88) |
| Chapters | 0.092 [0.097] (243) | — | 0.093 [0.098] (177) | 0.086 [0.115] (66) |
| Audio extraction | 0.124 [0.130] (126) | — | 0.123 [0.129] (93) | 0.139 [0.170] (33) |
| Head clip | 0.160 [0.170] (188) | — | 0.156 [0.162] (150) | 0.225 [0.364] (38) |
| Middle clip | 0.165 [0.182] (188) | — | 0.158 [0.164] (150) | 0.377 [0.464] (38) |
| Remux | 0.162 [0.172] (223) | 0.208 [0.357] (38) | 0.155 [0.162] (187) | 0.277 [0.363] (74) |
| Sanitize | 0.165 [0.178] (223) | — | 0.156 [0.164] (165) | 0.239 [0.290] (58) |
| Split | 0.170 [0.183] (188) | — | 0.159 [0.169] (150) | 0.371 [0.577] (38) |


Launch costs, the median of 20 runs of `--version` (`meta.json`):

| Way to start the command | Time |
|---|---:|
| The installed native binary (Linux and macOS after install) | 0.7 ms |
| The Node launcher, then the binary (Windows; installs without scripts) | 28.6 ms |
| The WASI build under Node (platforms without a package) | 61.2 ms |
| `npx coreav`, the package installed | 219.4 ms |

`npm exec` adds 219 ms per call before the command starts: npm's own
start-up, the same for any package. With it, a call through `npx` takes 6 to
33 times FFmpeg's time on these files (column "With `npx`"). Installed, the
static musl binary of the package runs at 0.82 to 1.06 times the time of
the native glibc build of the workspace on the judged operations (column "Installed / native
build").

### Final build: spot check

Between round 2 and merging, the branch was rebased onto `main`, which
added the command's HTTPS URL reader (the native binaries now
include TLS) and a faster copy window. The final build ran the
first 30 held-out files again:
- every case is equal to the native build's (299 cases);
- launching took 0.7 ms installed, 26.5 ms through the Node launcher,
  57.1 ms for the WASI build and 209 ms through `npx`;
- the median ratios to FFmpeg are within about 10% of round 2's on the
  same files (probe 0.135 against 0.127, check 0.223 against 0.206, remux
  0.165 against 0.162, split 0.178 against 0.175).

Another measurement was running on the host at the same time. Evidence:
[`npx-coreav/final-spot-check/`](npx-coreav/final-spot-check/).

## Round 1: the WASI build as the command

### Verdict

**Correctness passes and every speed bar misses.** The launcher's stdout,
exit code and output files equal the native command's on all 3,016 cases
compared (4 middle clips of files without a duration were not run), with no
trap. Against the FFmpeg command line doing the same job, the
installed command (`coreav` from `npm install`) takes 3.9 to 9.9 times as
long, and `npx coreav` 14 to 39 times, while the native command takes 0.13
to 0.21 times FFmpeg's time on the same files. The cost is mostly fixed per call:
the launcher spent 63 to 82 ms per call at the median, the native command 1
to 4 ms, and `--version` alone took 60 ms through the launcher. Starting
Node takes about 25 ms of that (measured during development); the rest is
loading, compiling and running the module. On these files, FFmpeg finishes
the whole job in 7 to 24 ms.

### Results

Times are each file's median of 5 rounds, as medians over files. Ratios
are medians over files of the per-file ratio, with bootstrap 95% intervals
(10,000 resamples, seed 20261018). "With `npx`" adds `npm exec`'s own
cost to the launcher's: 213 ms, the median of 20 runs of `npx coreav
--version` minus the installed command's (60 ms).

| Operation | Files | Launcher | Native | FFmpeg | Launcher / FFmpeg, median [95% CI] | Native / FFmpeg | With `npx` |
|---|---:|---:|---:|---:|---|---:|---:|
| Probe | 299 | 66 ms | 1.0 ms | 7.4 ms | **9.10** [8.41, 9.70] | 0.14 | 37.6 |
| Check (web-upload) | 302 | 72 ms | 1.4 ms | 7.3 ms | **9.95** [9.28, 10.80] | 0.21 | 39.0 |
| Chapters | 243 | 63 ms | 1.0 ms | 7.8 ms | **8.39** [7.82, 9.00] | 0.13 | 35.4 |
| Audio extraction | 126 | 71 ms | 1.8 ms | 19.2 ms | **3.87** [2.97, 4.99] | 0.13 | 14.9 |
| Head clip | 188 | 71 ms | 1.8 ms | 11.9 ms | **6.20** [5.53, 6.80] | 0.16 | 24.1 |
| Middle clip | 188 | 73 ms | 2.2 ms | 12.4 ms | **6.17** [5.70, 6.86] | 0.18 | 23.3 |
| Remux | 261 | 82 ms | 3.6 ms | 23.7 ms | **4.00** [3.51, 4.67] | 0.16 | 13.7 |
| Sanitize | 223 | 81 ms | 3.3 ms | 20.5 ms | **4.12** [3.67, 4.88] | 0.16 | 14.3 |
| Split | 188 | 78 ms | 3.0 ms | 17.9 ms | **6.12** [5.34, 6.96] | 0.16 | 18.7 |
| Subtitle extraction | 1 | 73 ms | 1.8 ms | 38.1 ms | 1.90, not judged (fewer than 10 files) | 0.05 | 7.5 |

| Operation | ISO base media | Matroska/WebM | Under 16 MiB | 16 MiB or more |
|---|---|---|---|---|
| Probe | 8.87 (243) | 18.56 (56) | 9.20 (213) | 8.56 (86) |
| Check (web-upload) | 9.51 (246) | 27.69 (56) | 9.77 (214) | 10.26 (88) |
| Chapters | 8.39 (243) | — | 8.55 (177) | 7.60 (66) |
| Audio extraction | 3.87 (126) | — | 5.11 (93) | 1.29 (33) |
| Head clip | 6.20 (188) | — | 6.53 (150) | 4.47 (38) |
| Middle clip | 6.17 (188) | — | 6.40 (150) | 5.22 (38) |
| Remux | 4.05 (223) | 3.91 (38) | 5.55 (187) | 1.72 (74) |
| Sanitize | 4.12 (223) | — | 5.32 (165) | 1.72 (58) |
| Split | 6.12 (188) | — | 6.43 (150) | 3.00 (38) |

Per class (median launcher / FFmpeg, files in parentheses). Every class
misses. For the operations that copy bytes (extraction, clips, remux,
sanitize, split), the ratio falls as the files grow, because FFmpeg's time
grows with the bytes copied and the launcher's fixed cost does not; probe,
check and chapters read about as much of a large file as of a small one.

Counted apart: files the native command refuses, by operation. These are
probe 3, chapters 59 (Matroska and WebM have no QuickTime chapters),
extraction 176 (no AAC track), clips and split 114 or 110, remux 41,
sanitize 79 and subtitles 301 (no `mov_text` track). There were also 4
middle clips of files without a duration. No FFmpeg run failed on a case
CoreAV accepts, and no round failed.

### Load during the run

For part of the run, other builds and tests ran on the same host (with
lower priority). The three commands alternate within each round, so they
share any load. As a check, the first 30 files were run again on a quiet
host (`npx-coreav/round1/quiet-check/`). The ratios there agree with the full
run's on the same files within 8%: probe 9.03 against 9.77, remux 3.74
against 3.85, split 6.32 against 6.08, head clip 7.07 against 7.16.

### What it means

No launcher that runs under Node can meet a per-call bar of 1.00 against
FFmpeg's command line on these files. Node's start-up alone (about 25 ms)
exceeds FFmpeg's whole run on most of them, and `npx` adds about 210 ms
before any launcher starts. The native command is 5 to 8 times faster than
FFmpeg on the same files. Two options would bring an installed npm
`coreav` close to it:
- native binaries in per-platform packages, linked as the `coreav` command
  at install;
- a pointer to the native command.

Neither changes what `npx` itself costs.

## Evidence and reproduction

Each round's directory under [`npx-coreav/`](npx-coreav/) holds:
- `meta.json`: the build's SHA-256s, tool versions and the launch timings;
- `results.jsonl.gz`: every case;
- `summary.json`.

Round 1 also has its quiet check.

    packages/cli/scripts/build.sh
    packages/cli/scripts/platform.sh linux-x64
    cargo build --locked --profile edge -p coreav-cli
    COREAV_BIN=target/edge/coreav python3 -I bench/npx/run.py INPUTS.json OUT [--path wasi]   # INPUTS.json: the private per-input list
    python3 -I bench/npx/summarize.py OUT

Round 1 ran the WASI build as the command (`--path wasi`, then the
package's only form). Inputs are downloaded once from the corpus bucket
with the environment's R2 credentials and verified by SHA-256. The runs
cost nothing beyond R2's free tier.

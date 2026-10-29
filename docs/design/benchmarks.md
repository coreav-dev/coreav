# Benchmarks: speed bars and the CI regression guard

Status: Accepted. Scope: every user-facing operation has a speed bar, and
CI runs a small, stable benchmark subset on main and fails on a significant
slowdown against a stored baseline.

## Speed bars

Each operation's bar against FFmpeg is in its design note, fixed before its
run: Matroska probe (`docs/design/mkv.md`), Matroska and WebM remux
(`docs/design/mkv-remux.md`), subtitles and chapters (`docs/design/text.md`),
and writing the output to `-` against FFmpeg's `pipe:1`
(`docs/design/cli-stdout.md`).
The Python binding's own-overhead bar is in `docs/design/python.md`. They
are measured by `docs/spikes/parity-bench/`; results are in its README.
Thumbnails' bars (`docs/design/thumbnails.md`, against the `ffmpeg` command
line) are measured by `docs/spikes/thumbnail-bench/` on the sandboxed FFmpeg
fallback, and AV1 through dav1d where the row says so. The table gives the median per-file ratio, the share of
held-out files where CoreAV is faster, and the result; it says "missed"
for the classes that are over 1.00.

| Thumbnails | Median ratio against `ffmpeg` (held-out) | Faster | Bar |
|---|---|---|---|
| Keyframe, JPEG (S1, `-ss T`) | 0.425 (95% CI 0.394 to 0.452) | 97% | met |
| Keyframe, same frame (S2, `-ss K`) | 0.655 (0.600 to 0.737) | 89% | met |
| `--exact` (S3), first run | 1.018 (0.993 to 1.039) | 46% | missed: overall, H.264, AV1, VP8 |
| `--exact` (S3), round 2 after a fix, same held-out partition (a development measurement; no "faster" claim until a fresh draw) | 0.854 (0.833 to 0.888) | 72% | met overall, H.264, HEVC, ProRes; missed AV1, VP8, VP9 (1.010, within ±5% run-to-run noise) |
| `--exact` (S3), AV1 through dav1d, the 26 held-out AV1 files (timed in both rounds before, so a development measurement) | 0.934 (0.778 to 1.030) | 62% | met, close (fallback round 2: 1.186) |
| PNG (S4) | 0.834 (0.788 to 0.942) | 61% | met overall; missed for VP8, and for AV1 through dav1d on the development set (1.082). At deflate level 3, development set: VP8 0.899, AV1 through dav1d 0.895, AV1 through the fallback 1.002 (missed, within noise) |
| WebP (S5) | 0.196 (0.184 to 0.212) | 100% | met |
| Joint confirmation on held-out draw 4 (52 files, the one refused counted as slower) | S1 0.399, S2 0.587, S3 0.740, S4 0.668, S5 0.184; every CI upper bound below 1.00 | 98%, 96%, 79%, 92%, 98% | met overall and for H.264; VP8's `--exact` 1.411 (7 files, not judged) |

| Operation | Median ratio against libavformat | Bar |
|---|---|---|
| Matroska and WebM probe | 0.227 (95% CI 0.218 to 0.233) | met |
| Matroska and WebM remux | 0.448 (95% CI 0.412 to 0.515) | met |
| Subtitles to WebVTT | 0.030 held-out; 0.040 on all `real` | met |
| Chapters | 0.006 held-out; 0.005 on all `real` | met |

The binding's bars: `probe` met (+12.7 µs); writing to a path met for
`extract_audio` and `remux`, missed for `clip`; in memory missed
(`docs/design/python.md`).

### Library probe and index against libavformat (fixed before the run)

The library's probe with the full packet index against libavformat opening
the same file in process was last measured on stage 2 code (W1 run-05:
libavformat's time over CoreAV's, 2.33) and, after an earlier change, on a
set drawn with an earlier change's salt. It is re-run on current `main` with the method
of `docs/spikes/large-tables/` (engines A-index, `probe-bench --index
--io=mmap`, and B-lean, `probe-libav --format-mov --lean --io=mmap
--buffer=4096`; long-lived processes pinned to their own cores, one
warm-up pass and 15 rounds, batches of at most 12 GB) on the 303 held-out
files of `docs/spikes/safe-probe/corpus/holdout-v1.json`: round 2 of W1 on this
set (run-05 passed on its first round; the set was never used for tuning).
The unit is CoreAV's time over libavformat's. Headline set as W1: both open
the file, CoreAV drops no stream, and its index has as many entries as
`ffprobe -count_packets` lists packets. Bar: the median ratio
is at most **1.00** overall and in each corpus category; "faster" only
where the upper bound of the bootstrap 95% interval is below 1.00.

(The engines ran pinned to one shared core, one request at a time, rather
than each to its own core as written above; requests never overlap, so the
timings are the same.)
**Result: met** (`docs/spikes/w1-227/`, run `w1-01` on the crates of `1114aa6`, round 2): CoreAV
takes 0.316 (95% CI 0.294 to 0.343) of libavformat's time over 232 files.
Every category is at most 0.553, every upper bound below 1.00, so CoreAV is
faster overall and in each category. One audio-only file is at 1.01.

## CI regression guard

The guard is in `.github/workflows/bench-guard.yml`, run by
`scripts/bench_guard.py`, as one job of a few minutes:

- on pushes to `main` that change the crates, and on manual runs, against
  the stored baseline;
- on ready pull requests (not on drafts) that change the crates,
  `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, the guard's script,
  `bench/guard/**` or the workflow: the pull request's merge commit against
  its first parent (the base it merges into), both built and counted in the
  same job on the same fixtures, so that a regression shows before it
  merges. Comparing against the base commit rather than the stored
  baseline judges only what the pull request changes, whatever state
  `main` is in. The base is built in its own target directory, seeded with
  the restored dependencies (a shared one would let cargo take the base's
  build for the head's: their crates have the same relative paths), and
  the job fails if two builds of different code give the same binary,
  unless cargo compiled the head's binary from the head's sources (a
  change that does not reach the native binary, such as code for another
  target or feature). A
  case the base commit cannot run (a new command) is shown without a
  comparison.

- **Cases that need a decoder** (`thumbnail`, `transcode`): the shipped build has no
  decoder backend, so these run on a second `dist` build with the FFmpeg
  fallback (`--decoder-coreav`), counted beyond that build's own start-up.
  Start-up and every other case stay on the shipped build. Rejected:
  counting everything on the fallback build, which would guard a binary
  that is not shipped, and put the fallback's linked code into every
  case's start-up. `transcode` (`docs/design/transcode.md`, bar B8) runs
  `coreav transcode` on `basic.mp4` with `--encoder ffmpeg --preset
  ultrafast --threads 1 --scale 160:-2` (the resize, `Picture::resize_with`,
  counted too): the encoder is the pinned `ffmpeg` on `PATH`,
  another process like the worker and its FFmpeg, so the count is the
  `coreav` process's own work (demuxing, frames copied out of the worker,
  writes to the encoder, the Annex B reader, the MP4 writer); one encoder
  thread keeps the encoded stream, and so the reader's work, the same on
  every runner. It joined the guard without a baseline entry: a case
  without one is reported, not judged, until a guard run on `main` records
  its count.
- **What it measures:** the number of instructions the `coreav` command
  executes (Valgrind's `cachegrind` tool with the cache simulation off) for
  a fixed list of cases, each user-facing command on one fixture, minus the
  instructions of `coreav --version` counted in the same run. Start-up
  (dynamic loading, argument parsing) is about 680,000 instructions on the
  runners in the `dist` build (550,000 when the guard was set up; the
  argument parser builds every subcommand on each run, so it grows with the
  commands), more than most cases' own work; subtracting it keeps the cases
  sensitive to the work and cancels start-up changes from a new C library on
  the runner. Start-up is guarded as a case of its own, with the same
  tolerance, since a regression there slows every command. Part of it (about
  an eighth) is the dynamic loader applying the binary's relocations, which
  grow with the code linked in: linking HTTP and TLS for URL inputs
  took a release build from 2,731 to 6,615 relocations, and relocations were
  most of the 10% that start-up grew. The guard therefore counts the `dist`
  profile, the build that is shipped (wheels, `cargo install --profile
  dist`): fat LTO with one codegen unit drops the linked crates' unused code
  and brings the relocations back to about 3,100. Rejected: fat LTO on
  `release`, which would slow every `cargo test --release` in CI; packed
  relative relocations (`DT_RELR`), which need glibc 2.36 at run time; a
  non-PIE binary, which loses address-space randomization. The stored
  baseline holds `dist` counts, recorded by a `workflow_dispatch` guard run
  with `update: true` (run 40828255844) on the change that stopped parsing
  a transport stream's PES headers twice (#32). The baseline before it
  predated the public release and had drifted from `main` by up to 4.75%
  up (start-up) and 45% down (`probe-packets-ts`), so `main` failed on
  a case each pull request had passed against its own base. A
  start-up failure on a change that does not touch the binary's dependencies
  or `main` points at the runner image (a new C library): the baseline needs
  updating, not the code.
- **Pinned tools:** the job builds with a pinned Rust release (overriding
  `rust-toolchain.toml`'s `stable`), so counts move with the code and not
  with each compiler release; the baseline records the `rustc` and
  Valgrind versions, and the guard reports a difference. Moving to a new
  compiler is a pull request that changes the pin and the baseline
  together.
  Instruction counts do not depend on the runner's load, clock or
  neighbours, so a small subset gives a stable signal on shared CI machines,
  where wall times vary by tens of percent.
- **Inputs:** the fixtures of `scripts/gen-fixtures.sh`, which the pinned
  FFmpeg writes byte for byte the same on every run, and others the guard
  writes from them with `-fflags +bitexact`: by stream copy, a Matroska
  copy of `basic.mp4` and a WebM copy of `vp9.mp4`; with the audio of
  `basic.mp4` encoded to FLAC and to MP3 (`libmp3lame`, 128 kbit/s), a
  Matroska and an MP4 file each for the `decode-audio-flac-*` and
  `decode-audio-mp3-*` cases (`docs/design/audio-decode.md`); the Matroska
  ones are also resampled and downmixed (`decode-audio-16k-mono`,
  `decode-audio-resample`). The fixtures of
  `scripts/gen-fixtures-mkv.sh` carry random segment IDs, so their bytes
  differ from run to run. Outputs go to standard output and are discarded,
  so no output path (with its process ID) enters the counts. The two
  commands that write files, `split` (segment names with `%d`) and `hls
  package` (a directory), write into a new directory per run under the
  guard's work directory, whose name has a fixed length; their counts are
  the same from run to run. `join` reads inputs the guard writes first from the
  case's fixture with the same binary (`split` and `clip`, not counted),
  into such a directory as well. Two transport streams are crafted rather
  than written by FFmpeg, because neither structure is one a muxer
  writes: `scripts/ts_craft.py` takes a generated stream of sound and
  replaces its PES payloads with bytes that are no frame header
  (`scramble`), and a generated stream of pictures and makes every PES
  header state the time the first states (`freeze`). Both are
  deterministic, and the guard fails a case whose crafted fixture could
  not be written rather than skipping it.
- **HLS** (`docs/design/hls.md`): `hls-index` builds the index of
  `hls_h264.mp4` (20 s, four segments per rendition); `hls-segment` cuts its second video
  segment with `hls get`, the per-request path, from an index the guard
  builds first with the same binary, uncounted; `hls-package` writes the
  whole package of the same file.
- **Cases per command**: `probe` on MP4,
  fragmented MP4, Matroska, WebM and a transport stream,
  `--ffprobe-compat` on fragmented MP4
  (it reads every fragment), Matroska and a transport stream, and
  `--scan-frame-sizes` on MP4
  and Matroska (every frame header; the block scan) and on a transport
  stream (`probe-packets-ts`: every access unit of the file,
  `docs/design/mpegts.md`); two transport streams that are not what
  their tables say they are, which is where a reader's searching and
  waiting costs most: `probe-ts-no-frames`, whose sound is
  payload no frame header reads, so the probe searches every payload
  whole, and `probe-packets-ts-no-period`, whose pictures all state one
  time, so the listing holds every access unit of them waiting for a
  picture period; `check` on MP4 and
  Matroska (the structure audit and the block scan); `remux` on MOV,
  fragmented MP4, Matroska and WebM; `clip` and `split`, each with one edit
  and with several; `join` of segments, of fragmented segments and of two clips
  (`docs/design/join.md`, "Speed guard in CI"); `extract-audio`;
  `sanitize`; `extract-subtitles`; `chapters`; and `hls index`, `hls get`
  and `hls package`, and the URL cases below.
  Not yet guarded: the `mcp` server's own request handling.
- **URL cases**: every command but `hls` reads HTTP(S) URLs by byte
  ranges (`docs/design/remote.md`), and a change to the read plan
  (`crates/coreav-range`) that adds round trips or bytes costs time over a
  network but few instructions. So the guard also serves its fixtures from
  a local HTTP server that answers range requests (`RangeServer`) and runs
  `probe` (movie box at the end and at the front, and a Matroska file),
  `extract-audio`, `clip`, `remux`, `sanitize`, `split`, `check`,
  `extract-subtitles` and `chapters` on their URLs. Per case it counts:
  - **requests**, as the server receives them (those of the HTTP host
    included);
  - **rounds**: the server holds each response's head back 100 ms
    (`RESPONSE_DELAY`), and a round starts with a request that arrives
    while no other head is pending. Requests the plan starts together (the
    tail with the first request, a pipeline's first segments) form one
    round. A request that has to wait for an earlier response starts
    another, as on a network, where it costs a round trip. A plan that
    makes the same requests one after another (no tail request, so a probe
    asks for the movie box after the first response; one segment in
    flight) adds rounds without adding requests;
  - **bytes received** by the readers (`COREAV_REMOTE_STATS`).

  A case fails with one request or one round more, or 2% more bytes
  (`URL_BYTES_TOLERANCE`), than the baseline's, except that `url-extract-audio`'s
  rounds may exceed it by 4 (`URL_ROUNDS_SLACK`: its rounds varied from 2 to 5 over
  14 runs of one build, with requests and bytes unchanged). Fewer never fails: a
  change that saves requests is reported, so that the baseline can be
  lowered. When the fixture's bytes differ from the baseline's (the pinned
  FFmpeg writes some fixtures differently on another CPU, which can move a
  boundary), one more request or round and 10% of bytes
  (`URL_BYTES_TOLERANCE_OTHER`) are allowed. On a pull request, base and
  head read the same fixtures, so the strict limits apply.
  - The requests do not depend on timing: `begin`, `fill`, `advance` and
    `seek` depend only on the reader's positions.
  - Bytes move only with how far a pipeline reads past the last read.
  - Rounds could move with the runner's scheduling if a burst of requests
    were split, which the 100 ms hold makes unlikely; the guard waits for
    the server to finish every request before reading the counts.
  - Runner runs agreed on requests in every case and on bytes within
    0.12% (guard runs 39375976401, 39376413253 and 39376420746, commit
    `b112824`). Three runs with the round count (39378414615, 39378421470
    and 39378429353) counted the same requests and the same rounds in
    every case: one for each probe, two or three for the copy passes,
    nine for `split`. The baseline's URL entries come from the first.
  - **Fixtures:** the existing fixtures are smaller than the plan's first
    response (32 MiB), so they exercise one request. The guard therefore
    encodes two of its own with the pinned FFmpeg, single-threaded and
    bitexact so that every run writes the same bytes: 48 s of 720p H.264
    at about 13 Mbit/s with AAC audio in chunks of 2 s, 79 MiB, its audio
    chunks about 3 MB apart, once with the movie box at the end and once at
    the front. They take about two seconds to encode, and one that cannot
    be encoded fails its cases with FFmpeg's message. On them a probe reads
    the start and the tail, and the copy passes run the plan's pipelines.
  - Rejected: **instruction counts of URL cases.** The reader's threads and
    the server's scheduling move them, and the cost they miss is round
    trips, not instructions.
  - Rejected: **wall time against a local server.** Its only round trip is
    the delay the guard adds, CPU time dominates it, and CI runners' wall
    times vary by tens of percent; the overlap count carries what wall time
    would show of serial requests.
- **Baseline:** `bench/guard/baseline.json` holds, per case, the fixture's
  SHA-256 and the count, recorded by the guard itself in CI (the same
  runner image, Valgrind and toolchain). A case whose fixture's bytes
  differ from the baseline's is still compared, and the difference is
  reported: the pinned FFmpeg writes most MP4 fixtures differently on
  different runner CPUs (the three noise runs below saw two versions of
  10 of the 11 fixtures), and the counts barely move with them. A case
  whose fixture is missing fails if the baseline has it, as does a command
  that fails: a case that does not run would hide its regressions.
- **Tolerance** (fixed before the noise runs): three guard runs of the same
  commit on fresh runners; the spread of a case is its largest count over
  its smallest, minus one. The tolerance is the larger of **5%** and three
  times the largest spread of any case. 5% is the floor because the guard
  is for significant slowdowns: a change that adds a
  field to a document or a check to a parser may add a few percent to a
  case's small workload, which is not a regression to stop `main` for. A
  case fails when its count exceeds the baseline by more than the
  tolerance; counts below the baseline by more than the tolerance are
  reported, so the baseline can be lowered.
- **Noise runs:** three runs of commit `23756d1` on fresh runners. Start-up
  was 549,929 instructions in each. 10 of the 14 cases counted the same in
  all three; `remux-mov` (0.30%), `sanitize` (0.28%) and `remux-mkv`
  (0.25%) moved with the fixture's bytes, `clip` by 3 instructions. Three
  times the largest spread is 0.9%, so the tolerance is the floor, **5%**.
  On the development machine (another CPU and C library; start-up 588,383)
  the same code counted within 2% of the runners' in every case, and the
  same in two runs; the baseline is still recorded on the runners.
- **What it does not catch:** the fixtures are small -- 2 seconds, and
  every case but three under 8 million instructions -- so a cost that
  grows with the file's size beyond what these files show, or a
  regression in I/O or memory rather than instructions, passes. The
  three are `thumbnail` at 10 million, and the two transport streams
  that are not what they say they are, at 41 and 66 million over 40
  seconds of media: those two exist because the costs they hold grow
  with the file, and a 2-second fixture would not have shown either. The speed bars against FFmpeg
  (`docs/spikes/parity-bench/`) measure those on real files. The URL cases count
  requests, rounds and bytes, not time: the round trip a request costs is
  measured against FFmpeg reading the same URL (`docs/design/remote.md`).
- **Updating the baseline:** a pull request may move
  `bench/guard/baseline.json` only for a stated, unavoidable cost (work a
  feature or a fix needs, a measured trade-off, a toolchain or runner
  update), never silently: its description names each case that moves, by
  how much, and why the cost cannot be avoided. A faster change lowers the
  baseline the same way. The counts come from a guard run
  (`workflow_dispatch` with `update: true` keeps them as an artifact). On
  pull requests the guard flags a changed baseline file in its summary and
  as a warning. Rejected: letting a pull request raise the baseline in the
  same change that slows the code without a statement, which is how a
  regression would pass unnoticed.

### What moved the counts after the split

The guard failed from the split's merge (`305135a`) on: start-up +4.1%,
`clip` +7.0%, `sanitize` +5.3%, while `extract-audio` fell by 4.4%. Per
function (cachegrind on the development machine, the same commits and
fixtures):

- **Start-up** grows with every subcommand. The command's argument parser
  (clap's derive API) builds the argument table of every subcommand before
  it parses anything, `coreav --version` included: each subcommand's
  arguments, value parsers and help strings are allocated and copied. The
  `split` subcommand added 24,200 instructions on the runners (nine
  arguments), the `mcp` server 49,900, and `hls` about
  61,000. This is the cost of offering the subcommand, paid once per
  process (some tens of thousands of instructions, a few microseconds);
  the baseline's start-up is raised by what the merged subcommands add
  (688,257 on the runners with `split`, `mcp` and `hls`, against 549,929
  before them). A pull request that adds a subcommand raises it by that
  subcommand's share and says so. Building only the chosen
  subcommand's table would remove most of it, with a hand-written first
  dispatch in front of clap; it is not done here, as it changes how every
  command's arguments are parsed.
- **`clip` and `sanitize`** did not do more work. Their whole difference
  was `memset` (+45,000 instructions each): the writers' copy window
  (64 KiB) was allocated zeroed, and whether the allocator had to clear it
  depended on what the process had allocated and freed before (memory
  fresh from the system comes cleared; reused memory is cleared by
  `memset`, whose byte loop Valgrind counts nearly byte by byte). The
  split's extra subcommand changed the heap before the window was
  allocated, so `clip` and `sanitize` began clearing it and `extract-audio`
  stopped. The window is now reserved, not zeroed: a refill reads over the
  bytes the window already holds, in place, and appends only what it lacks
  (`Source::read_append`, which a file, bytes in memory and a buffered
  source answer by reading into the spare capacity). So only the window's
  first fill appends, which costs a file a seek and two reads instead of
  one positioned read, and every later refill is one read as before. Every
  writer case is then below its count before the split, and none depends
  on the heap's history: on the development machine `clip` 629,724 against
  642,515 before the split, `sanitize` 748,961 against 754,744,
  `extract-audio` 316,151 against 370,211.

### The parsers' cases after the probe fixes and new subcommands

`probe-mp4`, `extract-subtitles`, `chapters` and `hls-index` then failed on
`main` (+6.9%, +6.2%, +5.7% and +11.9% on the runners). Counted per commit
on the development machine from the guard's own baseline commit (`662c08b`)
to `871ac99`, `probe-mp4` rose 15,100 instructions (+3.1% there). An earlier change
added 60, change C 2,300, change D 2,800 and change E 2,400, while changes A and B each
took about as much back. The steps that remained came with new
subcommands: change A added 4,600 and change B 9,700.

Per function, none of it was parsing. `malloc_consolidate` and
`unlink_chunk` grew by 10,000. The rest was building and writing the JSON
document: the compiler inlined serde_json's code differently from build to
build, which moves thousands of instructions between functions and nets
out close to zero, and an earlier change's new key (`implicit_ps_possible`) added a few
hundred. The argument parser allocates its whole command tree, thousands
of small blocks, and freed it before the command ran. The first large
allocation the command then made led the allocator to merge every one of
those freed blocks first. That cost grows with the number of subcommands,
and the subtracted `--version` run never pays it, because it makes no
large allocation after the free.

The command now keeps the parser's tree and matches allocated until it has
run, and frees them after. The `--version` run frees them too, so the
subtraction still cancels that.

| Case (development machine) | `662c08b` | `871ac99` | now |
|---|---|---|---|
| `probe-mp4` | 493,466 | 508,578 | 493,563 |
| `extract-subtitles` | 197,530 | 208,489 | 194,019 |
| `chapters` | 272,123 | 285,422 | 271,494 |
| `hls-index` | – | 1,717,957 | 1,685,319 |

On the development machine every case is now at or below its count at
`662c08b`, except three. `probe-fragmented` is +0.4%, `probe-mp4` +97
instructions and `remux-fragmented` +1,289 (+0.02%); that is the
output's new keys, kept as the cost of the fields.

On the runners `probe-mp4` is +4.2% over the stored baseline (from
+6.8%), `extract-subtitles` −1.5% (from +5.5%) and `chapters` +0.4% (from
+5.7%). Their C library merges more of the freed blocks than the
development machine's, so less of the drop shows there.

`hls-index` falls by 1.9% on the runners as here, but stays 8% over its
stored baseline. That baseline (1,564,443) was recorded on an intermediate
commit of change A: at change A's own merge the guard counted 1,751,081 against it,
so no merged code ever met it. The work the later an earlier change commits added is the
feature's own, so this change raises the `hls-index` entry to the guard
run's count with this change, on the same fixture bytes. It leaves
start-up, which an earlier change's dependencies moved, to its own change.

Rejected:
- Raising the baselines: the counts would still follow heap history, and
  the next subcommand would move them again.
- Leaking the parser's tree (never freeing it): it gives the same counts
  as freeing it after the run, and makes the Python entry point keep
  memory it no longer needs.

### The probe document's nested values

The guard flagged `probe-fragmented` (+5.3%) and `probe-mp4` (+3.7%) on
`main` after an earlier change added `video.parameter_set_color`. Counted with
cachegrind on the development machine, with the parameter set's colour read
or not in the same build, an earlier change cost 13,300 instructions on `basic.mp4`
and 13,600 on `fragmented_many.mp4`. About 2,000 of that was reading the
sequence parameter set, once per sample entry (not once per fragment, and
not again for a parameter set already read). The rest was the JSON
document. `serde_json::json!` converts each value expression with
`to_value(&value)`, which copies a nested `Value` (and the original is
then dropped). The probe document nests the new object four deep
(document, stream list, stream, `video`), so it was built once and copied
three times, and the same happened to every other nested value of the
document.

The probe document is now built with a small macro, `object!` in
`crates/coreav-cli/src/native.rs`, that moves each value into the map
(`Value::from`) instead of copying it. Its keys come out sorted as before
(serde_json's `Map` is a `BTreeMap` without the `preserve_order`
feature), and the output is byte-identical: 272 runs on the fixtures and
900 on the 225 MP4 and MOV files of the local corpus (`probe`,
`--scan-fragments`, `--ffprobe-compat` and `--scan-frame-sizes`, with
stdout, stderr and the exit status compared), and `check` on every
fixture.

| Case (development machine) | before | after |
|---|---:|---:|
| `probe-mp4` | 472,858 | 286,499 |
| `probe-fragmented` | 357,685 | 227,082 |
| `check` | 791,817 | 609,647 |

Every other case moves by less than 3%. `parameter_set_color` now costs
6,800 instructions on `basic.mp4` and 7,500 on `fragmented_many.mp4`,
the parse and the five keys it writes; that is the feature's own cost,
and it stays.

On the runners (guard run 39290537345, `update: true`, on this change)
`probe-mp4` counts 288,925 (−37.1% against its baseline), `probe-fragmented`
228,548 (−33.2%) and `check` 598,768 (−21.7%); every other case is within
2% of its baseline. Those three entries are lowered to these counts (with
the run's fixture hashes); the others stay.

Rejected:
- Raising the two probe baselines by an earlier change's cost: most of it was the
  copies, which are avoidable.
- Reading the sequence parameter set once for both the size and the
  colour (the size reader parses it too, about 2,000 instructions per
  video stream on these files): it couples two readers for under 1% of a
  probe.
- Writing the document with `Serialize` structs instead of `Value`: it
  would avoid the maps entirely, but rewrites every key of the schema for
  what is now a small remainder.
- Changing the other documents in the same change (the Matroska probe,
  the `ffprobe` shape, the writers' summaries): they still use `json!`
  and are within their baselines; each can take `object!` when it is next
  changed.

### Start-up after `decode-audio` and thumbnails

The guard failed on `main` from change A on. On the runners, start-up rose from 691,828 at `fff1725` to 712,382 at change A (`decode-audio`, +20.5k) and 717,829 at change B (thumbnails, +5.4k); change C added 0.2k. Change A's share is its workstream's.

Of an earlier change's +5.4k:
- **The worker check.** Every command's start-up read its arguments for the decode-worker check, even in a default build, which has no worker. It now reads them only in builds with `ffmpeg-fallback`. That removes work: 220 to 720 instructions on the development machine. The runners do not show it, since an earlier change's own guard run counted +1,436 against its base, inside the ±1–2k that unrelated changes move start-up there.
- **A layout effect.** An earlier change added 56 bytes of relocated data (`.data.rel.ro`). That moved the end of the RELRO segment, so the separate `.data` and `.bss` segment moved along and `.bss` spilled into a page of its own: one more mapping. At start-up, Rust's standard library sets up the main thread's stack guard through glibc's `pthread_getattr_np`, which reads `/proc/self/maps` line by line, about 2,000 instructions a line on the development machine. Later commits moved the boundary back (30 mappings again from `63812e4`).

Together these explain about 2.9k of the 5.4k. The rest is runner-only (another C library) and not reproduced on the development machine.

So a start-up step of a few thousand instructions can come from a change that touches neither dependencies nor `main`. Each mapping costs a line of `/proc/self/maps` parsing, and `pthread_getattr_np` is about 12% of start-up (about 90,000 instructions), a larger target than either step. Rejected: padding or reordering data to pin the boundary, which any later change undoes.

### `decode-audio`'s start-up

`decode-audio` raised start-up by 20,554 instructions on the runners
(691,828 to 712,382, a bisect of `main` in an earlier change) and did not raise the
baseline as the rule above asks. Per function (cachegrind on the development
machine, `dist` builds of `fff1725`, `7429037` and `main` made for this
note), the cost is clap's tree for the subcommand: its six options with
their value parsers and help strings, allocated and copied at every start
(`coreav --version`: 727,010 before an earlier change, 750,851 on `main` after it).

The subcommand now builds its options only when it runs: its `Args` are a
wrapper whose `augment_args` hands the derived ones to clap's
`Command::defer`, so `coreav --help` lists the subcommand from its name
and summary, and `coreav decode-audio` (and its `--help`) builds the
rest (`Command::defer` needs clap 4.3.3 or later, the workspace's
requirement). Start-up is 735,287 on the development machine, 15,564 below
`main`; what remains of an earlier change's share is the subcommand's own entry. The
baseline does not move. The guard counts each case beyond start-up, so
the `decode-audio-*` cases now include building the subcommand's options
(about 33,000 instructions, 0.3% of them); they have no baseline yet.

Deferring every subcommand followed in an earlier change (below).

### Every subcommand's options when it runs, and totals

Every subcommand now builds its options only when it runs (`Deferred` in
`main.rs`, as `decode-audio`'s in an earlier change; `sanitize`'s adjustments of
remux's options move into its deferred builder). Start-up then holds the
subcommand table alone, and each case counts its own subcommand's
options beyond it: the work moved, and the work per command fell. On the
development machine (`dist` builds, the guard's counting), start-up fell
by 202,117 instructions and every case's total, start-up plus case, by
1.5% to 19.9%; no total rose. The table is in the pull request's description.

The guard now reports each case's total, the whole command with its
start-up, beside its split, and uses it where a split would mislead: a
case above its tolerance passes only as work moved out of start-up, when
start-up fell in the same run and the whole command does no more work
than the reference's start-up plus the case's count. On a pull request
the reference is the base commit counted on the same runner, so this
change passes there as moved work (the runner table is in the pull request's description). On `main` the
reference is the baseline, which moves in the same change by what moved
on the runners: start-up down in proportion to its change (699,011 to
502,707 on the runner, so 677,121 to 486,964: it keeps its +3.2% above the
baseline; moved by the difference alone, its 22,000 instructions of drift
would weigh +4.6% on the smaller count, about 2,000 from the
tolerance), and each case up by its own subcommand's share (6,000 to
44,000; `join-fragmented` 74,000; the decoder build's `thumbnail`
90,000), which leaves every case as close to its baseline as it was. A later shift can still hide a
case's regression up to what start-up lost in the same run, never one
that makes the whole command do more work. Rejected: a total allowed to
grow by the tolerance of the case's count, which charges start-up's own
drift to every small case.
Rejected: re-recording every baseline from one run, which would also
reset the drift each entry carries, unrelated to this change.

### The read window's in-place refill

`BufferedSource` no longer zeroes its window before each refill, and the
frame scans read headers into stack buffers (`docs/design/read-window.md`).
Every case counts at or below `main`'s: a guard run of this change (run
39536062467) against `main`'s guard run at its base (`81f9ee2`, run
39534445494), the same runner image, counts beyond start-up:

| Case | `main` | this change | Change |
|---|---:|---:|---:|
| `remux-fragmented` | 6,699,992 | 4,474,233 | −33.2% |
| `probe-compat-mp4` | 1,631,021 | 1,215,149 | −25.5% |
| `probe-frame-sizes-mp4` | 476,732 | 371,741 | −22.0% |
| `remux-webm` | 461,864 | 363,037 | −21.4% |
| `check` | 610,013 | 505,000 | −17.2% |
| `remux-mkv` | 811,391 | 685,853 | −15.5% |
| `probe-packets-ts-no-period` | 40,641,011 | 34,450,355 | −15.2% |
| `probe-frame-sizes-mkv` | 996,807 | 849,379 | −14.8% |
| `check-mkv` | 1,122,824 | 976,502 | −13.0% |
| `extract-subtitles` | 206,563 | 182,631 | −11.6% |

The other cases fell by 0% to 4.7% (`hls-segment` rose by 46
instructions, 0.02%); start-up is the same in both runs (499,386 and
499,393). The baseline takes this run's counts for the ten cases above,
the ones that fell by more than the tolerance, so that a later change
that gives the saving back fails the guard, with the fixture digests of
that run (four of the old entries held digests of an earlier FFmpeg
build's fixtures, which the guard reported as differing bytes). The
others keep their entries. Rejected, as in an earlier change: re-recording every entry from one run,
which would also reset the drift unrelated to this change (start-up's
+2.6% over its entry is `main`'s, in both runs).

### CodSpeed or a guard of our own

CodSpeed runs benchmarks under the same kind of CPU simulation (instruction
counts through Valgrind), keeps the history and comments on pull requests.
It is free for public open-source repositories and needs a paid plan for a
private one, and the CodSpeed GitHub app needs access to the repository, a
third-party integration for the maintainer to approve. The guard here gives
the same signal with Valgrind directly, costs only Actions minutes, and keeps
its baseline in the repository. Rejected for now: CodSpeed. Reconsider once
the repository is public, when it is free; the cases carry over.

# The transforms, a row to a vector: profile, counts and timing

Evidence behind `docs/design/h264.md`, "Kernel round: the transforms, a
row to a vector".

- `profile/`: the stage-by-stage profile of `main` at 3d4c2e63 against
  FFmpeg 9.0.2 on the eight streams of `docs/measurements/h264-gap-2/`
  (`perf` cpu-clock samples, this VM of the 2.90 GHz class), the perf part
  of that note's `tools/run.sh` with its stage map (`rep.py`):
  `report.txt` (stages and top functions), `stages.json`, `wall.txt`.
  FFmpeg is the release tarball's build of that note, used as a black
  box: only its symbol names are read.
- `instructions.txt`: callgrind counts of the whole decode and of the two
  kernels on four streams, for `main`, Q (the four-block kernel alone)
  and A (both kernels).
- `local-timing.txt`: the local timing taken before the bars were
  written (information).
- `streams-8x8.txt`: which training streams enable the 8x8 transform,
  the streams of bar TR-5.
- `tools/miri`: the Miri harness for the pointer accesses (copy it to a
  crate, replace `KPATH` with the checkout's
  `crates/coreav-kernels-h264`, and run `RUSTFLAGS="-C
  target-feature=+avx2,+sse4.1" cargo +nightly miri run`).
- `analyze.py`: the summary of the gating run's three builds (bars TR-1
  to TR-5).
- The gating run: `run-ab-helper.sh` (the run), `provenance.txt` (the
  builds, the host, FFmpeg and the harness), `identity.txt` (outputs
  before timing), `g.{sd,720p,1080p,4k}.json` and `.log` (one
  `scripts/h264_parity.py` call per class), `g.summary.json` and
  `analysis.txt` (`analyze.py`'s output).

# Both chroma planes' edges in one call: profile, counts, timing and tools

Evidence behind `docs/design/h264.md`, "Kernel round: both chroma planes'
edges in one call".

- `profile/`: the stage-by-stage profile of `main` at e6d810cf against
  FFmpeg 9.0.2 on the eight streams of `docs/measurements/h264-gap-2/`
  (`perf` cpu-clock samples, on a VM that reported an Intel Xeon @ 2.50GHz,
  family 6, model 85, not the bars' 2.90 GHz class): `run.sh`,
  `rep.py` and `stages.py` (the gap-2 stage map with the intra kernels'
  names added), `report.txt` (stages, top functions, and the profile of
  3d4c2e63 as "old"), `stages.json`, `wall.txt`. FFmpeg is the release
  tarball's build, used as a black box: only its symbol names are read.
- `instructions.txt`: callgrind counts of the whole decode, the deblocking
  driver and the chroma kernels on two streams, and the chroma calls, for
  `main` and the branch.
- `local-timing.txt`: the local timing taken before the bars were
  written (information), with the identity of the 32 training streams.
- `tools/miri`: the Miri harness for the paired kernel's pointer accesses
  (copy `main.rs` to `src/` of a crate with this `Cargo.toml`, replace
  `KPATH` with the checkout's `crates/coreav-kernels-h264`, and run
  `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`):
  144 cases, no error.
- `analyze.py`: the summary of the gating run's two builds (bars DP-1 to
  DP-3).
- The gating run: `run-ab-helper.sh` (the run), `provenance.txt` (the
  builds, the host, FFmpeg and the harness), `identity.txt` (outputs
  before timing), `g.{sd,720p,1080p,4k}.json` and `.log` (one
  `scripts/h264_parity.py` call per class, decoders named `before` and
  `after`), `mapped/` (the same reports with the keys renamed to `base` and
  `a`, which `analyze.py` reads; `gating-README.md` is the run's note on
  it), `g.summary.json` and `analysis.txt` (`analyze.py` on `mapped/`),
  `progress.txt`.

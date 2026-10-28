# Intra prediction in the kernels crate: profile, timing and tools

Evidence behind `docs/design/h264.md`, "Kernel round: intra prediction in
the kernels crate".

- `profile-intra.txt`: `perf` cpu-clock samples of the intra functions on
  the eight streams of the transform round's profile
  (`../idct-rows/profile/streams.txt`), this VM of the 2.90 GHz class:
  the base with its four prediction functions kept out of line, and a
  build of this design. `profile-intra.sh` is the script.
- `local-timing.txt`: the local timing taken before the bars were
  written (information), with the identity of the 32 training streams.
- `tools/miri`: the Miri harness for the intra kernels' pointer accesses
  (copy `main.rs` to `src/` of a crate with this `Cargo.toml`, replace
  `KPATH` with the checkout's `crates/coreav-kernels-h264`, and run
  `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`):
  768 cases, no error.
- `analyze.py`: the summary of the gating run's three builds (bars IR-1
  to IR-5).
- The gating run: `run-ab-helper.sh` (the run), `provenance.txt` (the
  builds, the host, FFmpeg and the harness), `identity.txt` (outputs
  before timing), `g.{sd,720p,1080p,4k}.json` and `.log` (one
  `scripts/h264_parity.py` call per class), `g.summary.json` and
  `analysis.txt` (`analyze.py`'s output).

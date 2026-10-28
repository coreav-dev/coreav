# CABAC round: the rangeLPS tables in half the lines (probe)

`docs/design/h264.md`, "CABAC round: the rangeLPS tables in half the lines, a probe". Development VM of the 2.90 GHz class
(Intel Xeon @ 2.90 GHz, family 6, model 106). Arms: `base` = `main` (crates as `3d60f0bf`), `compact` =
`rejected/h264-cabac-compact` `9ea78b2e`, both `cargo build --locked --profile dist --example decode -p coreav-h264`.

- `lines.txt`: `groups.py` (over `lines.py`, self instructions per source line from a callgrind output, inlined lines
  attributed to their own file) for `main` built with `CARGO_PROFILE_DIST_DEBUG=line-tables-only`, 100 frames of
  `a81de82106be` and 200 of `1d4b1775d78a`.
- `cachegrind.txt`: `valgrind --tool=cachegrind --cache-sim=yes --D1=49152,12,64`, both arms, three CABAC streams.
- `identity.txt`: raw output of `compact` at 1 and 8 threads against `base` at 1, the 32 training streams.
- `ab.jsonl`, `ab.txt`: `../luma8x8/ab.py` (one thread pinned, `--discard`, five interleaved rounds) and `an2.py`.

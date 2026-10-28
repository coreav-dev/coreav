# Weighted prediction round: counts and timing

Evidence behind `docs/design/h264.md`, "Kernel round: weighted prediction
inlined". The candidate was rejected; its code is on the branch
`rejected/h264-weighted-sse2`.

- `instructions.txt`: the whole decode of four streams under callgrind,
  `examples/decode --discard`, one thread, before and after the
  candidate, and `Auto::combine`'s inclusive count and calls. "After" is
  an earlier commit of the candidate branch, not the measured d81655fd,
  and "before" is the luma round's rejected candidate C.
- `run.sh BEFORE AFTER`: the gating run (bars WP-1 to WP-4), two decoders
  built alike from one tree each, outputs checked, then one
  `scripts/h264_parity.py --pin 2 --rounds 5` call per class with FFmpeg
  9.0.2 as the third command. `run-ab-helper.sh` is the script that ran:
  `run.sh` with the paths of the host it ran on, a stop on any CPU but
  the 2.90 GHz class, a check of the streams' SHA-256 against the
  training set's `es.json`, and the harness taken from the base tree.
- `provenance.txt`: the binaries' SHA-256, the compiler, the CPU, FFmpeg
  and the harness; `identity.txt`: each stream's output MD5, the base at
  one thread and the candidate at one and eight.
- `g.<class>.json`: the harness's per-stream times; `analyze.py` reads
  them, with the nine weighted streams fixed before the run, and writes
  `g.summary.json` and `analysis.txt`.

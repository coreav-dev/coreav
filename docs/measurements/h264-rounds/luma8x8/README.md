# Kernel round: an inter macroblock's 8x8 luma residual in one call (not queued)

`docs/design/h264.md`, "Kernel round: an inter macroblock's 8x8 luma residual in one call". The code is on
`rejected/h264-luma8x8-mb` at `1685ba23`, based on `main` at `056bd25`. Every file here comes from a development VM of
the 2.50 GHz class (Intel Xeon @ 2.50 GHz, family 6, model 85); none is a bar run.

- `ab.py`, `an.py`: the interleaved CPU-time A/B (one thread pinned to CPU 2, `--discard`, the class frame
  counts; each round shuffles the streams and alternates the arms' order) and its analysis (median per-stream
  ratio, 95% bootstrap interval, per class, and the subset named by `SUBSET`); `ES` names the stream directory.
- `local-ab.jsonl`, `local-ab.txt`: the 32 training streams, five rounds, `main` against this round.
- `local-ab-one.jsonl`, `local-ab-one.txt`: `ab.py` with `ES` set to a directory of two streams alone,
  `f8d92a911e72` (the most inter 8x8 blocks, half of them DC-only) and `175433794e02`, ten rounds.
- `instructions.txt`: callgrind instruction counts, 30 pictures, four streams with inter 8x8 blocks.

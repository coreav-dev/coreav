# MB layer round 2: the layout without the added instructions

Design and bars ML2-1 to ML2-7: `docs/design/h264.md`, "MB layer round 2: the layout without the added
instructions". The bars and these scripts were committed before any timing of the round's code.

- `run-ab-helper.sh BEFORE AFTER`: the gating run on a host of the 2.90 GHz class (calls `g`, `t8`, `gm`, `h`;
  identity of the two arms first). Paths are the A/B host's.
- `analyze.py DIR`: the per-stream ratios, their medians with 95% bootstrap intervals, and the bars.
- Local screening, before any gating request: `luma8x8/ab.py` (eight interleaved rounds, one thread pinned,
  `--discard`, the 32 training streams), read with `luma8x8/an.py`.

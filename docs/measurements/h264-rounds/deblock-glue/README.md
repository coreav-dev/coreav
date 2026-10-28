# Deblocking round: thresholds from shifted tables

`docs/design/h264.md`, "Deblocking round: thresholds from shifted tables".

- `profile/`: the deblocking stage of `main` at `814eb2a1` against FFmpeg 9.0.2, split into kernels and glue. Recorded
  with `../../profile/rec.py` and `rec2.py` (two rounds, the 32 training streams with the class frame counts, one
  thread, pinned, 4 kHz `cpu-clock`), on the development VM of the 2.90 GHz class (Intel Xeon, family 6, model 106);
  CoreAV built `dist` with line tables only; FFmpeg the BtbN build `n9.0.2-17-g2a571b6068` for time and the 9.0.2
  attribution build of `../../profile/README.md` for stages. Addresses resolved with `../../profile/resolve2.py`.
  `table.txt`: the stage table (`classify.py`). `split.txt`: the deblocking stage by part, with its items
  (`deblock_split.py`). `lines.txt`: the hottest source lines of CoreAV's deblocking samples (`lines.py`; line numbers
  are `deblock.rs` at `814eb2a1`).
- `instructions.txt`: callgrind instruction counts of the whole decode, `main` against the branch.
- `identity.txt`: the branch's output against `main`'s; `conformance.txt`: the JVT suite on the branch.
- `analyze.py`: the bars D-1 to D-4 from the A/B run's JSON files.
- `gating/`: the A/B run of the bars D-1 to D-4, from branch `measured/h264-deblock-glue` `7a2553b8` (its README,
  `analysis.txt`, the JSON and logs of each call, the runner).

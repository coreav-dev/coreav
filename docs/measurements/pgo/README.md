# Profile-guided optimisation of the decoder build

`docs/design/pgo.md`. `analyze.py` reads the gating run's calls (`g`, `t8`, `gs`, `h`) for bars PG-1 to PG-6.

`gating/` holds the gating run (`before` the plain build, `after` the PGO build, both from `7a177c62`): PG-1 to PG-3
missed, PG-4 to PG-7 met, so PGO is not adopted.

`local/` holds the probe. It ran on a development VM of the 2.90 GHz class (Intel Xeon @ 2.90 GHz, family 6, model 106), on
`main` at `23b45d4c` (its crates are `3d60f0bf`'s). None of it is a bar run.

- `ab-1t-a.jsonl`, `ab-1t-b.jsonl`: `h264-rounds/luma8x8/ab.py` (CPU time, one thread pinned to CPU 2, `--discard`,
  the class frame counts, five interleaved rounds) over the 32 training streams. Arms:
  - `plain` and `splain`: the plain `dist` builds, fast and safe;
  - `pgo` and `spgo`: their PGO builds from the conformance profile;
  - `pgocs`: the PGO build trained on the conformance and synthetic streams;
  - `pgosyn`: the PGO build trained on the synthetic streams alone.
- `ab-8t.jsonl`: `ab8.py`, the same with `--threads 8`, unpinned, recording CPU time (`s`) and wall time (`w`).
- `synth.sh`: the six synthetic streams, from FFmpeg 9.0.2's test sources and libx264.
- `local.txt`: `an2.py` over each file (median per-stream ratio, 95% bootstrap interval, per class, streams faster).

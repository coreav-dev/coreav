# Motion round: a block's motion in 12 bytes

The design, the bars and the result are in `docs/design/h264.md`, "Motion round: a block's motion in 12 bytes".

- `profile/`: the attribution of macroblock control on `main` at 686fb48f against FFmpeg 9.0.2 with `-cpuflags 0` that chose the round (`attribution.txt`), and the scripts that made it (`tools/`: `callgrind.sh` and `perf.sh` run the decoders, `attr.py` sums callgrind outputs by function, source line and address, `rep.py`, `time.py`, `lines.py` and `ipline.py` print the tables; `stages.py` is the stage mapping of `docs/measurements/h264-rounds/deblock-pair/profile/`). FFmpeg is the release tarball's build, used as a black box: only its symbol names are read.
- `local/`: information only, on a VM that reported an Intel Xeon @ 2.50GHz (family 6, model 85), not the bar class: simulated cache and branch counts of `main` and the branch (`simulated.txt`), and local timing (`local-timing.txt`, from the reports and `run*.sh` beside it).
- `ptypes.txt`, `b-streams.txt`: each training stream's picture types over its first 300 pictures (`ffprobe`), and the 17 streams with B slices that bar M12-3 reads.
- `analyze.py`: the summary of the gating run's two builds, bars M12-1 to M12-5, for the default and the mapped run.
- `gating/`: the gating run on the 2.90 GHz class, by the A/B helper (`run-ab-helper.sh`, `provenance.txt`, `identity.txt`). It compared `main` at b7cad07b (`base`) against the branch at a266a6ca (`a`), now `rejected/h264-motion-12b`. There are two runs of the same two binaries: `g.*` with the default allocator, and `gm.*` with `MALLOC_MMAP_THRESHOLD_=131072` exported for every command, FFmpeg included. `analysis-g.txt` and `analysis-gm.txt` are `analyze.py`'s output.

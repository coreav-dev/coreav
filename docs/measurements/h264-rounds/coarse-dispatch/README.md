# Kernel round: the hot paths compiled twice

`docs/design/h264.md`, "Kernel round: the hot paths compiled twice".

- `analyze.py`: the bars CD-1 to CD-8 from the A/B run's reports (`python3 -I analyze.py DIR g gm t8`).
- `gating/`: the A/B run on the 2.90 GHz class (`README.md`, `analysis.txt`); CD-5 not met, the round rejected.
- `local/functions-v3.txt`: `perf` cpu-clock samples per function of `main` at `40f29a6d` built without flags and for
  x86-64-v3, eight training streams (two per class), two rounds, one thread, discard; the largest differences.
- `local/instructions.txt`: callgrind instruction counts of the builds the design weighed, three streams.
- `local/symbols.txt`: the out-of-line functions of this round's build that `main`'s does not have, and the reverse.
- `local/modes-training.txt`, `local/modes-synthetic.txt`: `scripts/h264_modes_identity.py` on the training streams and
  on the synthetic set of the CI job `modes-identity`: both builds' output compared, at 1 and 8 threads.
- `local/local-timing.txt`: local CPU times, information only.
- `local/identity-after.txt`, `local/identity-safe.txt`: the output of this round's builds, with and without `fast`, against `main`'s, 32 training streams at 1 and 8 threads.

All local files come from the development VM of the 2.90 GHz class (Intel Xeon @ 2.90 GHz, family 6, model 106); none is a
bar run.

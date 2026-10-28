# Motion round: the reference's lines requested ahead, cheaply

`docs/design/h264.md`, "Motion round: the reference's lines requested ahead, cheaply".

- `analyze.py`: the bars MA-1 to MA-5 from the A/B run's reports (`python3 -I analyze.py DIR g t8`).
- `local/profile-4k.txt`: `perf` cpu-clock samples per function, the 4k class, one thread, `main` at `75d77dc` and
  FFmpeg 9.0.2's `ffmpeg_g` (function names only).
- `local/cachegrind-3b1b.txt`: simulated cache misses of both decoders on `3b1b441e23ae`, 60 pictures, by function
  (FFmpeg's by function name only).
- `local/thp.txt`: CPU time of both decoders on the 4k class with transparent huge pages `madvise` (the host's
  setting) and `always`.
- `local/variants.txt`: the prototype variants (`all1.txt` and `c8.txt` are two of its runs, per stream).
- `local/identity-after.txt`: this round's output against the base's, 32 training streams at 1 and 8 threads.
- `local/instructions-sd.txt`: instructions on an sd stream, where no hint is issued, with `Planes::request` inlined
  and out of line.
- `local/identity-rebased.txt`: the rebased commit's output against `main` at `ddb34e9`, 32 training streams at 1 and 8
  threads.
- `conformance.txt`: the conformance suite on the rebased commit, 1, 2 and 8 threads, detected and scalar kernels.
- `gating/`: the A/B run that decided the bars (bar class, 2.90 GHz); its `README.md` describes it.
- `local/local-timing.txt`: this round's build against its base, CPU time, all 32 streams, information.

All local files come from a development VM of the 2.50 GHz class (Intel Xeon @ 2.50 GHz); none is a bar run.

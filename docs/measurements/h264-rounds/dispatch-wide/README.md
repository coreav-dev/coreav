# Kernel round: the hot paths compiled twice, wider

`docs/design/h264.md`, "Kernel round: the hot paths compiled twice, wider".

- `analyze.py`: the bars WD-1 to WD-8 from the A/B run's reports (`python3 -I analyze.py DIR g gm t8`).
- `local/instructions.txt`: callgrind instruction counts of `main`, its v3 build, the rejected round's code and the builds
  this round weighed, built without flags and for x86-64-v3, five training streams (two sd, three 720p; CABAC and CAVLC).
- `local/functions-rejected.txt`, `local/functions.txt`: where the v3 build of the rejected round's code, and of this
  round's, executes fewer instructions than the same code built without flags: per compiled function, same source.
- `local/files-rejected.txt`: the same for the rejected round's code, per source file (inlining resolved).
- `local/functions-w1.txt`: the first build of this round, every function through `Tier::enter`, against the rejected
  round's code, per compiled function.
- `local/identity-after.txt`, `local/identity-safe.txt`: the output of this round's builds, with and without `fast`,
  against `main`'s, 32 training streams at 1 and 8 threads.
- `local/local-timing.txt`: seconds per training stream on this VM, `main` and its v3 build against this round's builds,
  three rounds; information only.

- `gating/`: the A/B run that decided the bars (bar class, 2.90 GHz); its `README.md` describes it.
- `conformance.txt`: the conformance suite on the merged commit, 1, 2 and 8 threads, with and without `fast`, detected
  and scalar kernels.

All local files come from a development VM with an Intel Xeon @ 2.50 GHz (family 6, model 85), not of the bar class;
none is a bar run.

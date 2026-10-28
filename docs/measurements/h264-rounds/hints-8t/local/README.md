# Local timing (information)

Development VM of the 2.90 GHz class (Xeon @ 2.90GHz, family 6, model 106; 4 cores, 8 threads), not a bar run.
`dec-base` = `main` at `7af5d4f5`, `dec-k` = the change (`E` with `O`), both `dist` (`binaries.sha256`). `run.sh` runs
bundle 2's `ab8.py` (eight threads unpinned, 6 rounds kept) and `ab1t.py` (one thread pinned, CPU time, 4 rounds
kept) with this VM's paths; one warm-up round each, not kept.

- Eight threads (`t8.txt`): wall 0.9715 [0.9529, 1.0013], 20 of 32 faster (sd 0.9535, 720p 0.9707, 1080p 0.9783,
  4k 0.9935); CPU 0.9698 [0.9518, 0.9989]. Eight threads here share four cores, so the interval is wide; the bars
  are Compute Engine's.
- One thread (`t1.txt`): 0.9894 [0.9793, 1.0010], 21 of 32 faster (sd 1.0069, 720p 0.9936, 1080p 0.9840, 4k 0.9596).
  At one thread the change issues `main`'s hints (callgrind: +0.04% instructions at sd, +0.02% at 1080p), so this
  spread is the run's and the layout's, not the hints'.

Checks on `dec-k` (`../checks.txt`, `../identity.txt`): output identical to `dec-base` on the 32 training streams at 1
and 8 threads; the two crates' tests; conformance 97 of 97 at 1, 2 and 8 threads with detected and scalar kernels.

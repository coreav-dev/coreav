# Bundle 4: local timing (information)

On the development VM (Xeon @ 2.50 GHz, 8 vCPUs: four cores, two threads each), not a bar. Binaries:
`binaries.sha256` (`dist`, `RUSTFLAGS` unset): `dec-base` = `main` at `41c62136`, `dec-H` = the banded hints alone
(`rejected/h264-banded-hints`, `bb9547a6`), `dec-k` = bundle 4 (`db25cc6a`). Scripts: the banded hints' `ab8.py` and
`ab1t.py` with this round's paths.

- `first/` (`run.sh`): eight threads, six rounds, `dec-k` over `dec-base`: wall 1.0011 [0.9920, 1.0169]. Its
  one-thread half (1.0103) ran while a build ran beside it, so it is set aside, and the run was made again.
- `run2.sh`: eight threads, ten rounds, three arms (`t8.txt`): wall `dec-H` 1.0018 [0.9784, 1.0247] (sd 0.9818),
  `dec-k` 0.9977 [0.9852, 1.0067] (sd 0.9956). One thread pinned, six rounds (`t1.txt`): `dec-k` 0.9916
  [0.9838, 1.0015].

This VM shows neither the banded hints' gain (0.9884 on Compute Engine, and 0.9715 on an earlier development VM)
nor the bundle's: its eight threads run on four cores, two to a core, which share their caches, while the cut is of
lines moving between cores (`../../miss-8t/`). `dec-H` and `dec-k` are alike here, so the merge loses nothing the
banded hints had on this host. The host the bars are set on (one thread a core) is the stall check's (`../README.md`:
0.981 of `base`'s wall time in `perf stat`, information).

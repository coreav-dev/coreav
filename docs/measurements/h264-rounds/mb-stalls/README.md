# Why the MB layer's instruction cuts move little time

Evidence for `docs/design/h264.md`, "Why the MB layer's instruction cuts move little time". Development VM,
Intel Xeon @ 2.90GHz (family 6 model 106), one thread. Builds: `dw-4cb168e` = `main` at `056bd25` (dist),
`mbl-r24` = round 24 (`rejected/h264-mb-layer` at `3535661`, dist), md5 identical to fresh builds of those
commits. The scripts ran from a local work directory (`~/h264work/r25`), whose paths they name.

- `plan.txt`: the readings of the layout run (information; this is an investigation, not a gated round).
- `cachegrind-r24.txt`: cachegrind with `--cache-sim=yes --branch-sim=yes`, 40 pictures (4k 12), `cgs.sh`,
  per function, `cgcmp.py` (est is a rough weighting for ranking only).
- `cachegrind-main.txt`: `main` alone, by that estimate (`cgtop.py`).
- `ab-layout.txt`: `ab3.py` (round 18's), 7 rounds, all 32 streams; ratios by class with `abcls.py`.
- `time-vs-instructions.txt`: perf `cpu-clock` at 10 kHz, 8 runs per build (`perfab.sh`), against the
  instruction counts (cachegrind, `--cache-sim=no`) of the same pictures (`ipc.py`).
- `mb-items.txt`: the MB layer's functions on `main` (`mbrank.py`).

Valgrind has no AVX-512, so the kernels run their AVX2 instances under it; the relative rates of kernel
functions are not comparable, those of the MB layer's (scalar) functions are.

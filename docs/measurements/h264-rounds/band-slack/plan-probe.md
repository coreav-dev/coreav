# Blocks declined at the picture's top and bottom rows at 8 threads: the probe plan (written before the probe run)

## What led here (main `b3664291`, the development VM: Xeon @ 2.90 GHz, 4 cores, 8 logical CPUs)

- `band-views/instructions.txt` (an earlier change's evidence): at 8 threads, blocks declined to `predict_list_any` are 2.2 to 15.8%
  of the one-thread instructions on four streams; on sd `ec473c99c538`, 69,798 of 416,620 banded lookups (16.8%) are
  declined at 8 threads and none at 1.
- Verified with a counter by reason (`counts.txt`, a throwaway build that counts each declined block by why): on
  `ec473c99c538` at 8 threads, 69,035 of the 69,798 declines (98.9%) are windows that reach above the picture's first
  row or below its last while every sample their fraction uses lies inside the picture. A picture of one band keeps
  `LUMA_SLACK` (4) and `CHROMA_SLACK` (2) rows before and after its planes, so with one thread the kernel takes these
  windows where they lie; a picture of several bands keeps none (`Geometry::luma_slack`), so the bands holding row 0
  and the last band cannot hold such a window and the block goes to `predict_list_any`, which copies its windows
  through `edge_window`. Of the rest, 763 are chroma windows above a middle band's apron and 50 use samples outside
  the picture (declined with one thread too). The change in view: slack rows before the planes of every band whose
  stored rows start at row 0, and after the last band's.

## Probe (output byte-identical to `main` at 1 and 8 threads on the 32 streams; checked, `identity.txt`)

- `slack2` (`probe/r32-slack`): a banded block whose window reaches past the picture's top or bottom row with every
  used sample inside (the declines the change removes, tested as the counter tests them) takes `predict_list_any`
  twice, its samples in the picture saved before the first call and put back after it. The change removes the
  declined path's cost less the fast path's for these blocks; the probe adds all of the declined path and a 384-byte
  save and restore, so it over-prices the change: a bound.

## Run and bars (fixed now)

`ab8.py 10`: 8 threads, unpinned, `--discard`, class frame counts, all 32 training streams, builds `m` (main
`b3664291`), `m-null` (the same binary, a second copy) and `slack2` rotated each round, one warm-up round; per stream
the median of 10 rounds over `m`'s; wall time decides, CPU time is information.
- PS-1 (an earlier change's threshold, as the r31 probes applied it): the change is built only if the probe costs at least 1.015
  (median per stream) overall or in a class. Otherwise no round is built and the record says so.
- PS-2: the null arm checks the host. If `m-null`'s median per stream over `m` is outside [0.990, 1.010] overall or in
  any class, the run is void as a host fault (one re-run of all three arms is allowed for it).
- A host fault (a crashed or suspended run) voids the run; nothing else re-runs it.

Passing PS-1 does not claim a gain: the bound over-prices the change, and the round built from it is judged by its
own bars, committed before its run, with the parity harness's seeded order, null arm and gate stream.

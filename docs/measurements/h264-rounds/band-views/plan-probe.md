# Eight-thread items: the probe plan (written before the probe run)

## What led here (main `2a3972b2`, the development VM: Xeon @ 2.90 GHz, 4 cores, 8 logical CPUs)

- `scaling.txt`: our time at 8 threads over FFmpeg 9.0.2's, 1.036 (median per stream, 32 training streams, 5 rounds),
  splits per stream as W1 x I x B: W1, CPU at one thread over FFmpeg's, 1.144; I, our CPU growth from 1 to 8 threads
  over FFmpeg's, 1.116 (sd 1.144, 720p 1.157, 1080p 1.079, 4k 1.092); B, FFmpeg's logical CPUs kept busy over ours,
  0.851 (we wait less). Our kernel time and page faults at 8 threads are below FFmpeg's.
- `instructions.txt`: callgrind, instructions at 8 threads over 1, two streams a class: 1.04 to 1.39. The growth is
  work that only a banded decode does (share of the one-thread instructions, four streams, sd/720p/1080p/4k):
  the band view each block builds (`reference_band` with `Planes::band`): 10.8/8.9/6.1/14.6%; the apron copies
  (`copy_top`): 12.0/5.6/3.3/2.2%; blocks declined to `predict_list_any`: 15.8/4.4/2.2/2.7%; `colocated` up to 2.9%.

## Probes (each output byte-identical to `main` at 8 threads on the 32 streams; both checked)

- `band-view` (`probe/r31-band-view` `4e140e8a`): `reference_band` builds the band's `Planes` a second time from opaque inputs,
  result discarded. Under callgrind (sd `ec473c99c538`, 8 threads) it adds 67 M instructions, 68% of what the view
  costs now (the `Planes::band` checks and `Interior::band`; the band index and the reference lookup are not doubled):
  the part a per-slice cache of band views removes.
- `apron` (`probe/r31-apron` `636058a5`): `enter` and `refresh_top` copy the apron twice (the second copy hits cache, so this
  under-prices the copy).

## Run and bars (fixed now)

`ab8.py 10`: 8 threads, unpinned, `--discard`, class frame counts, all 32 training streams, builds `m` (main), `band-view`,
`apron` rotated each round, one warm-up round; per stream the median of 10 rounds over `m`'s; wall time decides,
CPU time is information.
- PB-1: an item is built only if its probe costs at least 1.015 (median per stream) overall or in a class.
- PB-2: of the items that pass PB-1, the one with the larger overall price is built first; if both pass, the second
  is the next round.
- PB-3: if neither passes, no round is built from these items; the report says so (and the instruction shares go to
  the bundle pool).
A host fault (a crashed or suspended run) voids the run; nothing else re-runs it.

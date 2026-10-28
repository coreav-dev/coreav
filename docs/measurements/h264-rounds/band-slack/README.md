# Blocks declined at the picture's top and bottom rows at 8 threads: probe record

Plan and bars: `plan-probe.md`, committed before any timing run. Host: the development VM (Xeon @ 2.90 GHz, 4 cores,
8 logical CPUs). Base: main `b3664291`. Probe code: branch `probe/r32-slack` (`64ab6b55`), not for merge. Spend: 0 USD
(local runs only).

## Verification of the cause (`counts.sh`, `counts.py`, `counts.txt`, `counts-summary.txt`)

A throwaway build (on an earlier change's branch, `bc97fa55`, whose kept views do not change which blocks are declined) counts
each banded block by whether the kernel takes it and, if not, why: the window reaches above the picture's first row
or below its last (`top`, `bottom`), every sample its fraction uses is inside the picture or not (`used_out`), and
the band's first stored row. All 32 training streams, 8 threads, class frame counts. With one thread no block is
banded.

- `ec473c99c538` (sd): 69,798 of 416,620 banded lookups declined; 68,985 of them (16.6% of the lookups) are windows
  past the top or bottom row with every used sample inside. The hypothesis holds: the bands holding row 0 and the
  last band keep no slack rows, where a picture of one band keeps four luma and two chroma rows each way.
- Across the 32 streams these declines are 0.3% to 16.6% of the banded lookups; median per class sd 9.3%, 720p 3.1%,
  1080p 1.6%, 4k 1.9%. The other declines are windows whose used samples leave the picture (783 K over the 32
  streams), declined with one thread too, and chroma windows that start above the chroma apron of the band they are
  read from (754 K; 763 on `ec473c99c538`, 70,986 on `3c593b8f0809`); no change here touches them.
- `plan-probe.md` gives 69,035 of the 69,798 (98.9%) for `ec473c99c538`; the counter's file gives 68,985 (98.8%). The
  plan's figure counted the 50 windows with used samples outside the picture among them. The plan is left as committed.

## The bound (`ab8.py`, `probe-runs.txt`, `ps-void1.json`, `ps-void2.json`)

Output byte-identical to main at 1 and 8 threads, 32 of 32 (`identity.txt`, `ident.sh`).

| Run | null arm, wall (all / sd / 720p / 1080p / 4k) | probe, wall (all / sd / 720p / 1080p / 4k) | PS-2 |
|---|---|---|---|
| 1 | 1.0029 / 0.9931 / 1.0135 / 1.0209 / 0.9889 | 1.0417 / 1.0495 / 1.0387 / 1.0302 / 1.0466 | void |
| 2 (re-run) | 1.0000 / 0.9986 / 0.9943 / 1.0105 / 1.0135 | 1.0252 / 1.0690 / 1.0102 / 1.0297 / 0.9956 | void |

Both runs are void under PS-2: the null arm, the same binary timed twice, left [0.990, 1.010] in two classes each
time. PS-2 allows one re-run, so the probe has no valid result and PS-1 is not judged. The probe arm's figures are
reported as observed, not as a result. The window PS-2 set, 1% per class of eight streams at 8 threads unpinned, is
narrower than this host's same-binary spread (up to 2.1% in a class here); the parity harness's null arm judges
3% per stream instead, and later plans use that.

## The round (design note: `docs/design/h264.md`, "Threading round: slack rows at the picture's edges in every layout")

The bars BS-1 to BS-5 and the local request rule were committed before any timing of the change, in `f52a324f` on
branch `perf/h264-band-slack`. In that commit the section sits under the wrong heading: an insertion anchored on
`## Verification` matched inside an earlier `### Verification`. Its text is the one in the design note. Code:
`rejected/h264-band-slack` (`c27c8be5` measured, `k.diff`; `4b512a85` the origin fix, `k-origin-fix.diff`).

- `identity-round.txt`: `k` at 1 and 8 threads against `main` at 1, the 32 training streams: identical (`ident.sh`).
- `conformance.txt`: 97 of 97 in every mode.
- `local-ab/ab8.txt`, `local-ab/ab8.json`: eight threads, `ab8.py 10`, with a null arm (a second copy of `base`).
- `local-ab/ab1.txt`: one thread pinned, `ab3.py 6`, all 32 streams; its summary line from `ab1sum.py` (`ab3.py` keeps
  no per-round times).
- `instructions.txt`, `fdiff.py`: callgrind totals and per-function deltas, `k`, the origin fix, and a diagnostic.

Result: not queued. The one-thread figure, 1.0032, is above the request rule's 1.003, and callgrind shows the cost is
real, in `deblock_rows`. Spend: 0 USD (local runs only).

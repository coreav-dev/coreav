# Probe AP: the aprons' copies at eight threads, on the Compute Engine host (written before the run)

## Why

`../scaling-8t/` ranks the aprons second among the causes of our growth from one thread to eight: `copy_top`
0.82% and the `memmove` it calls 7.10% of our one-thread cycles, about 7.9% over the 32 training streams (sd about
19.7, 720p 3.5, 1080p 1.3, 4k 6.7; run 2's call graphs, 14 streams). Those are cycles of all threads, not wall time.
In run 2's call graphs the copies' samples are `__memmove_avx512_unaligned_erms` in user mode, with almost no kernel
samples under them (first touch is not the cost); the sd stream `ec473c99c538` spends 57% of its one-thread cycles
there at eight threads. The apron is 23 luma and 9 chroma rows a band, copied when its first row is filtered, after
4 luma rows, 2 chroma rows and a row of motion copied when decoding enters it (`Canvas::enter`,
`Canvas::refresh_top`, `copy_top` in `picture.rs`): 27 luma rows for every 16 a band owns at sd.

The only timing so far is a copy made twice, on the development VM: 1.0077 [0.9901, 1.0188] of the eight-thread wall
time (`../band-views/probe.txt`), within that host's noise. A second copy finds its source and its destination in
cache, so it prices the copy below what the first one costs. This probe times the copies on the host of the
eight-thread bars, both ways: made twice, and not made at all.

## Arms

All three built alike: `cargo build --locked --profile dist --example decode -p coreav-h264`.

- `base`: `main` at `13e4dfc4`.
- `twice`: branch `probe/r38-apron-twice` (`6cc8190d`): `enter` and `refresh_top` call `copy_top` twice (the change of
  `probe/r31-apron`, on this `main`). Output byte-identical to `base` on the 32 training streams at one and eight
  threads (`identity-probe.txt`, checked before the run).
- `none`: branch `probe/r38-apron-none` (`599db851`): `copy_top` copies no sample row, only the motion. The
  apron's samples are then whatever the reused buffer held, so the output at eight threads is wrong, and the arm
  is for timing only: the parse does not read samples, and no kernel's work depends on their values except the
  deblocking filter's per-edge decisions (a few rows a band). At one thread a picture is one band with no apron, so
  its output stays byte-identical to `base` (checked). It prices what a cut that removed every sample copy could
  gain at most; no design removes them all.

## Run

`scripts/h264_parity_gcp.py` (`run-gcp.sh`, binaries from `build.sh`) on `c4-standard-16`, one thread a core, STANDARD: decoders `base`,
`twice` and `none`, null arm `base`, FFmpeg 9.0.2 (BtbN, as an earlier change), the 32 training streams, one call a class at
eight threads, 5 rounds, calls of at least 3 s, seed 20261026, gate stream `41e324ebb708` (sd), the host's null and
spread bars ("Where the eight-thread arms run"), at most 3 hours and 5 USD.

## Rule (fixed now)

- **AP-1.** `none` over `base`, eight threads, discard, median of the per-stream ratios over the 32 or over any
  class: at most **0.985** (the copies are worth at least an earlier change's 1.5%) opens a round that cuts the copies, with its
  own design-note section and bars committed before its timing, aimed at the classes that pass. Above 0.985 overall
  and in every class: no round is built from the copies; the result is recorded, with its cause, as below the
  threshold.
- Information, not bars: `twice` over `base` (against the development VM's 1.0077), each class, the write arm,
  `none` and `twice` against FFmpeg, the null arm's median.
- A void run (the harness's rules) is run again into a new directory; nothing else re-runs it.

## Result (`run1/`, `ap.py`, `run1/ap-summary.txt`)

One run, not void (`run1/verdict.json`): the reference moved +0.45% between the first gate call and the last, no
stream unresolved, the null arm's median 1.0006 (discard) and 0.9991 (write). `c4-standard-16`, one thread a core,
STANDARD, Xeon Platinum 8581C, in `us-east4-a`: two attempts before it created no VM, the project's vCPU quota
being held by other runs (`run1.void1/`, all regions; `run1.void2/`, C4 in `us-central1`), so `run-gcp.sh` takes the
zone from `ZONE` and this run used `us-east4-a`, as an earlier change's probe did; the harness's CPU check holds it to the
calibration's CPU. Spend: 2.04 USD (bound), 0.01 for the two attempts.

| eight threads, discard | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| `none` / `base` | **0.9810** [0.9666, 0.9888], 32 of 32 faster | **0.9508** | **0.9779** | 0.9907 | 0.9893 |
| `twice` / `base` | 1.0046 [0.9971, 1.0069] | 1.0072 | 0.9991 | 0.9946 | 1.0033 |
| write, `none` / `base` | 0.9860 [0.9791, 0.9893] | 0.9529 | 0.9849 | 0.9964 | 0.9884 |
| `base` / FFmpeg | 0.9336 [0.8751, 0.9646], 24 of 32 faster | 0.9696 | 0.9376 | 0.9799 | 0.7570 |
| `none` / FFmpeg | 0.8932, 28 of 32 | 0.9252 | 0.9198 | 0.9726 | 0.7441 |

**AP-1 is met**: over the 32 (0.9810) and at sd (0.9508) and 720p (0.9779); 1080p and 4k are above 0.985. A round
that cuts the copies opens, aimed at sd and 720p.

What it says about the cost (a reading, not a bar): a second copy costs a quarter of what not copying saves (+0.46%
against −1.90% over the 32; +0.72% against −4.92% at sd). The second copy finds its source and destination in
cache; the first finds the destination cold: the apron's rows of a reused band buffer were last written when the
buffer held an older picture, and only the 4 luma rows and 2 chroma rows of the entry copy are written in between.
So most of the copies' price is the misses on the rows they write, not the bytes moved, and the cut's first target
is those misses (rows written warm, or fewer cold rows), the copy's own work second.

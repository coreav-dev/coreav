# Probe PF: a new band's first touch twice, at eight threads (written before the run)

**Result: PF-1 met; a round on the first touch opens** (`README.md`).

## Why

`../scaling-8t/README.md` ranks first touch fifth among the causes of eight threads only: about 3.3% of our
one-thread cycles over the 32 (4k 8.8%): `BandData::fit` writing a new band's planes (2.48, 4k 5.68), and the
predicted blocks writing a new picture's luma pages first (0.81, 4k 3.16).

What is new at eight threads (`main` at `13e4dfc4`, a counting build that prints `Decoder::pooled_pictures`, one
stream a class at the class frame counts, discard): the picture pool serves almost every picture at both thread
counts, and the pictures it cannot serve are the ones decoded while the pipeline fills.

| stream | pictures | new at 1 thread | new at 8 threads |
|---|---|---|---|
| `41e324ebb708` (sd) | 1798, 1791 | 3 | 7 |
| `1d4b1775d78a` (720p) | 298, 290 | 4 | 5 |
| `175433794e02` (1080p) | 298, 291 | 1 | 2 |
| `3b1b441e23ae` (4k) | 147, 143 | 3 | 10 |

A new picture costs its pages' first touch: `Canvas::new`, on the caller's thread, fills the chroma planes, the
motion and the heads (`BandData::fit`); the luma plane comes zeroed from the allocator, and its pages fault when a
worker first writes them. At 4k ten new pictures of 12.4 MB each are about 7% of a 150-frame run's pictures.

The changes in view either touch fewer new pages (a smaller set of pictures in flight at the start) or move the
touch off the caller's thread (fill each new band on the worker that decodes it). Both remove at most what one new
band's first touch costs. This probe prices that.

## Arms

- `base`: `main` at `13e4dfc4`, `cargo build --locked --profile dist --example decode -p coreav-h264`.
- `pf2`: branch `probe/r39-first-touch`, the same build. When `BandData::fit` makes a band's buffers new, it also
  makes a second set of the same sizes and writes every page: the luma zeros through `black_box`, the chroma 128,
  the motion and the heads. It then keeps them for the process (`mem::forget`). Freeing them could move glibc's mmap
  threshold, and so change what `base`'s own allocations cost. So `pf2` doubles a new band's first touch. It does it
  all on the thread that runs `fit`, the caller's, where `base` faults its luma pages on the workers. That makes the
  probe a bound on what either change removes. Output byte-identical to `base` on the 32 training streams at 1 and 8
  threads (`identity-probe.txt`), checked before the run.

## Run

`scripts/h264_parity_gcp.py` (the copy on `docs/h264-view-path-1t`, at its defaults of eight threads, unpinned) on
`c4-standard-16`, one thread a core. Decoders `base` and `pf2`, null arm `base`, the 32 training streams, 5 rounds,
calls of at least 3 s, seed 20261027, gate stream `36501d448eaa` (720p), the host's bars and void rules, cap 5 USD.

## Rule (fixed now)

- **PF-1.** `pf2` over `base`, eight threads, discard, median of the per-stream ratios. At least **1.015**, over the
  32 or in a class (sd, 720p, 1080p, 4k), opens a round on the first touch, with its own design note section and bars
  committed before its timing (the earlier threshold, as `../publish-wake/plan-probe.md` PW-1 applies it). Below 1.015
  everywhere: no round is built, and the record says so.
- Information, not bars: the write arm, the null arm's median, the one-thread counts above.
- A void run (the harness's rules) is run again into a new directory; nothing else re-runs it.

Passing PF-1 does not claim a gain: the probe over-prices the change, and the round built from it is judged by its
own bars, committed before its run.

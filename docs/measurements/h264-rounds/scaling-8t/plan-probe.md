# Probe PV: the banded reference's view built twice, at eight threads (written before the run)

## Why

Run 1 (`run1/`, plan S2) ranks the band views first among the items that are ours (S2-2), and a reading after the
run that adds the lines of `predict_fast_body` that a banded reference takes for every block (the `whole` lookup
that misses, the call to `reference_band` and the return of its `Planes`) puts that path at 11.3% of our one-thread
cycles at eight threads, over the 32 (sd 12.6%, 720p 7.3%, 1080p 6.9%, 4k 18.4%; `viewpath.py`), against our whole
growth of 41.1% and FFmpeg's of 26.2%. The probe sizes its worth in wall time on the host of the eight-thread bars.

## Arms

- `base`: `main` at `dd85b381`, `cargo build --locked --profile dist --example decode -p coreav-h264` (byte-identical
  to the `m-acf3` of run 1).
- `pv2`: branch `probe/r37-view2` (`dc0f1d7c`), the same build; in `predict_fast_body`, where the reference is banded, it calls
  `reference_band` once more first, every argument and the result through `black_box`, and drops that view. The
  whole-picture lookup is not doubled. One thread takes the path only for a reference not yet whole when its slice
  starts. Output byte-identical to `base` on the 32 training streams at 1 and 8 threads (`identity-probe.txt`).

## Run

`scripts/h264_parity_gcp.py` on `c4-standard-16`, one thread a core: decoders `base` and `pv2`, null arm `base`,
the 32 training streams, 5 rounds, calls of at least 3 s, seed 20261026, gate stream `36501d448eaa` (720p), the
host's bars and void rules ("Where the eight-thread arms run"), cap 5 USD.

## Rule (fixed now)

- **PV-1.** `pv2` over `base`, eight threads, discard, median of the 32 per-stream ratios: at least **1.015** (PB-1
  of `../band-views/plan-probe.md`, the earlier threshold) opens a round that removes most of the path, with its own
  design note section and bars committed before its timing. Below 1.015: the path is not a round alone, and the
  kept views of an earlier change (`rejected/h264-band-views`) stay a bundle candidate.
- Information, not bars: each class, the write arm, the null arm's median.
- A void run (the harness's rules) is run again into a new directory; nothing else re-runs it.

# The view path's one-thread cost at 4k, on a quiet host: a diagnostic (written before the run)

**Result: undecided by D-2 (null arm +0.61%); the view path stays out of the bundle pool** (`README.md`).

## Why

The view path round (`vp2`, branch `rejected/h264-view-path`, code `b97a575e`, plan and evidence under
`docs/measurements/h264-rounds/view-path/` on that branch) read, on the development VM, one thread, `vp2` over `b2`
(bundle 2, `ae673d8e`): 1.0011 [0.9999, 1.0069] over the 32 and **1.0152 at 4k**, against an instruction cost of
0.0% to 0.25%. A second build (`vp4`, `rejected/h264-view-path-2`) that keeps `b2`'s per-block path at one thread
read 4k 1.0075 [0.9926, 1.0125]; that VM's null arm at 4k reached 1.019. So it is open whether the 4k cost is real.
The round's eight-thread gain (local 0.9782 [0.9736, 1.0048]) is a bundle candidate only if the one-thread cost is
not real or can be removed. This run decides which, on the host whose null arm moves about 0.1%.

## Arms

`b2` and `vp2` built from their code commits, `ae673d8e` (bundle 2) and `b97a575e` (the view path), with
`cargo build --locked --profile dist --example decode -p coreav-h264`, as the local timing's binaries were
(`view-path/binaries.sha256` on `rejected/h264-view-path`: `b2` `47ef059e88ce2740…`, `vp2` `78b8f3b128518b70…`); the
run's `host.txt` records the sha256 of what it ran. Null arm `b2`.

## Run

`scripts/h264_parity_gcp.py` on `c4-standard-16`, one thread a core, `--threads 1 --pin 3`, `--classes 4k` (the
eight 4k training streams, 150 frames), 10 rounds, calls of at least 3 s, seed 20261027, gate stream
`36501d448eaa` (720p), the host's bars and void rules, cap 2 USD. The in-call gate runs at one thread on the pinned core:
`h264_parity.py` takes no `--gate-threads` with `--pin`.

## Rule (fixed now)

- **D-1.** `vp2` over `b2`, one thread, discard, median of the eight per-stream ratios: at most **1.005** reads the
  local 4k cost as the development VM's noise, and the view path (`vp2`) goes to the next bundle round's pool, whose
  own bars (one thread, per class) judge it with the rest. Above 1.005: the cost is real, and the view path stays out
  of the pool until a build that removes it is measured.
- **D-2.** The null arm's median over the eight outside [0.995, 1.005]: the host cannot resolve D-1's margin and the
  run decides nothing (the view path stays out). That is not a host fault and is not run again.
- Information, not bars: the write arm, the interval, each stream.
- A void run (the harness's rules) is run again into a new directory; nothing else re-runs it.

First attempt (`vp1t-run1.a13`, VM 1.3 minutes, 0.03 USD): void with no timing. The harness passed `--gate-threads 8`
with `--pin`, which `h264_parity.py` refuses, so the call wrote no report. Fixed in the harness (the gate follows
the pin) before the run that counts, into a new directory; nothing else changed.

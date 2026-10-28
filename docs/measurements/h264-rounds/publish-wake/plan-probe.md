# Probe PW: a band's publishing woken twice, at eight threads (written before the run)

**Result: PW-1 not met, no round built** (`README.md`).

## Why

`../scaling-8t/README.md` ranks the bands' publishing and waits third among the causes of eight threads only: about
6.0% of our one-thread cycles over the 32 (sd 19.0%), of which the futex wake on every band published is 2.41 (sd
8.12), the kernel's side of blocking 1.92 and the user side 1.67. At sd a band is one macroblock row.

`Picture::publish` sets the band's cell, then takes the picture's lock and calls `Condvar::notify_all` for every band,
whether or not a reader waits. The standard library's futex condition variable does not count its waiters, so each
call is a `futex` wake system call. The change in view is to skip the lock and the wake when no reader waits for a
band of the picture (a count of waiters, read after the cell is set). This probe prices that part alone: the wake
and the lock where nobody waits. It does not price the waits themselves (the kernel's side of blocking), which the
change leaves.

## Arms

- `base`: `main` at `13e4dfc4`, `cargo build --locked --profile dist --example decode -p coreav-h264`.
- `pw2`: branch `probe/r38-publish-wake`, the same build; `Picture::publish` releases the lock after its wake, then
  takes it once more and calls `notify_all` once more, both through `black_box`. The second wake finds no waiter in
  the futex queue (the first moved them out), so it costs what a wake costs where nobody waits, plus the lock, plus
  contention with the readers the first woke, which take the lock to recheck: a bound on what the change removes.
  Output byte-identical to `base` on the 32 training streams at 1 and 8 threads (`identity-probe.txt`), checked
  before the run.

## Run

`scripts/h264_parity_gcp.py` on `c4-standard-16`, one thread a core: decoders `base` and `pw2`, null arm `base`, the
32 training streams, 5 rounds, calls of at least 3 s, seed 20261027, gate stream `36501d448eaa` (720p), the host's
bars and void rules ("Where the eight-thread arms run"), cap 5 USD.

## Rule (fixed now)

- **PW-1.** `pw2` over `base`, eight threads, discard, median of the per-stream ratios: at least **1.015** over the
  32 or in a class (sd, 720p, 1080p, 4k) opens a round that skips the wake where nobody waits, with its own design
  note section and bars committed before its timing (the earlier threshold, as `../band-slack/plan-probe.md` PS-1
  applies it). Below 1.015 everywhere: no round is built, and the record says so.
- Information, not bars: the write arm, the null arm's median.
- A void run (the harness's rules) is run again into a new directory; nothing else re-runs it.

Passing PW-1 does not claim a gain: the probe over-prices the change, and the round built from it is judged by its
own bars, committed before its run.

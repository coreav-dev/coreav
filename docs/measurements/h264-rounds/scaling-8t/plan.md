# Plan S2: our scaling from one thread to eight against FFmpeg's, on the quiet Compute Engine host (written before the run)

Diagnosis only: nothing here moves a bar or is a speed claim. It repeats `../band-views/plan-scaling.md` (S1, on the
development VM at `2a3972b`) on the host the eight-thread arms now run on ("Where the eight-thread arms run" in
`docs/design/h264.md`), at `main`'s current decoder, and adds hardware counts and samples so that our growth from one
thread to eight can be split by item in the same run.

## Why

At eight threads `main` is about 0.995 of FFmpeg's time (16 of 32 streams faster), at one thread about 0.99. S1 found
our 8T-only factor (I × B below) at 0.933: we did more work at eight threads than FFmpeg added (I 1.116) and kept
more CPUs busy (B 0.851), on a host of 4 cores with SMT. Since then the band views (rejected), the slack rows,
the band-lag rounds and the 1T rounds changed both sides of that product, and the host changed: the GCP host has 8
physical cores and no SMT, where the 4-core VM's eighth thread shared a core. Which 8T-only causes remain, and their
size, is not known at `main`.

## Host and binaries

- Compute Engine `c4-standard-16` with `threadsPerCore=1` (8 vCPUs on 8 cores), STANDARD provisioning, PMU
  `STANDARD`, `us-central1-a`, Debian 13, by `scripts/gcp_vm_job.py` (one VM, `--max-hours 1.5`, 1.20 USD an hour:
  bound 1.80 USD, cap 5 USD).
- CoreAV `main` at `acf3ee2f` (the decoder's code at `dd85b381` is the same; later commits are docs), `dist`,
  `--example decode`: `m-acf3` (the timing build, as the gating runs use) for timing and counting; `prof-acf3ee2f`
  (the same tree with `CARGO_PROFILE_DIST_DEBUG=line-tables-only`, as an earlier change) for the samples, so that inlined code is
  attributed by its own line tables. FFmpeg: the BtbN build `n9.0.2-17-g2a571b6068` (the timing reference); no
  FFmpeg build and no FFmpeg source on the VM, and FFmpeg's samples are kept by object only (its own binary as one
  item; the kernel by symbol).
- The 32 training streams at the class frame counts (sd 1800, 720p 900, 1080p 300, 4k 150), the parity harness's
  argument forms: ours `--frames N [--threads 8] --discard`, FFmpeg `-threads T -i S -frames:v N -fps_mode
  passthrough -f null -`.

## Run (`job.sh`, `scale8.py`)

1. **Timing.** Arms `us1`, `ff1` (pinned with `taskset -c 3`), `us8`, `ff8` (unpinned, 8 threads); one warm-up round
   and 7 rounds; in each round the streams and, per stream, the four arms in an order drawn from a generator seeded
   with 4880 and the round. Per command: the wall time (`perf_counter` around the process) and from `wait4` the
   user and system CPU time, minor faults, voluntary and involuntary switches. Per stream the median over rounds.
2. **Counting.** Two rounds, each arm under `perf stat` with `cycles:u`, `instructions:u`, `cycles:k`,
   `instructions:k`, `task-clock`, `context-switches`, `cpu-migrations`, `page-faults` (no multiplexing: two fixed
   and at most six programmable counters a core without SMT).
3. **Samples.** Two rounds, `us1`, `us8` (the `prof` build) and `ff1`, `ff8` under `perf record -F 2000 -e
   '{cycles,instructions}'`, all threads, user and kernel. Ours by file address, resolved locally through the
   line tables' inline chain (`llvm-addr2line --inlines`); FFmpeg's binary as one item; libc and the kernel by symbol.

## Decomposition (exact per stream, as S1)

`r8 = wall8_us / wall8_ff = W1 * I * B`, with `W1 = cpu1_us / cpu1_ff`, `I = (cpu8_us / cpu1_us) / (cpu8_ff /
cpu1_ff)` and `B = (cpu8_ff / wall8_ff) / (cpu8_us / wall8_us)`; the 8T-only factor is `I * B`. I is split by
counting into cycles and frequency: `I_c` (the same ratio in cycles, user and kernel), `I_i` (in instructions) and
`I_c / I_i` (cycles an instruction). Every factor reported per class as the median per stream, and over the 32.

Items of our growth (step 3): for each stream, an item's cycles at eight threads less its cycles at one, as a share
of our one-thread cycles; per class the mean over its streams. Against them stands FFmpeg's whole growth (`C8_ff /
C1_ff - 1`). The items, fixed now, by the first rule that matches a sample (any frame of its inline chain, CoreAV's
own symbols):

| item | rule |
|---|---|
| aprons | `copy_top`, `refresh_top`, `Canvas>::enter` |
| band views | `reference_band`, a `band` function of `predict.rs` or `picture.rs` (`Interior::band`, `Planes::band`, `Canvas::band`) |
| windows that leave a band | `predict_list_any`, `edge_window`, `Windows>::fit`, `BandData>::fit` |
| colocated motion | `colocated` |
| bands' publishing and waits (user) | `wait_for`, `publish`, `publish_ready`, any frame in `work.rs` or `picture.rs` not above, `std::sync`, `std::thread`; libc `pthread_*`, `__lll_*`, `futex*`, `__GI___clone*`, `start_thread` |
| libc copies and allocation | libc `memcpy*`, `memmove*`, `memset*`, `malloc`, `free`, `_int_*`, `calloc`, `realloc` (callers not known: no call chains) |
| kernel: first touch and memory | kernel symbols matching `fault`, `clear_page`, `alloc_pages`, `folio`, `rmqueue`, `lru`, `pte`, `pmd`, `zap_`, `free_pages`, `free_unref`, `tlb`, `memcg`, `mm_`, `page`, `mmap`, `munmap`, `vma`, `brk` |
| kernel: other | every other kernel symbol (scheduler, futex, system calls, interrupts) |
| the one-thread stages | every other CoreAV sample, by `../profile-6c9e240/classify.py`'s `core_stage` (entropy, MB layer, inter, reconstruction, deblocking, other) |

The one-thread stages' growth is split into instructions (more of the same work) and cycles at those instructions
(the same work slower: caches, other cores).

## Rules (fixed now)

- **S2-1.** A factor I or B whose class median, or median over the 32, is at least **1.015** (the earlier threshold,
  about 1.5% of a stream's work) is attributed: I by the items above, B by a later diagnostic build with wait timers
  (its own plan). Neither factor at 1.015 anywhere: the 8T axis has no item to build from scaling alone, and the
  report says so; per-stream outliers (`I * B` at least 1.10) are listed.
- **S2-2.** The items are ranked by their growth over the 32 (mean of the class means), with each class's value
  beside. The largest item whose growth is at least **1.5%** of our one-thread cycles over the 32, or in a class,
  and that is ours (not FFmpeg's, not "kernel: other" unless a function in our code issues it) is probed part-only:
  a probe that does that item's work twice (the second from opaque inputs, `black_box`), byte-identical to `main`,
  timed at eight threads on this host type against `main` (plan and bar committed before that run, `PB-1` of
  `../band-views/plan-probe.md`: a cost of at least 1.015, median over the 32, opens a round for that item).
- **S2-3.** A host fault (a VM that does not have 8 vCPUs with SMT off, a crashed or preempted run, a decoder that
  fails) voids the run and it is run again into a new directory; nothing else re-runs it. Every number is of this
  host and is not compared with the development VM's.

## Addendum S2a (written after run 1, before run 2): the callers of libc's and the kernel's growth

Run 1's samples in libc are `[unknown]`: Debian's `libc.so.6` has no symbols for its internal functions (the
`memcpy`/`memset` variants, `_int_malloc`, `_int_free`), so the item "libc copies and allocation" could not match
them; they went to "libc and loader: other", the third-largest growth (6.7% of our one-thread cycles over the 32,
15.5% at sd). The kernel's growth is mostly the wake-up path (`futex_wake`, `_raw_spin_lock`, `try_to_wake_up`), with
no caller. Run 2 names both, and moves no item of run 1:

- Same host type and binaries as run 1 (SPOT, which is enough for samples), `libc6-dbg` installed so that perf
  resolves libc's internal symbols. `us1` and `us8` only (the `prof` build), the 8 sd streams and the first 2
  streams of each other class by name, one round, `perf record -F 1000 -e cycles --call-graph dwarf,16384`.
- `callers.py`: every sample whose innermost frame is in libc or the kernel goes to (that object's symbol, the first
  CoreAV frame outside `std`, `core` and `alloc` above it); the growth of each pair, eight threads less one, as a
  share of our one-thread cycles of the stream, mean per class.
- The report puts run 2's split beside run 1's item "libc and loader: other" and the two kernel items; run 1's
  ranking by the rule in S2-2 is reported as it is, and a reading that moves samples between items is marked as a
  reading made after the run.

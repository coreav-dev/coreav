# Our scaling from one thread to eight against FFmpeg's, on the Compute Engine host, by cause

Diagnosis only: nothing here moves a bar or is a speed claim. Plans: `plan.md` (S2, and its addendum S2a) and
`plan-probe.md` (probe PV), each committed before its run. The decoder is `main` at `acf3ee2f` (the code of `main`
at `dd85b381`); FFmpeg is the BtbN build of 9.0.2 (the timing reference; `plan.md`'s "no FFmpeg build" means none was compiled
on the VM). Every number is of this host and is not compared with the
development VM's.

## Runs

| run | what | host | spend bound |
|---|---|---|---|
| `run1/` | S2 steps 1 to 3: timing (7 rounds), counting (2), samples (2), the 32 training streams | `c4-standard-16`, one thread a core, STANDARD, `us-central1-a`, Xeon Platinum 8581C | 0.52 USD |
| `run2/` | S2a: call graphs of `us1` and `us8`, 14 streams (8 sd, 2 of each other class) | the same machine type, SPOT, `us-east4-a`, Xeon Platinum 8581C | 0.04 USD |
| `run2.void1/` to `run2.void3/` | S2a, void: a script fault (`ls \| head` under `pipefail`), the region's C4 quota taken, and perf's inline lookup without `addr2line` on the VM (S2-3: crashed runs) | | 0.04, 0, 0.03 USD |
| `pv-run1/` | probe PV's timing | `c4-standard-16`, one thread a core, STANDARD, `us-east4-a` | 1.60 USD |

Spend in total, by the bounds of each run: **2.23 USD** (cap 5).

Run 2 and the probe ran in `us-east4-a` because another run held 16 of the project's 24 C4 vCPUs in `us-central1`
at the time; `scripts/h264_parity_gcp.py` takes `--zone` for that (default unchanged), and the harness's CPU check
(`--require-cpu 8581C`) holds the probe to the calibration's CPU.

## Factors (run 1, `run1/factors.txt`)

As S1 (`../band-views/plan-scaling.md`): `r8 = W1 * I * B`, with W1 our one-thread CPU time over FFmpeg's, I our
growth in CPU time from one thread to eight over FFmpeg's, B FFmpeg's CPUs kept busy over ours. Median per stream.

| | r1 | r8 | W1 | I | B | I × B | I in cycles | in instructions | cycles an instruction | streams faster at 8T |
|---|---|---|---|---|---|---|---|---|---|---|
| sd | 1.036 | 0.988 | 1.037 | **1.126** | 0.839 | 0.956 | 1.134 | 1.129 | 1.030 | 4/8 |
| 720p | 1.134 | 0.959 | 1.134 | **1.061** | 0.802 | 0.851 | 1.069 | 1.039 | 1.031 | 7/8 |
| 1080p | 1.139 | 0.981 | 1.139 | 1.010 | 0.857 | 0.881 | 1.013 | 1.017 | 1.003 | 5/8 |
| 4k | 0.999 | 0.741 | 0.998 | **1.135** | 0.632 | 0.723 | 1.132 | 1.058 | 1.049 | 8/8 |
| all 32 | 1.096 | **0.950** | 1.096 | **1.069** | 0.814 | 0.860 | 1.090 | 1.053 | 1.026 | **24/32** |

- On this host we are 0.950 of FFmpeg at eight threads (24 of 32 faster) and 1.096 at one (9 of 32; this host's one
  thread is not the bar class). Eight threads gain on FFmpeg because we keep more CPUs busy: B is 0.81 (4.73 CPUs
  busy against FFmpeg's 3.80), which more than pays for I.
- **By S2-1, I is attributed**: 1.069 over the 32, 1.126 at sd, 1.061 at 720p, 1.135 at 4k (at least 1.015). B is
  below 1 in every class and is not a cost. At sd and 720p I is mostly instructions (we execute 18.7% more at
  eight threads at sd, FFmpeg 5.8%); at 4k half of it is cycles an instruction. Outlier (I × B at least 1.10):
  `ec473c99c538` (sd), 1.350.

## Causes, ranked (run 1 items, `run1/items.txt`; run 2 callers, `run2/callers-items.txt`)

Growth from one thread to eight, cycles of all threads, user and kernel, as % of our one-thread cycles; per class
the mean over its streams, "all" the mean of the class means. Our whole growth is **41.1%** (sd 80.6, 720p 27.3,
1080p 14.6, 4k 42.0); FFmpeg's is **26.2%** of its own (sd 47.6, 720p 17.5, 1080p 11.4, 4k 28.1). The excess to
explain is about 15 points.

Run 1's items by the rule of S2-2, as fixed:

| item | all | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| the one-thread stages: inter | 9.66 | 17.62 | 7.03 | 3.99 | 9.98 |
| kernel: other | 6.91 | 17.27 | 3.58 | 1.50 | 5.30 |
| libc and loader: other | 6.74 | 15.45 | 4.73 | 1.62 | 5.16 |
| band views | **5.98** | 6.65 | 4.02 | 3.54 | 9.73 |
| the one-thread stages: MB layer | 2.43 | 3.35 | 1.28 | 1.31 | 3.76 |
| windows that leave a band | 2.40 | 5.73 | 1.73 | 0.61 | 1.54 |
| bands' publishing and waits (user) | 1.67 | 4.69 | 0.70 | 0.35 | 0.94 |
| kernel: first touch and memory | 1.59 | 0.40 | 0.76 | 0.61 | 4.60 |
| the one-thread stages: entropy | 0.88 | 1.81 | 1.18 | 0.04 | 0.47 |
| aprons | 0.82 | 2.49 | 0.46 | 0.12 | 0.20 |
| others (recon, other, colocated, libc copies, deblocking) | 2.07 | | | | |

**The probe's item departs from S2-2 as written, a choice made after the run.** Read literally, S2-2 picks the
largest row that is ours: "the one-thread stages: inter" (9.66), then "kernel: other" (6.91, which S2-2 admits only
where our code issues it; run 1 has no call chains to show that). The probe went to the band views (5.98% over the
32, 9.73% at 4k), the largest row that is a cause of eight threads only, for two reasons:
- "The one-thread stages" are the one-thread work itself. Doing that work twice would measure its one-thread
  cost, not what eight threads add. 8.33 of the inter stage's 9.66 is cycles at the same instructions, with no
  added work to double.
- "Libc and loader: other" is a residual row of `analyze.py`, not an item of the plan's table: Debian's libc has no
  symbols for its internal functions. Run 2 (S2a, planned before it ran) names its callers below.

PV's verdict stands on its own bar, committed before its run (`plan-probe.md`).

Two readings made after run 1 put those rows under their causes:

- **The band view's whole path** (`viewpath.py`, `run1/viewpath.txt`): a banded reference costs, for every block, the
  lookup of a whole picture that misses, the call to `reference_band` and the return of its `Planes`, which the item
  rule counted under the inter stage (`predict_fast_body`, `inter.rs` 1295 to 1310 at `acf3ee2f`), besides the item.
  With them: **11.28%** (sd 12.57, 720p 7.31, 1080p 6.89, 4k 18.36).
- **Libc's and the kernel's growth by caller** (run 2, the plan's S2a; 8 sd streams, 2 of each other class):

| caller (libc and kernel samples) | all | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| the aprons: `memmove` in `Canvas::copy_top` | **7.10** | 17.20 | 3.46 | 1.25 | 6.50 |
| first touch: `BandData::fit` (a band's planes, new each picture) | 2.48 | 1.60 | 1.84 | 0.80 | 5.68 |
| bands' publishing: futex wake-ups | 2.41 | 8.12 | 0.80 | 0.27 | 0.47 |
| bands' waits: the kernel's side of blocking | 1.92 | 6.23 | 0.69 | 0.32 | 0.44 |
| first touch: predicted blocks writing a new picture | 0.81 | −0.02 | 0.02 | 0.08 | 3.16 |
| other | 1.99 | 5.85 | 1.15 | −0.51 | 1.49 |

So the 8T-only causes, ranked over the 32 (run 1's samples, with run 2's split of libc and the kernel, which is of
14 streams on another VM of the same type, so the sums are approximate):

1. **The band view built for every block: 11.3%** (4k 18.4). The largest; probe PV below. `viewpath.py` counts
   `predict_fast_body`'s lines 1295 to 1310, which include the fallback call to `predict_list_any`, so this may
   overlap item 6 slightly.
2. **The aprons: about 7.9%** (`copy_top` 0.82 and its `memmove` 7.10; sd about 19.7, 4k 6.7). Copying the
   aprons twice cost 1.008 on the development VM (`../band-views/probe.txt`), where the bands above were mostly in the
   same core's cache; with eight cores the rows are probably read from another core's cache (not measured).
3. **Bands' publishing and waits: about 6.0%** (sd 19.0): the futex wake on every band published (2.41), the
   kernel's side of blocking (1.92) and the user side (1.67). At sd a band is one macroblock row.
4. **The same work slower**: the inter stage's growth beyond the view's path (about 4.4, mostly cycles at the same
   instructions) and the MB layer's cycles at the same instructions (2.2), probably reads of references and motion
   that another core wrote (not measured).
5. **First touch: about 3.3%** (4k 8.8): each band's planes are allocated new for each picture (`BandData::fit`), and
   at 4k the picture's own pages are first written by the predicted blocks.
6. **Windows that leave a band: 2.4%** (sd 5.7).

## Probe PV (`plan-probe.md`, `pv-run1/`)

`pv2` (`probe/r37-view2`, `dc0f1d7c`) calls `reference_band` once more where the reference is banded, from opaque
inputs, and drops that view; it does not double the whole-picture lookup. Byte-identical to `base` on the 32
streams at 1 and 8 threads (`identity-probe.txt`). One run, not void: the reference moved −0.01% between the first
call and the last, no stream unresolved, the null arm's deviation at most 0.8% (discard) and 1.4% (write), its median
+0.09% and −0.07% (`pv-run1/verdict.json`, `pv-run1/pv-summary.txt`).

| `pv2` / `base`, eight threads | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| discard | **1.0152** [1.0109, 1.0194], 0 of 32 faster | 1.0127 | 1.0158 | 1.0149 | 1.0341 |
| write | 1.0117 [1.0081, 1.0147] | 1.0132 | 1.0084 | 1.0106 | 1.0139 |

**PV-1 is met** (at least 1.015), at its threshold: the second view costs 1.5% of the eight-thread wall time, 3.4%
at 4k. A round that removes most of the path opens; its design and bars come before its timing. The rejected kept
views (`rejected/h264-band-views`, item D of bundle 2) remove about two thirds of the view's instructions and
keep the lookup and the call for every block, so the round aims at the whole path: a view taken once for the blocks
that read the same band of the same reference, not once for each block.

Against FFmpeg on this VM (`us-east4-a`), eight threads, discard: `base` 0.9174 [0.8526, 0.9546], 26 of 32 faster
(sd 0.964, 720p 0.927, 1080p 0.955, 4k 0.729); `pv2` 0.9288.

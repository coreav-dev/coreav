# The reference hints against FFmpeg's prefetch: stall or issue

Diagnosis only, on an Emerald Rapids VM with a PMU (not the bar machine's class), so nothing here is a speed claim or
moves a bar. The question: CoreAV's `prefetch::for_rows` takes 2 to 3 times the cycles of FFmpeg's `ff_prefetch_*`
per macroblock (the PMU profile at `dd85b381`, an earlier change's slowest-streams analysis). Earlier rounds judged that time to be
memory wait moved ahead of the reads (design note, "Motion round: fewer or cheaper requests for the reference's
lines"; `../request/`). If part of it is instruction or issue overhead instead, a cut there could show in time.

## Before the run (free, from the `ipc` records of the PMU profile at `dd85b381`)

`pf_stalls.py` splits `for_rows` by its inlined caller: `rows_for_read` (the reference's lines, the counterpart of
FFmpeg's `ff_prefetch_*`) and `rows_for_write` (the output macroblock's rows, which FFmpeg has no function of its own
for). Per macroblock, mean over each class's eight streams, two rounds:

| class | read: cycles | instructions | write: cycles | instructions | FFmpeg `ff_prefetch_*`: cycles | instructions |
|---|---|---|---|---|---|---|
| 720p | 15.1 | 41.0 | 17.1 | 31.0 | 12.5 | 41.3 |
| 1080p | 27.3 | 70.0 | 21.0 | 42.7 | 16.5 | 52.5 |
| 4k | 22.9 | 62.7 | 16.9 | 28.7 | 15.3 | 57.5 |

The reference hints issue about FFmpeg's instructions (1.33 times at 1080p) and take 1.2 to 1.7 times its cycles; the
output hints are a cost FFmpeg shows no function for, at an IPC of 1.7 to 2.0.

## Method

- Host and FFmpeg as `../profile-acf3ee2f-pmu/`: Compute Engine `c4-standard-8`, SPOT, PMU `STANDARD`, made, run and
  deleted by `scripts/gcp_vm_job.py`; the attribution build of FFmpeg 9.0.2 (function symbols only, no debug
  information) built on the VM by `job.sh` with make's log deleted unread and the source tree, tarball and log deleted
  right after `make`, before anything is profiled. FFmpeg is read at function level only.
- CoreAV `main` at `bb7255cd`, `dist` with line tables (`CARGO_PROFILE_DIST_DEBUG=line-tables-only`), `--example
  decode`, named `prof-bb7255cd`.
- The 24 training streams of the 720p, 1080p and 4k classes (the sd class issues no hints) at the class frame counts,
  one thread pinned to CPU 3, two rounds. `rec_pf.py` samples two groups, each with at most three programmable events
  beside the fixed counters (four programmable counters a thread at least, so none should be multiplexed; the run
kept no per-event running time to show it): `stall` (cycles, instructions,
  `cycle_activity.stalls_total`, `cycle_activity.stalls_l1d_miss`, `l1d_pend_miss.fb_full`) and `swpf` (cycles,
  `sw_prefetch_access.t0`, `load_hit_prefetch.swpf`, `exe_activity.bound_on_loads`), 2,000 samples a second an event.
- `pf_stalls.py`: per class, the events of `for_rows` under `rows_for_read` (R) and under `rows_for_write` (W), and of
  FFmpeg's `ff_prefetch_*` (F), per macroblock, each stream counting equally.

## Rule, fixed before the run

For each class c of 720p, 1080p and 4k, with cycles C, stall cycles S (`cycle_activity.stalls_total`) and the issue
part N = C − S of each side:

- X = N_R + N_W − N_F, the issue part's excess over FFmpeg's (the output hints count whole: FFmpeg shows no function
  for them);
- Y = S_R + S_W − S_F, the wait part's excess;
- T = 0.75% of CoreAV's cycles per macroblock in c: an earlier change's plain-instruction threshold (1.5% of a stream's
  instructions) at its rate of about half reaching time.

If X ≥ T in at least one class, the issue part is real: a part-only probe follows (padding `for_rows` with one-uop
instructions, as the decision probe did), its bars committed before its run. Otherwise the excess is wait, and a
short note in the design note closes the question. `fb_full` (fill buffers, B), `sw_prefetch_access.t0` (hints
issued) and `load_hit_prefetch.swpf` (demand loads that found a hint still in flight, so the hint was late) are
reported beside the rule and decide nothing.

Limits: a sampled event lands on the instruction the core was at when the counter overflowed, give or take a few
(skid), so a hint's stall can be one a nearby load caused; FFmpeg's inlined caller of `ff_prefetch_*` is counted in
its caller, not in F.

Spend cap for the run: 1 USD (`--max-hours 1`, the script's bound 0.60 USD an hour).

## Results

Run on one VM (`run/gcp-job.log`: 0.152 hours, bound 0.091 USD). All six events counted
(`run/events.json`); the attribution build has no `.debug_*` section and decodes as the BtbN build does
(`run/ffattr-sections.txt`, `run/ffattr-output.txt`, `run/build-order.txt`). `run/pf_stalls.txt` holds every event;
per macroblock, mean over each class's eight streams:

| class | side | cycles | instructions | stall cycles S | issue part N | fill buffer full B |
|---|---|---|---|---|---|---|
| 720p | read (R) | 16.12 | 46.6 | 4.10 | 12.02 | 3.80 |
| | write (W) | 14.16 | 32.9 | 5.68 | 8.49 | 3.02 |
| | FFmpeg (F) | 15.08 | 49.1 | 1.04 | 14.04 | 0.06 |
| 1080p | R | 28.00 | 70.8 | 6.58 | 21.42 | 6.59 |
| | W | 20.75 | 42.3 | 8.39 | 12.36 | 6.08 |
| | F | 16.99 | 45.6 | 1.54 | 15.45 | 0.51 |
| 4k | R | 20.80 | 67.5 | 4.57 | 16.23 | 3.50 |
| | W | 14.06 | 34.3 | 5.19 | 8.88 | 1.96 |
| | F | 14.82 | 67.7 | 1.72 | 13.10 | 0.59 |

The rule:

| class | X | Y | T | X ≥ T |
|---|---|---|---|---|
| 720p | 6.46 | 8.73 | 16.11 | no |
| 1080p | 18.33 | 13.43 | 25.55 | no |
| 4k | 12.01 | 8.04 | 9.64 | **yes** |

So the probe follows, for the 4k class. Readings beside the rule:

- Most of a hint's cycles are not stall cycles on either side: S is 0.22 to 0.25 of the reference hints' cycles,
  0.37 to 0.40 of the output hints', and 0.07 to 0.12 of FFmpeg's. Most of the time is spent while other work
  executes; that can still be waiting, for a fill buffer while later instructions run (next point).
- CoreAV's hints find the fill buffers full far more often than FFmpeg's (B 3.5 to 6.6 against 0.06 to 0.59 a
  macroblock for the reference hints, and the output hints add as much again), with as many hints issued
  (`sw_prefetch_access.t0` over the whole decode: 8.4, 14.6 and 14.6 a macroblock against FFmpeg's 12.6, 13.6 and
  13.4). Fill-buffer waits that overlap other work count in N, not S.
- The reference hints take about FFmpeg's instructions at 720p and 4k (67.5 against 67.7 at 4k; 1.55 times at
  1080p); at 4k the excess in X is the output hints (N_W, which FFmpeg has no function for) and about 3 cycles of the
  reference hints' issue part.
- Over the whole decode, demand loads that found a software prefetch still in flight are 0.00 to 0.01 a macroblock
  on both sides, so no hint is late. (Events sampled per instruction land a few instructions late, so the per-function
  `sw_prefetch_access.t0` figures in `run/pf_stalls.txt` undercount the hints; only the whole-decode ones are used.)

`pf_stalls.py` as committed before the run summed `cycles` over the two groups that sample it (the only event in
both), doubling C but not S; it was fixed to average them before this section was written. With the doubled C, N
read 2C − S, so that script's test was 2X + Y ≥ 2T, that is X + Y/2 ≥ T: 720p 10.8 against 16.1, 1080p 25.05 against
25.55, 4k 16.0 against 9.6. The verdicts are the same either way, 1080p within 2% of the line in that reading.

## The probe, fixed before it is built or timed

Question: do the hints' instructions cost time as other instructions do, so that a walk with fewer of them gains at
4k? Both walks take 7 instructions a hint in `prof-bb7255cd`'s machine code: the hint, the pointer's add, and 5 for
the per-row bound check and the loop (`sub`/`mov`, `not`, `add`, `cmp`, `jae`, or `dec`, `jne`). The cut in view (C):
a walk that checks the last row's index once and then issues the hints straight from a base and the stride with
scaled addressing, about 1.5 instructions a hint; 5 fewer a hint. (The request round screened a walk of that kind,
`lean`, at an earlier `main`: 4k 1.0088, all 24 1.0005.)

- P (branch `probe/h264-prefetch-pad`, feature `probe-pad` of `coreav-kernels-h264`, not for merge): 5 independent
  one-uop instructions (`lea o, [a + a]`, output unused, as the earlier probes) after every hint `for_rows` issues,
  for both callers: C's predicted cut, added in its place.
- Verified before timing: callgrind instructions of P over base on one 4k stream (30 pictures), expected 5 a hint;
  output identical to base on the 24 hinting streams at the class frame counts (md5).
- Arms: base = `dist` of `bb7255cd` (`--example decode -p coreav-h264`), P = the same tree with `--features
  coreav-kernels-h264/probe-pad`, one toolchain, `RUSTFLAGS` unset. `ab3.py`, 8 rounds, all 32 training streams at
  the class frame counts, one thread pinned to CPU 3, `--discard`, order rotated each round, CPU time; `abset.py`:
  per stream the median of P over base, the bootstrap 95% interval of the median, per class and over all 32. Host: this
  development VM, Intel Xeon @ 2.90 GHz (directional; no gating bar moves).
- Rule: C is built only if P's median over the eight 4k streams is ≥ 1.010 with the interval's lower end above 1.000
  (the bar of the earlier part-only probes). The sd class issues no hints and is the null control: its median should
  read 1.000 ± 0.005, or the run is a host fault and is run once more. Otherwise C is not built, and a short note in
  the design note closes the question.
- If C is built: its local timing with the same method, base and C, after output identity on the 32 streams at 1 and
  8 threads and conformance; a gating run is requested only if the 4k median of C over base is ≤ 0.990 with the upper
  end below 1.000 and the all-32 median ≤ 1.000, with gating bars committed before that run. Otherwise C goes to a
  `rejected/` branch as a bundle candidate.

## The probe's result

P as built: `probe/h264-prefetch-pad` at `ad94aa91` (on `bb7255cd`). Instructions (`run/instructions.txt`, callgrind,
`3b1b441e23ae`, 30 pictures): base 1,083.5 million, P 1,130.4 million, +46.9 million (+4.33%), 5 for each of 22
hints a macroblock. Output identical to base on the 24 hinting streams (`run/probe-identity.txt`).

Timing (`run/probe-ab.txt`, `run/probe-ab-summary.txt`, 8 rounds, P over base):

| | median | 95% interval | streams faster |
|---|---|---|---|
| all 32 | 1.0087 | [1.0023, 1.0139] | 9 of 32 |
| sd (null control, no hints) | 0.9980 | [0.9904, 1.0045] | 5 of 8 |
| 720p | 1.0087 | [1.0023, 1.0193] | 1 of 8 |
| 1080p | 1.0099 | [0.9952, 1.0139] | 2 of 8 |
| **4k** | **1.0209** | **[1.0075, 1.0335]** | 1 of 8 |

The null control reads within 1.000 ± 0.005 and the 4k median is above 1.010 with the lower end above 1.000: the
rule is met, and C is built.

## C, built and timed

C as built: `rejected/h264-prefetch-straight` at `2968bd3a` (on `main` at `4f118676`, whose decoder's code is
`bb7255cd`'s: base is the same binary). `for_rows` checks the last row's index once; when it lies inside the plane,
16, 8, 4 and 2 rows are hinted straight from the first row's address with no check a row (about 1.7 instructions a
hint in the machine code, against 7); other counts, and rows past the plane, keep the walk. The hint takes the
address as a pointer, so the prefetch site in `simd` changes its argument (`docs/unsafe-sites.md` updated on that
branch). Instructions: 1,051.1 million on `3b1b441e23ae`, −32.4 million (−3.0%), about 3.5 a hint, 69% of the
padding. Output identical to base on the 32 training streams at 1 and 8 threads (`run/c-identity.txt`, 64 of 64);
conformance 97 of 97 with detected kernels at 1, 2 and 8 threads, with scalar kernels, and without `fast`
(`run/c-conformance.txt`).

Timing (`run/c-ab.txt`, `run/c-ab-summary.txt`, 8 rounds, C over base):

| | median | 95% interval | streams faster |
|---|---|---|---|
| all 32 | 0.9989 | [0.9955, 1.0018] | 19 of 32 |
| sd | 1.0017 | [0.9985, 1.0064] | 2 of 8 |
| 720p | 1.0010 | [0.9869, 1.0049] | 4 of 8 |
| 1080p | 0.9963 | [0.9930, 0.9990] | 7 of 8 |
| **4k** | **0.9938** | **[0.9799, 1.0064]** | 6 of 8 |

The gating condition (4k at most 0.990 with the upper end below 1.000, all 32 at most 1.000) is not met: no gating
run is requested, and C stays on `rejected/h264-prefetch-straight` as a bundle candidate.

Reading: adding 5 instructions a hint cost 2.1% at 4k, and removing 3.5 gave 0.6%, inside the noise. A likely
reason, not measured here: the hints wait for fill buffers (B above), so instructions added among them lengthen that
wait, while instructions removed do not shorten it by as much. The excess over FFmpeg in this function is mostly the
output hints and the fill-buffer waits they share with the reference hints; their count and timing were the request
round's subject, and this run adds no variant to time.

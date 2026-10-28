# H.264 gap to FFmpeg, by stage

Status: Accepted. Measurement only; no decoder change.

The rounds of step 6 pick their targets from our own profiles. This note
puts our decoder and FFmpeg 9.0.2 side by side, stage by stage, on eight
training streams. It covers:
- **time:** `perf`, cpu-clock samples;
- **instructions, simulated cache misses and simulated branch
  mispredicts:** callgrind;
- **the instruction mix,** and the cost of bounds checks.

The questions it answers: where the time gap sits, and whether each part
is instruction volume, memory, or branches.

FFmpeg is a black box here:
- it was built from its release tarball and run;
- only its symbol names were used;
- its source was not opened.

## Setup

- **Machine.** The development VM: Intel Xeon reporting 2.90 GHz, 8 vCPU,
  AVX-512. It has **no hardware PMU**: `perf` records `cpu-clock` only, and
  every cache and branch figure below comes from simulation
  (`evidence/provenance.txt`).
- **Ours.** `main` at 3c47e960, `dist` profile, `examples/decode --frames
  F --discard`.
- **FFmpeg build.**
  - Source: 9.0.2 from the release tarball (SHA-256 `8c385028…`), built
    with gcc 14.
  - Configure line: `evidence/ffmpeg-configure.txt`, the pinned build's
    defaults without its external libraries, none of which an H.264
    decode touches.
  - Symbols are kept.
  - Check against the pinned BtbN `n9.0.2-17-g2a571b6068`: within 2% on
    three streams. On 6da34958296c, two rounds of ten runs read 1.274
    against 1.281 s and 1.235 against 1.213 s.
- **FFmpeg runs.** `-threads 1 -i S -frames:v F -f null -`, measured two
  ways:
  - **FFmpeg SIMD:** its runtime-selected assembly on;
  - **FFmpeg C:** with `-cpuflags 0`. That turns off the selected
    assembly only. Two things stay:
    - code its compiler vectorized, so 14.6% of its executed
      instructions still use vector registers (`evidence/mix.txt`);
    - its inline CABAC assembly, so its entropy stage is not pure C.
- **Pinning.** Every decode is single-threaded and pinned to one core.
  FFmpeg's command line still starts helper threads, all on that core.
- **Drift between sessions.** The check against BtbN and the runs below
  were taken in different sessions on this VM. In those two sessions FFmpeg
  SIMD's time for the same stream differs by 6 to 9% (6da34958296c 1.274
  against 1.162 s, 41e324ebb708 0.837 against 0.777 s). So "within 2%"
  holds within a session, and every ratio below compares runs of one
  session.
- **Streams.** Eight of the training set, at its frame counts. The
  profile, entropy coding and picture types are in
  `evidence/streams-info.txt`:

| Stream | Class | F | Profile | Entropy | B frames in F |
|---|---|---|---|---|---|
| 41e324ebb708 | sd | 1800 | Main | CABAC | 979 |
| 5185721dda11 | sd | 1800 | Baseline | CAVLC | 0 |
| 9db41aa16b5a | 720p | 900 | High | CAVLC | 592 |
| 1d4b1775d78a | 720p | 900 | High | CABAC | 187 |
| 6da34958296c | 1080p | 300 | Main | CABAC | 224 |
| b71b6b31e54f | 1080p | 300 | Main | CAVLC | 145 |
| 3b1b441e23ae | 4k | 150 | High | CABAC | 56, weighted |
| e624cbe0d2be | 4k | 150 | Main | CABAC | 0 |

**Wall time** (`perf stat -r 3`, `evidence/wall.txt`):

| Stream | Ours / FFmpeg SIMD | Ours / FFmpeg C |
|---|---|---|
| 41e324ebb708 | 2.33 | 1.02 |
| 5185721dda11 | 2.08 | 1.07 |
| 9db41aa16b5a | 2.91 | 1.06 |
| 1d4b1775d78a | 2.45 | 1.13 |
| 6da34958296c | 2.49 | 1.30 |
| b71b6b31e54f | 3.00 | 1.36 |
| 3b1b441e23ae | 2.14 | 1.25 |
| e624cbe0d2be | 1.99 | 1.34 |

## Stages

Each function is mapped to a stage by name (`tools/stages.py`): ours by
module and function, FFmpeg by symbol name. A name no rule matches goes to
`other`. What remains there:
- kernel and libc time (page faults, scheduling, allocation);
- `memset`, `memcpy` and `memmove` on both sides, since FFmpeg's are in
  libc where its symbols cannot name them;
- a few decoder functions outside every stage: our `decode_unit`, FFmpeg's
  start-code search.

The largest entries are in `evidence/stages.json`.

The table sums the eight streams:
- **ms:** `perf` samples at 999 Hz, about 1 ms each;
- **Counts:** instructions, simulated L1 data and last-level misses, and
  simulated mispredicts (conditional and indirect), all from callgrind.

The last-level cache is the host's 56 MiB, so LL misses are rare on both
sides. Per stream and per function: `evidence/stages.json`,
`evidence/functions.json` and `evidence/perf/`.

| Stage | ms ours | ms FFmpeg C | ms FFmpeg SIMD | ours/C | ours/SIMD | gap to SIMD (ms) | G instructions ours / C / SIMD | D1 misses (M) ours / C / SIMD | LL misses (M) ours / C / SIMD | mispredicts (M) ours / C / SIMD |
|---|---|---|---|---|---|---|---|---|---|---|
| entropy | 1614 | 1621 | 1520 | 1.00 | 1.06 | +94 | 12.0 / 9.6 / 9.6 | 13 / 50 / 50 | 0.0 / 0.0 / 0.0 | 98 / 61 / 56 |
| mb control | 3141 | 1096 | 1052 | 2.87 | 2.99 | **+2089** | 31.3 / 11.8 / 12.2 | 156 / 17 / 23 | 14.4 / 0.0 / 0.0 | 129 / 44 / 43 |
| luma MC | 1617 | 1946 | 465 | 0.83 | 3.48 | +1152 | 14.9 / 21.1 / 3.9 | 135 / 147 / 145 | 10.9 / 0.3 / 0.0 | 41 / 18 / 5 |
| chroma MC | 1661 | 829 | 222 | 2.00 | 7.48 | **+1439** | 19.7 / 8.6 / 1.6 | 73 / 75 / 69 | 4.9 / 0.2 / 0.0 | 46 / 6 / 4 |
| combine | 569 | 867 | 113 | 0.66 | 5.04 | +456 | 8.2 / 9.6 / 1.1 | 11 / 0 / 0 | 0.0 / 0.0 / 0.0 | 18 / 4 / 4 |
| idct/dequant/add | 600 | 618 | 137 | 0.97 | 4.38 | +463 | 6.5 / 5.6 / 1.2 | 4 / 1 / 1 | 0.0 / 0.0 / 0.0 | 18 / 33 / 4 |
| intra pred | 417 | 90 | 73 | 4.63 | 5.71 | +344 | 5.6 / 0.7 / 0.3 | 13 / 10 / 10 | 1.0 / 0.0 / 0.0 | 26 / 0 / 1 |
| deblock filter | 1878 | 2644 | 601 | 0.71 | 3.12 | **+1277** | 22.8 / 33.5 / 5.1 | 43 / 30 / 30 | 0.0 / 0.0 / 0.0 | 68 / 126 / 0 |
| deblock strength/driver | 1025 | 984 | 680 | 1.04 | 1.51 | +345 | 11.0 / 9.8 / 8.9 | 70 / 47 / 48 | 0.0 / 0.0 / 0.0 | 52 / 43 / 19 |
| picture/edge/output | 286 | 27 | 129 | 10.59 | 2.22 | +157 | 1.3 / 0.2 / 0.9 | 45 / 1 / 1 | 0.8 / 0.0 / 0.0 | 4 / 2 / 9 |
| other | 334 | 478 | 400 | 0.70 | 0.83 | -66 | 1.6 / 1.3 / 1.2 | 8 / 19 / 18 | 2.7 / 4.5 / 4.0 | 4 / 12 / 10 |
| **total** | 13142 | 11200 | 5392 | 1.17 | 2.44 | +7750 | 135.1 / 111.7 / 46.0 | 570 / 397 / 395 | 34.8 / 5.0 / 4.0 | 505 / 350 / 157 |

**How the stages map.** Each mapping is by name only:
- **MB control:**
  - ours: the slice decoder's macroblock functions (prediction setup,
    motion storage, direct mode, neighbours);
  - FFmpeg: the macroblock reconstruction driver, the decode caches, the
    per-partition motion compensation driver and direct-mode prediction.
- **Deblock strength/driver:** our deblocking module, and FFmpeg's
  per-macroblock filter driver and its strength check.
- **The macroblock-level stages are not mapped symmetrically.** Two work
  items sit on different sides of the stage line in the two decoders:
  - **Motion-vector work.** FFmpeg's binary has no symbol for motion-vector
    prediction or motion write-back besides direct mode. So that work is
    inlined into its macroblock parsing functions, which count as entropy,
    while ours (`mv_predictor`, `neighbour_motion`, `store_motion`, about
    0.7 s) count as MB control.
  - **CAVLC macroblock parsing.** Ours is inlined into
    `PictureWork::decode_slice`, MB control, up to 175 ms on the CAVLC
    streams. FFmpeg's has a symbol of its own, counted as entropy.

  In the other direction, FFmpeg's macroblock driver holds its intra and
  residual driving, while our counterparts count as intra pred and
  idct. The four stages are therefore also reported together:
  - **all macroblock-level work** (entropy, MB control, intra pred,
    idct/dequant/add): ours 5,772 ms against FFmpeg C's 3,425 and FFmpeg
    SIMD's 2,782, that is **+2.99 s**;
  - **instructions:** 55.4 G against 27.6 G and 23.3 G.

  That total is the robust figure; the split between entropy and MB
  control is not.

## Where the gap is

Ours takes 13.1 s to FFmpeg SIMD's 5.4 s on these streams. Three stages
hold most of the 7.75 s gap:

1. **Macroblock-level work, +2.99 s (39%).** This is entropy, MB control,
   intra and idct together, since their split is not symmetric (above).
   - **Instructions:** 55.4 G against 23.3 G (2.4 times) and against
     FFmpeg C's 27.6 G (2.0 times).
   - **Mispredicts:** 271 M against 105 M.
   - **D1 misses:** 186 M against 84 M.

   Most of the gap remains against FFmpeg's C build (1.69 times its
   sampled time, against 2.07 times SIMD's; 2.0 against 2.4 times the
   instructions). So the gap is **instruction volume**, with **branches** and **D1
   traffic** of the same order, not missing SIMD. MB control alone holds
   +2.09 s of it.
2. **Chroma MC, +1.44 s (19%).**
   - **Against FFmpeg SIMD:** 12 times the instructions (19.7 G against
     1.6 G) and 11 times the mispredicts, but the same D1 misses (73
     against 69 M): both read the same reference samples.
   - **Against FFmpeg C:** 2.3 times the instructions and 2.0 times the
     sampled time.

   So it is **instruction volume and branches**, not memory: our
   fractional chroma path is scalar (`CHROMA_MC_VECTOR = false`).
3. **Deblock filtering, +1.28 s (16%).** 4.5 times FFmpeg SIMD's
   instructions (22.8 G against 5.1 G), with 68 M mispredicts against
   FFmpeg's assembly filters' about 0. So it is **instruction volume and
   branches**. FFmpeg's C filters take more sampled time than ours (ratio
   0.71).

**The other large gaps.**
- **Luma MC, +1.15 s:** 3.9 times FFmpeg SIMD's instructions with the
  same D1 misses. Its sampled time is 0.83 of FFmpeg C's.
- **Deblock strength and driver, +0.35 s.**

**Last-level misses.** Ours has 35 M simulated last-level misses against
FFmpeg's 4 to 5 M, but they are almost all in the two 4k streams:

| Stream | Ours | FFmpeg SIMD |
|---|---|---|
| e624cbe0d2be | 28.8 M | 1.3 M |
| 3b1b441e23ae | 4.3 M | 0.9 M |

On the other six streams ours has as many as FFmpeg or fewer. They are
nearly all **write** misses, in prediction (`inter_pred`, `luma_mc_into`,
`chroma_mc`) and motion storage (`store_motion`), and our profiles of
those streams show kernel page-fault time. That fits first writes into
freshly allocated pictures or motion arrays at 4k. FFmpeg's misses fall
in `memset`, not in its decoding code. This reading is inference; the
simulation counts misses, not which buffer they hit.

## Bounds checks and the instruction mix

- **Panicking bounds checks are absent from the hot code.**
  `tools/bounds.py` finds 112 conditional jumps to a bounds panic in the
  binary. Four are in decoder code, all in the reference list sort; the
  rest are in the standard library. Executed: 84 to 87 times per stream, in
  6.8 G to 27.5 G instructions. That follows from the crate's ban on
  indexing: the decoder checks bounds with `get()` and a fallback, which
  branch to no panic and which no tool here can separate from other
  branches.
- **Where such checks would show: the compares and branches** (`evidence/mix.txt`, executed instructions, eight streams):

| | Instructions | Compare/test | Conditional branch | Vector register |
|---|---|---|---|---|
| ours | 135.1 G | 13.1% (17.7 G) | 10.0% (13.5 G) | 11.8% |
| FFmpeg C | 111.7 G | 7.9% (8.8 G) | 5.4% (6.0 G) | 14.6% |
| FFmpeg SIMD | 46.0 G | 5.6% (2.6 G) | 6.3% (2.9 G) | 18.0% |

Against FFmpeg C, ours executes 23.4 G more instructions. 16.4 G of that
difference is compares and conditional branches, so **most of what we do
beyond FFmpeg's C code is deciding, not computing**. Bounds checks by
`get()` are one source of those decisions, and control flow is the other.

**What bounds checks cost is not measured.** Separating them needs a
build that removes the checks (`-Zbuild-std` with unchecked accesses),
which this note did not attempt. The 16.4 G is only an indication, by
inference:
- `get()` fallbacks also cost instructions that are neither compares nor
  branches (`lea`, `cmov`);
- not every compare and branch of FFmpeg's has a counterpart in ours;
- instructions are not cycles.

FFmpeg SIMD's row understates its shares: 7.5% of its executed
instructions sit at addresses the disassembly does not list, inside its
hand-written functions, and are left unclassified.

## What it suggests

These are inferences from the table, for the rounds to measure:
- **Macroblock-level work** is the largest gap. Most of it remains against
  C, and it is branchy and touches the D1 cache widely. So
  it is a question of what per-macroblock state is kept, and how many
  decisions derive it, not of vector code.
- **Chroma MC at fractional positions** has a 12-fold instruction gap with
  the same memory traffic. A vector path that does not pay a call per
  small block may help: both planes per call, or more rows per call. The
  earlier one lost on call overhead.
- **Deblock filtering** has 4.5 times FFmpeg SIMD's instructions and many
  mispredicts where FFmpeg's assembly has almost none, though our filters
  are already vector. That points to branching in our kernels or around
  them.
- **The 4k write misses** (28.8 M on one stream) are worth a check of
  whether pictures and motion arrays are reused between pictures at 4k.

## Reproducing

`tools/` holds what produced the tables:
- `run.sh`: the runs (`perf`, `perf stat` and callgrind, with the exact
  commands);
- `stages.py`: the stage mapping;
- `cg.py`: per-function self costs from callgrind, with assembly
  attributed by address;
- `report.py`: the per-stage sums;
- `mix.py`: the instruction mix;
- `bounds.py`: the bounds-check guards.

The raw `perf.data` and `callgrind.out` files are not committed: 40 MB,
and FFmpeg's include its symbol names only. The per-function summaries
are.

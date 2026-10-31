# The packet index and the HLS index against libavformat, after an earlier change

Throwaway measurement code, outside the Cargo workspace. It
measures the packet index and the HLS index against libavformat opening
the same file in process, after the sample walk of
[`docs/design/sample-tables.md`](../../design/sample-tables.md), and
against the commit the branch merges into, on a held-out draw no number
has come from.

## Hypothesis and bars

Fixed before the draw was collected and before the run, in
`docs/design/sample-tables.md` ("Bars") and `docs/design/hls.md` ("Index
against FFmpeg a third time"):

- **Against libavformat**, over the inputs both engines
  open: the median of the per-input ratios is at most **1.00** overall and
  in each class of five inputs or more, for the packet index (A-index /
  B-lean) and for the HLS index (A-hls / B-lean). "Faster" is said only
  where the upper bound of the bootstrap 95% interval is below 1.00. A
  class with fewer than five inputs is reported without a verdict. The
  HLS index's `group-a` class is expected to miss again; a miss is recorded
  with its cause and the bar does not change.
- **Against the base commit**, paired on the same inputs in the same run:
  the median of new / base is at most **0.60** for the packet index in
  `group-a`, **0.85** for the packet index overall, **0.90** for the HLS
  index in `group-a`, and **1.00** for either engine overall and in every
  class. The two binaries differ in code layout, so a class median
  between 1.00 and 1.02 is reported as unchanged rather than as a
  regression; above 1.02 is a miss.
- **Correctness:** the A-index digest and what `coreav hls index` writes
  are identical between the two builds on **every** file of the draw,
  including the files either refuses.
- **Share:** every median is reported with the share of inputs CoreAV is
  faster on, meaning a ratio below 1.00.

## Sets

Corpus **v2-heldout**, whose rules, classes and
salts were committed before each draw was collected. Every `real` file of
a draw is held out, and a draw is spent once a number comes from it.

- **Draw 1** (40 files) and **draw 2** (46 files) are spent: run `w3-01`
  of `docs/spikes/w2-hls-index/` reported an earlier change's result from draw 2, for the HLS
  index and the packet index both. (That spike's runs are numbered
  `w2-01` to `w2-04` and `w3-01`; this one's carries on at `w4-01`.)
  They are tuning pools, and the profile and the tuning behind an earlier change used
  draw 1.
- **Draw 3** is the set of this run. No file of the earlier draws is in
  it, and no structural signature repeats within it. Skipping a
  signature already in an earlier draw as well was tried and withdrawn
  before any file of draw 3 was measured: it left the `group-a` class with
  two files ([`docs/corpus.md`](../../corpus.md#rules-withdrawn-or-not-held), and an earlier change).

## Method

`w3.py`, an earlier change's driver with five engines instead
of three:

- **A-index:** `probe-bench --index --io=mmap`, probe plus the packet
  index, built from this branch.
- **A-hls:** `probe-bench --hls --io=mmap`, that is
  `coreav_isobmff_write::hls::build_index` with `coreav hls index`'s
  defaults, with the index held until the request ends, built from this
  branch.
- **A-index-base** and **A-hls-base:** the same two, built from the commit
  the branch merges into, with `evidence/base-digest.patch` applied — the
  `--digest` option and nothing else, since the digest that carries the
  correctness bar is this branch's addition to the spike and the base
  would otherwise print none. `w3.py` refuses an engine whose usage does
  not take `--digest`, so a base without the patch cannot make the bar
  read as met.
- **B-lean:** `probe-libav --format-mov --lean --io=mmap --buffer=4096`,
  the pinned LGPL libavformat 7.0.2 opening the file.

Each engine is one long-lived process; the five are pinned to one core and
serve one request at a time, and the driver runs on another core. After
one warm-up pass come 15 rounds, with file and engine order shuffled. The
index check against `ffprobe -count_packets`, and the two digests that
carry the correctness bar, are untimed. Everything runs on the development
machine, with nothing else running. `--local` takes the files from the
directory they were collected into, verified by SHA-256, so the set is
measured before it is uploaded.

`summarize.py RUN_DIR` computes the comparison.

## Results

**Run `w4-01` on draw 3 is the result.** It ran from a clean tree on the
crates of `fdde743`, against the commit the branch merges into
(`8d1b0b7`), over the 46 `real` files of draw 3 (2.96 GB), **60 rounds**
after a warm-up, five engines pinned to one core of an 8 vCPU Intel Xeon
at 2.90 GHz. The files were measured from the directory they were
collected into, verified by SHA-256, before they were uploaded.
Evidence: `evidence/w4-01.tar.xz` and `evidence/summary-w4-01.json`.

**Rounds, and what the measurement repeats to.** The method's 15 rounds
settle every ratio here but one: the *change* against the base on files
where the walk is a small part of a request, where the two engines are
within a tenth of each other and a run moves as a whole. Three 15-round
runs on these binaries and this set gave an overall packet index change
of 0.828, 0.812 and 0.836, against a bar of 0.85
(`evidence/repeats-15-rounds.json`). Run `w4-01` is therefore 60 rounds,
which measures the same thing with less of that noise; the ratios
against libavformat move by about 0.02 across all four runs, and the
change in the `group-a` class, which is what the bar is about, by 0.09 —
still four times clear of its bar in the worst of them.

Each ratio is over the inputs its own engines open, so each carries its
own input count. `n` is that count; a class under five inputs is reported
without a verdict.

### Correctness: met

The packet index's entries and what `coreav hls index` writes are
**identical between the two builds on all 46 files** — every entry of
every track, and the index's bytes with the command's exit status,
including the files either build refuses. Nothing differs and nothing
went uncompared.

### Against libavformat

| Class | n | Packet index / libavformat | n | `hls index` / libavformat |
|---|---:|---|---:|---|
| **All** | 42 | **0.269** (0.257 to 0.306) | 32 | **0.622** (0.479 to 0.809) |
| group-a | 9 | 0.352 (0.332 to 0.383) | 9 | **1.547** (1.311 to 1.925) |
| group-c | 20 | 0.271 (0.235 to 0.312) | 13 | 0.463 (0.365 to 0.692) |
| group-g | 11 | 0.261 (0.252 to 0.269) | 8 | 0.479 (0.467 to 0.674) |
| group-e | 2 | 0.216 (no verdict) | 2 | 0.589 (no verdict) |

- **The packet index meets the bar overall and in every class**, and is
  *faster* than libavformat's open in each of them: every interval's
  upper bound is below 1.00. CoreAV is faster on **42 of the 42 inputs**
  (100%).
- **The HLS index meets the bar overall and in `group-c` and `group-g`, and
  misses in `group-a`** at 1.547. CoreAV is faster on **23 of the 32
  inputs** (72%); the nine it is slower on are the eight `group-a` inputs
  and one `group-g` input at 1.080. The miss is recorded with its cause
  below; the bar does not change (`docs/design/hls.md`).
- The same two engines **built from the base commit**, on the same
  inputs in the same run: packet index 0.344 overall (0.270 to 0.394)
  with `group-a` at 0.850 (0.761 to **1.033**), faster on 40 of 42; `hls
  index` 0.728 overall (0.495 to 0.850) with `group-a` at 1.991 (1.729 to
  2.281), faster on 23 of 32. On the base the packet index's `group-a`
  interval reaches above 1.00, so "faster than libavformat" did not hold
  for that class before this change and does now.

### Against the base commit, paired on the same inputs

| Class | n | Packet index new / base | n | `hls index` new / base |
|---|---:|---|---:|---|
| **All** | 42 | **0.833** (0.734 to 0.916) | 32 | **0.907** (0.852 to 0.935) |
| group-a | 9 | **0.412** (0.374 to 0.436) | 9 | **0.805** (0.758 to 0.891) |
| group-c | 20 | 0.855 (0.781 to 0.916) | 13 | 0.926 (0.901 to 0.952) |
| group-g | 11 | 1.003 (0.834 to 1.060) | 8 | 0.937 (0.865 to 0.977) |
| group-e | 2 | 0.623 (no verdict) | 2 | 0.808 (no verdict) |

**Both bars are met.** The packet index is at most 0.85 overall (0.833)
and at most 0.60 in `group-a` (0.412); the HLS index is at most 1.00
overall (0.907) and at most 0.90 in `group-a` (0.805), and no class of it
passes 1.00. The packet index's `group-g` class is at **1.003**, inside the
band the bar allows a class for the two binaries' code layout (1.00 to
1.02), and is reported as unchanged rather than as a regression: six of
those eleven inputs are read from a segment index, so the index is built
without walking a sample and there is nothing in them for this change to
move. The packet index is faster on 35 of the 42 inputs and the HLS
index on 30 of the 32; the inputs above 1.00 are seven (up to 1.099) and
two (up to 1.040).

The overall numbers are the part of this table the measurement holds
least tightly: 0.833 sits 0.017 under its bar, and three 15-round runs
put it between 0.812 and 0.836. The number the change is about, `group-a`
at 0.412 against a bar of 0.60, is clear of its bar by far more than
that in every run.

The gain follows the share of a request the sample walk is. On `group-a`,
where the tables are one AAC track and the walk is nearly the whole
request, the packet index takes 0.412 of what it took; on `group-g`, where
six of eleven inputs are read from a segment index, 1.003.

### What the index build costs over the walk

The HLS index takes **2.240** times what listing the same samples with
the packet index takes (median; `group-a` 4.370, `group-e` 2.726, `group-c`
2.010, `group-g` 1.914). Draw 2 measured 1.886 overall and 2.159 on `group-a`
for the same `build_index`: the ratio grew because the walk under it got
faster, not because the index does more. What is left in it is the
index's own work — the segment boundaries, the per-sample records and
the serialized index — which libavformat's open does not do, and which
is now most of the request.

### Why group-a still misses, and by how much less

The cause is unchanged and is now smaller. libavformat has little to
lose on an audio-only file, so its open is the cheapest it ever is
relative to CoreAV's walk, and an index build costing about four times
that walk lands above 1. The change moved the class from 1.991 to
1.547 — 22% off the miss — by taking the walk out from under it; closing
the rest means the index's own per-sample record, which is an earlier change's
remaining work and not this change's.

### What the draw is, and what it is not

46 `real` files against a target of 60 to 100: `group-a` 13, `group-c` 20,
`group-g` 11, `group-e` 2 and **`group-b` 0**. `group-e` and `group-b` carry no
verdict, as `group-b` did not on draw 2. The caps bind, and they bind
hardest on the small groups ([`docs/corpus.md`](../../corpus.md#draw-3)).

Of the 46, **four are refused by `coreav probe`** and so carry no packet
index: four audiobooks, each with a top-level box that runs past the end
of the file at exactly the offset CoreAV reports, which the pinned
`ffprobe` reads. That is `KD-STRICT` (`docs/known-divergences.md`),
checked here by walking the top-level boxes of each file: a covered
divergence, not a new one. Draw 2 had three of the same shape. `coreav
hls index` refuses fourteen inputs: those four, seven `group-c` files and
three 1.2 KB `group-g` fragments.

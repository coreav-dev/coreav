# The HLS index against libavformat, after an earlier change

Throwaway measurement code, outside the Cargo workspace. It
re-measures the HLS index build against libavformat opening the same file
in process, after the speed-up of an earlier change, on a held-out set that
`docs/spikes/w1-remeasure/`'s run `w1-01` did not use.

## Hypothesis and bar

Fixed before the set was collected and before the run, in
`docs/design/hls.md`, "Index against FFmpeg again": the median of
the per-input ratios A-hls / B-lean is at most **1.00** overall and in
each class, over the inputs both engines open. "Faster" is said only
where the upper bound of the bootstrap 95% interval is below 1.00. A
class with fewer than five inputs is reported without a verdict.

## Sets

Corpus **v2-heldout**, whose rules, classes and
salts were committed before each draw was collected. Every `real` file of
a draw is held out, and a draw is spent once a number comes from it.

- **Draw 1**, 40 files, carried runs `w2-01` to `w2-04`. Its permission
  rule left `group-e` empty and `group-b` at one file. It is spent, and a
  tuning pool from now on.
- **Draw 2**, 46 files, is the set of run **`w3-01`**, collected under
  a wider permission basis, which
  filled `group-e` and gave five `group-g` inputs
  with a verdict.

## Method

`w2.py`, an earlier change's driver reading the v2-heldout
manifest, with the same three engines:

- **A-index:** `probe-bench --index --io=mmap`, probe plus the packet
  index.
- **A-hls:** `probe-bench --hls --io=mmap`, that is
  `coreav_isobmff_write::hls::build_index` with `coreav hls index`'s
  defaults, with the index held until the request ends.
- **B-lean:** `probe-libav --format-mov --lean --io=mmap --buffer=4096`,
  the pinned LGPL libavformat 7.0.2 opening the file.

Each engine is one long-lived process; the three are pinned to one core
and serve one request at a time, and the driver runs on another core.
After one warm-up pass come 15 rounds, with file and engine order
shuffled. The index check against `ffprobe -count_packets` is untimed.
Everything runs on the development machine (8 vCPU), with nothing else
running. `--local` takes the files from the directory they were collected
into, verified by SHA-256, so the set is measured before it is uploaded.

`summarize.py RUN_DIR` computes the comparison.

## Results

**Run `w3-01` on draw 2 is the result.** Runs `w2-01` to `w2-04` measured
draw 1 as the branch's code settled, and `w2-04`, on its final binary,
was reported as an earlier change's result and merged as such; it stays below as the
record of that draw, since the two draws measured the same
`build_index`. Both ran from a clean tree, `w3-01` on the crates of the
merged `63812e4` (recorded as `92d0e86`, which adds the corpus and the
collection scripts and gives `w2.py` the filter that skips a spent
draw). 41 of draw 2's 46 inputs are compared: the other five are inputs
`hls index` refuses, listed in the summary — two with non-AAC audio, and
three audiobooks with a box that runs past the end of the file.
Those three are refused by `coreav probe` as well, with the same error,
while the pinned `ffprobe` reads all three and each matches its source's
`Content-Length`: a parser divergence against the oracle on 3 of 46
files, and the reason the packet index below is measured over 43 inputs
rather than 46. Evidence: `evidence/w3-01.tar.xz` and
`evidence/summary-w3-01.json`, with the draw-1 runs' beside them.

Each ratio is over the inputs its own engines open, so each carries its
own input count.

| Class | n | `hls index` / libavformat, median (95% CI) | n | Packet index / libavformat | n | Index / packet index |
|---|---:|---|---:|---|---:|---|
| **All** | 41 | **0.613** (0.497 to 0.752) | 43 | 0.330 | 41 | 1.886 |
| group-a | 12 | **1.788** (1.028 to 2.268) | 12 | 0.833 | 12 | 2.159 |
| group-b | 1 | 0.271 | 2 | 0.201 | 1 | 1.537 |
| group-c | 17 | 0.613 (0.419 to 0.704) | 18 | 0.334 | 17 | 1.842 |
| group-e | 6 | 0.465 (0.301 to 0.654) | 6 | 0.259 | 6 | 1.769 |
| group-g | 5 | 0.500 (0.475 to 0.522) | 5 | 0.264 | 5 | 1.832 |

Draw 1, on the same `build_index` (run `w2-04`): all **0.902** (0.632 to
**1.290**), group-a 2.056, group-b 0.271, group-c 0.857, group-g 0.702, no
`group-e`. The bar is met overall on both draws; the claim that the index
is *faster* than libavformat's open overall holds on draw 2, whose
interval ends at 0.752, and not on draw 1, whose interval reaches 1.290.
The two intervals overlap, so neither contradicts the other; the
difference is what the draws hold, draw 2 having six `group-e` inputs and
six with a segment index where draw 1 had none and three.

**The bar is met overall and in every class that carries a verdict but
group-a, and missed in group-a.** Overall, and in `group-c`, `group-e` and
`group-g`, the upper bound of the interval is below 1.00, so the index is
faster than libavformat's open there. `group-b` carries no verdict: it
has one input, under the bar's five. That input is also not independent
of draw 1 (below), so the class rests on less than its one row suggests.
The two right-hand columns are reported, not judged.

CoreAV is faster — a ratio below 1.00 — on **30 of the 41 inputs**
(73%): 3 of 12 in `group-a`, 15 of 17 in `group-c`, 6 of 6 in `group-e`, 5 of
5 in `group-g`, and the one `group-b` input. Draw 1 (`w2-04`): 18 of its 30
compared inputs (60%), being 1 of 8 in `group-a`, 13 of 17 in `group-c`, 3
of 4 in `group-g` and its one `group-b` input; `hls index` refuses the other
ten of draw 1's 40 files. Both shares are over the inputs each run's
engines both open, the same sets the medians use.

- **What the speed-up did.** The HLS index takes 1.54 to 2.30 times what
  listing the samples with the packet index takes (A-index, the same
  run), a median of 1.89. Run `w1-01`, before the change, measured 3.35
  (group-a), 2.59 (group-c) and 2.71 (group-g, over its group-g inputs **without** a
  segment index; the seven with one ran at 12 to 124 times the packet
  index, since it was built from their segment index while the HLS index
  read every fragment). The whole of that ratio is the index's own work:
  the segment boundaries, the per-sample records and the serialized
  index, which the packet index does not produce.
- **Sources with a segment index.** Six of draw 2's inputs carry one, and
  they are the tightest group in the set: 0.475 to 0.522 of libavformat's
  open, which is what reading the segment index rather than every
  fragment looks like. Run `w1-01`'s seven such inputs ran at 4.2 to 36.8
  times it.
- **Why group-a still misses.** On audio-only files the packet index is
  already at 0.833 of libavformat's open, against 0.334 for group-c and
  0.264 for group-g: there is almost nothing between CoreAV's table walk and
  libavformat's on a file whose tables are one track of constant-rate
  AAC. An index build that costs about twice the table walk therefore
  lands near 2, and no share of it is the table walk's. The class's
  twelve inputs run from 0.497 to 2.336. The three under 1.00 are not
  separated from the rest by size — the fastest, at 0.497, is a 953 KB
  fragmented DASH segment whose segment index stands for its fragments,
  and the 3.6 MB audiobook at 0.783 is larger than two files above 1.00 —
  and 7 of the 12 are audiobooks, which is the shape of this
  draw's `group-a` class rather than of audio files in general.
- **What the two small classes rest on.** `group-b`'s single measured
  input repeats a draw-1 file's structural signature
  ([`docs/corpus.md`](../../corpus.md#rules-withdrawn-or-not-held)): a different clip of the same recording session, 59.7 MB against 13.9 MB. Its 0.271 here and draw 1's 0.271 are therefore not
  independent evidence, though they are two files. `group-g`'s five inputs all have a segment index, and at five inputs the bootstrap's
  95% interval is the sample's own range, so 0.475 to 0.522 says that
  all five were under 1.00 and little more.
- **The miss is recorded; the bar does not change** (`docs/design/hls.md`,
  "Misses are recorded with their cause before any bar changes").

### What draw 2 is, and what it is not

The draw is held out and was never used for tuning, and its rules and
salt were committed before it was collected. It is nonetheless **smaller
than the rules aimed at**:

- 46 `real` files against a target of 60 to 100, with `group-b` at two
  files and `group-e` at six against ten.
- The caps bind: 377 candidates went over a source's, 296 over a class's
  ceiling of 20, 86 over a device's three and 39 repeated a structural
  signature.
- Two of its own rules did not hold: the source cap, which the script
  implements as a flat 10 and which is therefore 22% of a 46-file draw
  for its two largest sources, and the signature rule across draws, which
  the script did not apply.

The corpus README records both, and that permission is no longer what
limits them. For the comparison it means `group-b` carries no verdict and
`group-a`'s rests on twelve inputs.

# H.264 kernels on the training set

Status: Accepted. Each candidate's bar below was committed before that
candidate was timed.

The kernel candidates for the training set's profile of `main` at
`dd8e81c` (`docs/measurements/h264-training/README.md`). Each
candidate is a change to `crates/coreav-kernels-h264` alone. It is kept
only if it beats the code it replaces in wall time on the training set;
otherwise the simpler code stays, whatever the instruction count says.

## Method (every candidate)

- **Streams.** The 32 streams of the training set, extracted with
  `docs/measurements/h264-training/tools/extract.py`. Each SHA-256 is
  checked against the committed `es.json`.
- **Binaries.** `cargo build --profile dist --example decode -p
  coreav-h264` of the candidate and of its base, from one source tree each,
  on one machine, in one session.
- **Timing.** `scripts/h264_parity.py --pin 2 --rounds 5`, one call per
  resolution class with `--frames` that class's F (sd 1800, 720p 900, 1080p
  300, 4k 150). The candidate and its base are `--decoder`s of the same
  call, so they run in the same interleaved rounds.
- **Measure.** For each stream, the candidate's median time over its base's
  median time, in the discard pairing.
  - **Overall:** the median over the 32 streams, with a bootstrap 95%
    interval over 2,000 resamples.
  - **Per class:** the median over each class's 8 streams.
- **Bar.**
  - The candidate is kept only if the overall median is at most 0.995 and
    the interval's upper bound is below 1.000, and no class median is
    above 1.010.
  - Level or worse keeps the base.
  - Callgrind and the ratio to FFmpeg 7.0.2 are reported beside it as
    information; neither decides.
- **Machine.** Recorded with each result. Before and after are timed on
  one machine, never across two.

## Candidate 1: chroma edges

Two changes, timed in the same rounds as three decoders: `base` (`main`),
`rows` and `columns`.

- **1a, `rows` against `base`.** The vector chroma kernel, which the
  dispatch already takes for edges along a row:
  - writes back only p0 and q0, the only samples clause 8.7.2.3 with
    chromaStyleFilteringFlag changes, and nothing where no line was
    filtered;
  - gathers an edge down a column four bytes a line instead of through the
    eight-byte helper.

  The dispatch is unchanged (`CHROMA_COLUMN_VECTOR = false`).
- **1b, `columns` against `rows`.** The dispatch also sends chroma edges
  down a column to that kernel (`CHROMA_COLUMN_VECTOR = true`). The
  profile of `main` puts the scalar filter of those edges at 5.5% of the
  time (`<Auto as Kernels>::filter_edge`), 21% in 720p.

1b was measured before on one 1080p tuning stream and found level
(`perf/h264-chroma-edge-column`). The training set is the wider test that
decides it here.

**Result.**
- **Machine:** Intel Xeon reporting 2.90 GHz, 8 vCPU, AVX-512, the development
  VM of the training set's README.
- **Run:** `reports/c1.<class>.json`, `--pin 2 --rounds 5`; every stream ran.
- **Ratios:** `ab.py` (candidate over base, discard pairing).

| | Overall median [95% interval] | Faster | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|---|
| 1a `rows` / `base` | **0.989** [0.987, 0.995] | 27 of 32 | 0.992 | 0.989 | 0.989 | 0.988 |
| 1b `columns` / `rows` | **0.977** [0.969, 0.986] | 28 of 32 | 0.971 | 0.969 | 0.986 | 0.994 |
| both, `columns` / `base` | 0.965 [0.961, 0.978] | 29 of 32 | 0.964 | 0.957 | 0.977 | 0.992 |

**Verdict.** Both pass their bars, so both are kept, with
`CHROMA_COLUMN_VECTOR = true`. The single 1080p stream that found 1b level
was the exception; across the training set the columns gain the most in
sd and 720p, where chroma edges are the largest share of the time.

As information, the median ratio to FFmpeg 7.0.2 (discard) went from 2.66
to 2.56 (sd), 2.96 to 2.76 (720p), 2.68 to 2.58 (1080p) and 2.81 to 2.75
(4k).

## Candidate 2: luma interpolation at fractional positions

`luma_mc_filtered` of the x86 kernels, rewritten. Its base is the kernel
crate as candidate 1 leaves it, so that the two are measured apart. The
code it replaces:
- filtered each output row afresh;
- loaded and widened the six source rows of a vertical tap again for every
  output row and every half of a 16-wide row;
- computed the centre sample j in 32-bit lanes with `mullo_epi32`.

The rewrite:
- **one 256-bit pass:** a 16-wide row is one pass of sixteen 16-bit lanes;
- **rolling windows:** a vertical tap keeps its six rows in registers and
  moves down one row per output row, so each source row is widened, or
  filtered horizontally, once;
- **j in pairs:** j interleaves rows in pairs and multiplies each pair by a
  pair of the filter's taps (`madd`, 32-bit sums), Equations 8-246 and
  8-247.

The training profile of `main` puts luma interpolation at 15.7% of the time
at one thread: `luma_mc_filtered`, `vertical_tap`, `tap_row`, `centre_tap`
and `luma_mc`.

The bar and the method are those of the Method section above.

**Result.** Same machine and method as candidate 1. The run is
`reports/c23.<class>.json`: base `c1` (candidate 1's state), `c2` (this
candidate) and `c3` (candidate 3) in the same rounds, and every stream ran.

| | Overall median [95% interval] | Faster | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|---|
| `c2` / `c1` | **0.916** [0.895, 0.949] | 28 of 32 | 0.895 | 0.940 | 0.895 | 0.995 |

**Verdict.** The bar is met; the rewrite is kept. As information, the
median ratio to FFmpeg 7.0.2 (discard) went from 2.57 to 2.36 (sd), 2.79 to
2.66 (720p), 2.59 to 2.35 (1080p) and 2.70 to 2.48 (4k).

## Candidate 3: averaging and copying in the vector kernel

`combine` with default weighting: `Combine::Average` (Equation 8-284) and
`Combine::Copy`. Both went to the scalar reference before. In the x86
kernel, each row is now done in pieces of 16, 8 and 4 samples and a padded
remainder: Average with `pavgb`, which computes `(a + b + 1) >> 1`, and Copy
as plain moves. The dispatch sends both modes there
(`AVERAGE_VECTOR = true`).

- **Base:** the kernel crate as candidate 1 leaves it, timed in the same
  rounds as candidate 2.
- **Why:** the training profile puts `Scalar::combine` (these two modes) at
  0.9% of the time on average, but at 15.2% and 7.7% in two of the
  worst-ratio streams.

The bar and the method are those of the Method section above.

**Result.** In the same run as candidate 2 (`reports/c23.<class>.json`):

| | Overall median [95% interval] | Faster | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|---|
| `c3` / `c1` | **1.006** [1.003, 1.014] | 5 of 32 | 1.004 | 1.009 | 1.005 | 1.018 |

**Verdict.** Rejected: slower overall, and the 4k median is above 1.010.
It is slowest where the reference averaging matters most:
- ae16bb7c7192, where `Scalar::combine` takes 15.2% of the time, ran 7.4%
  slower;
- b71b6b31e54f ran 2.8% slower.

The reference is inlined into the dispatch. The vector path pays for the
`Simd` wrapper's checks and a call into a `#[target_feature]` function
that cannot be inlined, and on blocks of 4x4 to 16x16 that costs more than
`pavgb` saves. Whether the compiler vectorizes the reference loop was not
checked.

The code is not kept, so averaging and copying stay with the reference.
The way left open is on the caller's side: fewer, larger calls, or
predicting straight into the picture, which the decoder already does for
one list with default weighting.

## The chroma gather, reformulated

Candidate 1 gathers a chroma edge down a column in `transpose8x4_u8`, an
unpack chain like any byte transpose. In the reformulation the eight 4-byte lines
go into two vectors of four lines each, a byte shuffle groups each sample
position, and two 32-bit unpacks join the halves (`samples_by_position`).
The output is the same.

The rewrite is timed against the gather that candidate 1 measured, both on
top of candidate 2, on the sd and 720p streams, where chroma edges down a
column weigh most. Method as above.

**Bar.** The reformulation is kept if its median per-stream ratio over the
16 streams is at most 1.005; above that, the change is reported.

**Result.** `reports/c4.<class>.json`, same machine and method: `c4` (the
reformulation) against `c2` (the measured gather), on the 16 streams:

| | Median [95% interval] | Faster | sd | 720p |
|---|---|---|---|---|
| `c4` / `c2` | **1.0048** [0.9995, 1.0102] | 5 of 16 | 1.0041 | 1.0068 |

**Verdict.** Within the bar, by a hair, so the reformulation is kept. It
most likely costs about half a percent of the time on sd and 720p against
the unpack chain; 5 of 16 streams were faster. The bar had no interval
condition and covered only sd and 720p, so 1080p and 4k were not timed
for it. That leaves most of candidate 1b's gain (0.977, the larger part of
it in exactly these classes).

# H.264 training set

Status: Accepted. The salt, the rule and the list below, and the method
and the check of "What it is used for", were fixed before any stream of
the set was decoded, profiled or timed.

The rounds of step 6 tuned on eight streams of N2's tuning set at 300
frames and measured about 2.5 against FFmpeg 7.0.2; the held-out draw of
an earlier change measured 3.36 (`docs/measurements/h264-n2-heldout/README.md`). A ratio
from those eight streams is not a forecast of a held-out one
(`docs/design/h264.md`, "Speed (step 6)"). This set is the wider one that
profiles and A/B runs use from here on, drawn like the held-out set and
measured with its frame counts, so that what improves here is what a
held-out draw measures.

## Draw

`scripts/h264_training_draw.py` (run at `dd8e81c`) writes `list.json`.

- **Pool.** The pool of an earlier change's held-out draw (`../h264-n2-heldout/pool.json`,
  SHA-256 `b4aab02b8fdda9c8e5f1be19d3ea8d56e8f119656ac88bda26ddf22c44f46db7` as fixed (the public copy, whose `key` fields hold the file name only and whose `category` fields are removed, hashes to `4fd05b6ca69f7d8238b16cf43bda985d4bf407154f1ebf5c2e6b408750f0c87f`)):
  corpus v1 `real` streams in N1's scope that N1 decoded in agreement with
  FFmpeg, minus N2's held-out set, minus every stream named in the
  repository's tracked text at `ca5e4bc8` (N1's and N2's tuning and A/B
  sets among them), minus streams under the 0.4 s FFmpeg floor. Minus the
  64 streams that draw took, which are spent. Left: 42 sd, 55 720p,
  48 1080p, 33 4k.
- **Salt.** `coreav-h264-training-1:20261021`, SHA-256
  `32ce5447057b4816c3909c2fb5a3b5836866fdf0f231785644f304159f3a8960`.
- **Rule.** Per resolution class, order the pool by
  HMAC-SHA256(salt, `<file sha256>.<track>`) ascending and take the first 8:
  32 streams. Classes as in the held-out draw.
- **List.** `list.json`, SHA-256
  `9f7b9f634cd2e89dc69ff17feef51490fbaa1ce8d7eccf9545210c9ae93ef2cb` as fixed (the public copy, whose `key` fields hold the file name only and whose `category` fields are removed, hashes to `2dfed9c93718643a2b378ccf00f7bc492000a971f132ea539cd0cda0991394b8`).
  Profiles as drawn, not stratified: 18 High, 10 Main, 4 constrained
  Baseline (the held-out draw: 44, 9, 11 of 64).
- **What it leaves.** These 32 streams are named here, so the held-out
  draw's rule (every stream named in the tracked text is left out) keeps
  them out of the next held-out draw. The pool for that draw keeps at
  least 34 sd, 47 720p, 40 1080p and 25 4k, enough for 16 per class.
  This set is never reported as held-out.

## Input

`tools/extract.py list.json DIR > es.json`: each stream's video track
stream-copied to Annex B, the first `F` access units of the class (sd
1,800, 720p 900, 1080p 300, 4k 150, as the held-out draw), or the whole
stream where it has fewer. `es.json` records the SHA-256 of each
elementary stream, so every machine that measures this set can check it
reads the same bytes.

## What it is used for

1. **Profiles** (`profiles/`). For each stream, `perf record -e cpu-clock
   -F 999` of `decode --discard` over the stream's `F` frames, the
   `dist` build of the commit named in the file, SIMD kernels on:
   - one thread, pinned to one core;
   - eight threads (`--threads 8`), unpinned, recorded per thread.

   Targets are ranked by their **mean per-stream share** of the samples:
   each stream weighs the same, because the held-out bar is a median of
   per-stream ratios and a few long 4k streams must not decide the
   ranking. The pooled share (every sample weighs the same) and the
   per-class means are reported beside it. With eight threads, the
   samples are CPU time, not wall time; the report adds the share of the
   samples on the caller's thread and how busy that thread is (its
   samples over the run's wall time at 999 Hz), since a caller's thread
   near 100% busy puts its own symbols on the critical path whatever
   their CPU share.
2. **A/B of candidates.** A change is measured against its base on these
   32 streams with `scripts/h264_parity.py --pin`, interleaved rounds,
   `--frames` the class's `F`, against bars written in the design note
   and committed before the run, as in the rounds of step 6.
3. **Ratios to FFmpeg, as information.** Never a claim and never a bar's
   arm: a held-out draw is the only thing a claim rests on.

**Representativeness check** (fixed before the first run): `main` at
`dd8e81c` is timed against FFmpeg 7.0.2 on this set, one thread,
`--pin 2 --rounds 3`, discard and write pairings. The held-out draw
measured `ca5e4bc8` at 3.359 (discard) and round 5 took 0.964 of that on
the tuning streams, about 3.24. If the median per-stream ratio here
(discard) is at least 2.9, the set reproduces the held-out level and
stands. If it is below 2.9, the set does not, which is reported, and its
streams are compared with the held-out ones (run length, profile, bit
rate) before any tuning on it.

## Machine calibration

Fixed before the run, after the check above came out below 2.9: whether the
held-out run's machine, not its streams, accounts for the difference.

- **Streams.** The first 4 per class of `../h264-n2-heldout/list.json`, in
  its order (ascending digest): 16 streams. The draw is spent; this times
  again the binary that was timed on it, which tunes nothing.
- **Binary.** `ca5e4bc8`, the one an earlier change measured, built as there.
- **Harness.** `scripts/h264_parity.py` from `ca5e4bc8`, `--pin 2 --rounds 5
  --frames F`, F by class as in an earlier change, FFmpeg 7.0.2, on this machine.
- **Quantity.** Per stream, this run's ratio over an earlier change's for the same
  stream (`../h264-n2-heldout/reports/<class>.ff7.json`), in both pairings,
  and FFmpeg's and this decoder's time here over there. Median over the 16
  with the bootstrap interval of the check.
- **Reading.** If the discard median of the ratio of ratios is at most 0.90
  with the upper end of its interval below 1.00, the machine accounts for a
  difference of that size and the set's level is read as the held-out
  level on this machine. If the interval includes 1.00, the machine does
  not, and the difference is the streams'.

---

Results. The draw, the check and the calibration rule above are unchanged;
two corrections to the text above the line: the counts the set leaves for a
held-out draw (34, 47, 40, 25) are upper bounds, since a stream named later
leaves the pool too; and the caller's thread's busy fraction is measured
over the span from its run's first sample to its last, not over the run's
wall time, because `perf record` notices the child's exit up to a second
late. Binary: `main` at `dd8e81c`
(`cargo build --profile dist --example decode -p coreav-h264`, rustc 1.99.1).
Machine: the development VM of the rounds of step 6 and step 7, 8 vCPU (4
cores, 8 threads) Intel Xeon reporting 2.90 GHz, AVX-512, 16 GB, Linux
6.18.49. The held-out run of an earlier change reported a 2.50 GHz Xeon: another
machine. Elementary streams: `es.json` (32 extracted, none failed, 227 MB).

### The representativeness check: not met

`reports/check-dd8e81c.<class>.json` (`--pin 2 --rounds 3`, FFmpeg 7.0.2,
`-threads 1`); every stream ran, none failed.

| Class | n | discard: median [95% interval] | write: median [95% interval] |
|---|---|---|---|
| overall | 32 | 2.819 [2.511, 3.097] | 2.572 [2.450, 2.944] |
| sd | 8 | 2.658 | 2.718 |
| 720p | 8 | 2.995 | 2.651 |
| 1080p | 8 | 2.731 | 2.604 |
| 4k | 8 | 2.877 | 2.545 |

The discard median is below the 2.9 of the check: the check is not met,
and as registered the set does not reproduce the held-out level as
measured in an earlier change. The stream comparison the check calls for
(`tools/compare.py`, which prints every number here) finds no
stream-level difference that would account for it:

- Bit rate is alike (median 19.8 bits a macroblock here, 23.0 in the
  held-out draw), and the ratio is lower by about the same factor in each
  bit-rate tercile (2.93, 2.91, 2.63 here against 3.51, 3.26, 3.27) and in
  each profile but Main (Baseline 2.51 against 3.38, High 2.83 against 3.41;
  Main 2.93 against 2.78, on 10 and 9 streams). Run lengths are the
  held-out draw's by construction.

What remains is a hypothesis: the machine. Each list carries N1's FFmpeg
time for the frames measured, one machine's time per stream for both
sets. Over it, FFmpeg took a median 0.651 in the held-out run and 0.423
here, and this decoder 2.239 and 1.194. Those are medians over two
different sets of streams with a wide spread (FFmpeg over N1, 10th to
90th percentile: 0.39 to 0.92 in the held-out set, 0.26 to 0.69 here),
and the factors (1.54 for FFmpeg, 1.82 for this decoder after the code's
own 1.03) are larger than the clock difference alone (2.90 against 2.50
GHz, 1.16) would give; another load on the held-out run's machine would fit
them as well. Taken at face value they move the ratio by 1.54 / 1.82 =
0.85, and 3.359 × 0.97 × 0.85 = 2.77 against 2.82 measured. The
calibration above tests this directly, with the same binary on the same
streams.

### Machine calibration: about 10%

`reports/calibration-ca5e4bc8.<class>.json`, timed with the harness of
`ca5e4bc8` as registered (its reports carry no machine field; the machine is
the one above). The 16 elementary streams' SHA-256 (`reports/calibration-es.json`)
equal an earlier change's `es.json`; every stream ran, none failed. Per stream,
this machine over an earlier change's (`tools/compare.py`):

| Quantity | Median [95% interval] |
|---|---|
| ratio to FFmpeg, discard | 0.903 [0.889, 0.936] |
| ratio to FFmpeg, write | 0.915 [0.892, 0.942] |
| FFmpeg's time | 0.764 [0.732, 0.777] |
| this decoder's time | 0.684 [0.659, 0.709] |

Neither reading registered above applies as written: the median, 0.903, is
just above 0.90, and the interval excludes 1.00. What it says is a size:
with the same binary on the same streams, this machine puts the ratio
about 10% lower than an earlier change's machine (6 to 11%), FFmpeg running 1.31 times
and this decoder 1.46 times as fast. The estimate from N1's times above
(15%) overstated it. On these 16 held-out streams `ca5e4bc8` measures a
median 2.723 here, against 2.98 for the same 16 in an earlier change: they are an easier
subset than the 64 (3.36 there), so applying their factor to the 64 assumes
it carries over.

So the held-out level on this machine is about 3.359 × 0.903 = 3.03 for
`ca5e4bc8` and about 2.94 for `dd8e81c` (× 0.97, rounds 5 and 7 on the
tuning streams), against which the training set's 2.819 [2.511, 3.097] is
within its interval, a closeness no bar was set for, so it is
framing, not a test. The
set is adopted on the stream comparison, which found no difference
between the two sets. Of the factor 0.839 from 3.359 to 2.819, the
machine is 0.903, the code 0.97 and the rest, 0.96, which streams each set
happens to hold. Ratios from the two
machines are not compared from here on (`docs/design/h264.md`, "The bar
machine"). The next held-out draw leaves out, by name, the 64 streams of
change A's draw and the 32 of this set, besides the rules of change A's draw.

### Where the time goes

`profiles/dd8e81c.t1.json` (one thread, pinned) and
`profiles/dd8e81c.t8.json` (eight threads), 32 of 32 streams. Shares are
the mean per-stream share of the samples (the ranking), with the pooled
share beside it; the per-class means are in the files. "Kernel" rows are
`coreav-kernels-h264`, the others `coreav-h264`.

| Target | Symbols | 1 thread: mean, pooled | 8 threads: mean, pooled |
|---|---|---|---|
| Kernel: luma interpolation | `luma_mc_filtered`, `vertical_tap`, `tap_row`, `centre_tap`, `luma_mc` | 15.7, 19.3 | 15.1, 18.5 |
| Kernel: deblocking edges | `simd::x86::filter_edge` 8.7; `Auto::filter_edge` 5.7, which is the scalar filter inlined for vertical chroma edges (`Simd::prefers_filter_edge`) | 14.4, 13.8 | 13.2, 13.2 |
| Control: motion compensation | `motion_compensate` 5.7, `inter::fetch` 2.5, `inter_pred`, `store_motion`, `direct_8x8`, `neighbour_motion` | 13.9, 9.4 | 14.6, 10.6 |
| Control: CABAC residual and macroblock | `significance_map`, `levels`, `decode_mb`, `parse_residual`, `mvd`, `block` | 12.6, 18.5 | 11.3, 17.0 |
| Control: deblocking driver | `deblock_rows` 7.8, `motion_strength` | 8.6, 7.4 | 7.9, 7.3 |
| Kernel: chroma interpolation | `Auto::chroma_mc` | 7.0, 6.3 | 6.9, 6.2 |
| Kernel: prediction combine | `simd::x86::combine` 5.1 (weighted), `Scalar::combine` 1.0 (average, copy) | 6.1, 5.0 | 6.4, 5.4 |
| Control: slice loop | `decode_slice`, `start_mb` | 3.8, 2.3 | 3.8, 2.5 |
| Kernel: inverse transforms | `idct4x4_add` (scalar), `idct8x8_add` | 3.4, 4.9 | 3.2, 4.6 |
| Memory fill and copy | `memmove`, `memset`, `memcmp` | 2.3, 1.6 | 3.0, 2.1 |
| Control: CAVLC | | 2.0, 1.0 | 1.2, 0.7 |
| Control: intra | `reconstruct_intra`, `intra::` | 1.7, 1.9 | 2.2, 2.3 |

The ranking is the same at one and at eight threads. With eight threads
the caller's thread holds a median 1.8% of the samples and is busy a
median 8.6% of the run (at most 31%, on a short 4k stream), so it is not
on the critical path; `memmove` grows from 1.0% to 2.5% (mean), the copies of the
bands' aprons. Symbols outside a file's top 40 are not in it, so a row's
difference between the two files can be partly that cut. The tuning streams' top five (0404af, round 5: `chroma_mc` 12.9%,
`levels<16>` 10.2%, `motion_compensate` 8.9%) overweighted chroma and CABAC
against this set, where the edge filter and luma interpolation lead.

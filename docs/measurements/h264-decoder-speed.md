# H.264 decoder: the speed rounds of step 6

Where this decoder stands against FFmpeg, single-threaded, and what each
round of `docs/design/h264.md` step 6 changed. Bars P1 and P2 are
measured once, at the end, on a fresh held-out draw; these rounds are
measured on the N2 spike's tuning streams, which carry no bar weight and
exist to be optimised against.

## Method

Both sides decode the same Annex B elementary stream and throw the
frames away, so what is compared is decoding: this decoder with
`--discard`, FFmpeg with `-threads 1` writing `rawvideo` to a
`/dev/null` the kernel discards without reading. One warm-up of each,
then three rounds in alternating order; a side's time is the median of
its rounds, a stream's ratio is ours over FFmpeg's, and the report gives
the median of the per-stream ratios with a bootstrap 95% confidence
interval. `scripts/h264_speed.py` runs it; `scripts/h264_profile.py`
takes the profile beside it.

**The pairing these rounds used is asymmetric.** `--discard` throws the
frame away; `ffmpeg -f rawvideo -` writes about 3 MB a frame, so the two
sides differ by more than what is being measured. That much follows from
what the two commands do, not from a measurement.

What it is worth is not settled. Eight tuning streams, three interleaved
rounds, medians with a 95% bootstrap interval
(`scripts/h264_parity.py`; evidence:
[`h264-rounds/pairing-comparison.json`](h264-rounds/pairing-comparison.json)):

| Pairing | Median | 95% interval |
|---|---|---|
| Both discard: `--discard` against `-f null -` | 3.633 | [2.751, 4.188] |
| Both write: frames to stdout against `-f rawvideo -` | 3.901 | [2.825, 4.041] |
| Mixed, what these rounds used | 3.719 | [2.783, 3.965] |

The two symmetric pairings are 7% apart and the mixed one lies between
them, with all three intervals overlapping heavily. So this machine
cannot say by how much, or even reliably in which direction, the
asymmetry moves a ratio — only that the comparison is not clean. An
earlier version of this note claimed the two symmetric pairings agreed at
3.75 and that the mixed one therefore flattered this decoder by about 4%;
that rested on a single-stream run and does not survive eight streams.
It is withdrawn.

Two things that do stand: the mixed pairing should not be used, because
nothing is gained by comparing two different amounts of work; and no
ratio in this note is good to better than a few percent, which is worth
remembering before reading a change of that size as a result.

An earlier version also said the null muxer costs FFmpeg more than a
discarded write, so the mixed pairing was the reading least favourable to
this decoder. That is not what the measurement shows either, and it is
withdrawn too.

This is a different rule from the before/after one below. That one is
about the two binaries of a comparison; this one is about the two sides
of the CoreAV-against-FFmpeg pairing. Both say the comparison should
differ only in what it measures, but they are not the same rule and the
one below was not written for this. **Before and after are always measured
in one session**: this machine's absolute times drift between sessions
by more than some of the changes are worth, so a round's "before" is
re-measured beside its "after" rather than quoted from an earlier run.
How far it drifts, measured: the same binary and the same method gave a
median of 3.426, then 4.440, then 4.098 as the "before" of three
sessions an hour or two apart. The machine is shared and has four cores
behind eight threads, so another build or another session moves every
absolute time while leaving the comparison inside a session intact. An
earlier report of 3.75 is not comparable with any of them: it predates
the `--frames` fix below and used the method where both sides wrote
their frames.

Machine: the development machine, 4 cores and 8 threads, Intel Xeon at
2.90 GHz, Linux x86-64, nothing else running. Build: the `dist` profile
(fat LTO, one codegen unit), which is what ships; the plain `release`
profile measures the same (3.725 against 3.752), so the build is not
where the distance is.

## Where this decoder stands

Eight tuning streams, 300 frames each, decode-only on both sides, one
session. Evidence:
[`h264-rounds/invalid-round1-pair-before.json`](h264-rounds/invalid-round1-pair-before.json)
and its "after", kept under that name because they are **not** a valid
pair: the error below is what they record.

| | Median ratio to FFmpeg, one thread |
|---|---|
| This decoder | **about 3.0** |
| The N2 spike, same stream and harness | about 2.5 |

On the 1080p tuning stream, 300 frames: FFmpeg 0.538 s, the spike
1.362 s, this decoder 1.613 s.

## The change in this round, and what it measures

A reused picture is no longer cleared. Every macroblock a slice covers
is written by decoding it, so clearing a 1080p frame spent about 3 MB of
stores on samples about to be overwritten; the macroblocks no slice
covered are filled when the picture is finished, and the scan that finds
them runs only where the slices did not cover the picture.

**It saves 383 instructions per macroblock, 3.4% of the decoder**
(19.575 G against 18.907 G over 1,747,200 macroblocks), counted exactly
by callgrind. The two binaries differ only in whether `Picture::reset`
clears the planes; both carry the harness fixes above, as the rule below
requires. Evidence:
[`h264-rounds/picture-clear-instructions.json`](h264-rounds/picture-clear-instructions.json).

In wall clock the same change is **−1.1% median** over six streams, from
−4.7% to +1.6% — at this machine's two-point noise floor, which is why
the instruction count is the figure quoted. A time measurement of a
change this size on this machine says little; an exact instruction count
says what the change does.

## An error in the first measurement of this round, and the rule it gives

The first version of this note claimed 4.098 → 3.598 for this round.
That was wrong, and the cause is worth keeping.

The "before" binary was built from a tree without the `--frames` fix
this round introduces — the fix that makes `--frames N` stop the
decoding rather than only the writing. So the baseline decoded whole
streams while the new binary stopped at 300 frames, and the measurement
recorded that difference. Measured against a baseline that has the
harness fix and nothing else, the round is **−1.1%**, not −12%.

The change the first note credited for the win does nothing at all:
`CHROMA_MC_VECTOR` is `false` on x86-64 (`docs/measurements/h264-kernels.md`
disabled the vector chroma kernel, which loses to the reference in the
decoder), so `prefers_chroma_mc` was already always false and removing
`&& w == 8` from it changes no behaviour. It is reverted.

**The rule: the two binaries of a comparison must differ only in what is
being measured.** A pull request that changes the measuring harness
builds its baseline with that change, not without it. This applies to
the decode example, to `scripts/h264_speed.py`, and to anything else
that decides how much work either side does.

## Where the time goes now

The profile after round 1, on a 1080p tuning stream
(`scripts/h264_profile.py`), beside the N2 spike's binary on the same
stream under the same conditions. The spike decodes that stream in
1.496 s and this decoder in 1.779 s, so a share here is 1.19 times a
share there in absolute terms.

| Part | This decoder | The spike |
|---|---|---|
| Chroma interpolation | 17.4% | 13.0% |
| Deblocking driver and strengths | 19.0% | 15.6% |
| CABAC residual blocks | 9.5% | 13.0% |
| Luma interpolation | 6.0% | 7.7% |
| Edge filter kernel | 8.2% | 3.4% |
| Motion compensation control | 8.8% | 9.5% |

The deblocking driver and the edge filter are where the distance is, and
neither is a threshold computation or a spill: round 2 takes them with a
design rather than with micro-edits.

## Round 2: deblocking and chroma motion compensation

Measured after the gap attribution
([`h264-gap-attribution.md`](h264-gap-attribution.md)) showed the distance
to FFmpeg is instruction count rather than instructions per cycle: this
decoder executed about five times FFmpeg's instructions while retiring
them faster. (The figure once quoted here as 4.25 compared our discard
run against FFmpeg's *write* run; on one basis it is
10,821 / 2,099 = 5.16. The conclusion is unchanged and the ratio is
larger, not smaller.) So this round is measured in **instructions per
macroblock**,
which callgrind counts exactly, with wall-clock beside it rather than
under it. The FFmpeg figures used throughout —  2,099 instructions per
macroblock decode-only, 3,250 with its SIMD disabled — are in
[`h264-gap-b/ffmpeg-simd-vs-c.jsonl`](h264-gap-b/ffmpeg-simd-vs-c.jsonl),
beside the process-wide counts and the vector-share measurement in the
same directory. The instruments are `scripts/h264_counts.py`,
`scripts/h264_stage_insn.py`, `scripts/h264_isa_mix.py` and
`scripts/h264_parity.py`.

Stream `0404af1890da.0.1080p.h264`, 300 frames, 1,747,200 macroblocks.
Evidence: [`h264-rounds/stages-baseline.json`](h264-rounds/stages-baseline.json),
[`stages-after-deblock.json`](h264-rounds/stages-after-deblock.json),
[`stages-after-chroma.json`](h264-rounds/stages-after-chroma.json).

| Stage | Before | After deblocking | After chroma |
|---|---|---|---|
| Deblocking | 3,843 | **2,273** | 2,273 |
| Chroma MC | 2,522 | 2,522 | **682** |
| CABAC | 1,180 | 1,182 | 1,182 |
| MC control and fetch | 1,105 | 1,048 | 1,048 |
| Luma MC | 955 | 955 | 955 |
| Intra prediction | 612 | 612 | 612 |
| **Total** | **10,821** | 9,269 | **7,617** |

For scale: FFmpeg's whole decoder is 2,099 instructions per macroblock
decode-only, and 3,250 with its hand-written SIMD disabled
(`-cpuflags 0`). Before this round, deblocking alone cost more than
FFmpeg's entire decode.

**What changed.**

- **A quiet macroblock's edges are retired without deriving each one**
  (−41% of deblocking). Where a macroblock is inter, holds no luma
  coefficient and carries one motion throughout, every boundary inside it
  is bS = 0 by clause 8.7.2.1, so twenty-four derivations are skipped;
  where its neighbour is quiet in the same sense, one derivation answers
  for all four segments of the macroblock edge. Uniformity is tracked
  where the motion is written rather than re-derived in the filter.
- **A full-sample chroma block is predicted by copying it** (−73% of
  chroma MC). At xFracC = yFracC = 0 equation 8-266 weights the
  upper-left sample by 64 and the rest by 0. Measured on this stream,
  89.0% of chroma motion compensation calls are at a full sample
  position, and all of them were running the 4-tap filter.

**The round's cumulative effect**, measured on a second machine so that
coding and measuring did not serialise. Before `ccd1621`, after
`4679c9e`, both built `--profile dist` from clean worktrees, both timed
in the same interleaved rounds, five rounds, eight tuning streams across
four classes. The statistic is the per-stream **paired** ratio
after/before, which is stronger than dividing two medians because each
pair is timed in the same round. Evidence:
[`h264-rounds/round2-cumulative-ab.json`](h264-rounds/round2-cumulative-ab.json).

| Pairing | after/before | 95% interval |
|---|---|---|
| Both discard | **0.925** | [0.859, 0.996] |
| Both write | **0.937** | [0.835, 0.996] |

So about 6% to 8% of the time, not the 30% the instruction count might
suggest. Both intervals lie below 1.

These are the pinned figures. The same run unpinned read 0.906
[0.841, 1.022] and 0.902 [0.852, 0.987], and **pinning made the result
smaller, and more certain on the pairing that mattered**: the medians
moved from −9.4% and −9.8% to −7.5% and −6.3%, and the discard interval
stopped including 1. It did not narrow both — the write interval went
from 0.135 wide to 0.161 — so "more certain" is a claim about the
discard pairing, which was the one that had included 1. The unpinned figures
were this note's first answer and are
withdrawn; they are kept in
[`h264-rounds/round2-cumulative-ab.json`](h264-rounds/round2-cumulative-ab.json)
beside the pinned ones
([`round2-cumulative-ab-pinned.json`](h264-rounds/round2-cumulative-ab-pinned.json))
because the pair is the evidence for pinning being worth doing.

One stream, `0ba6abe19f7b.0.sd`, is 1.4% to 2.0% *slower* after the
round and stays so when pinned, while its instruction count moved
+0.19%. That is too close to the floor to call, and it is recorded
rather than explained.

One of the eight streams is slower, `0ba6abe19f7b.0.sd` at 1.020, and
its instruction count moved +0.19% — which is to say not at all. It is
recorded rather than explained. (`04e45b2f41df.0.720p` reads 1.024
*unpinned* and 0.996 pinned; quoting it as slower was reading the
withdrawn run, and it is not.)

The gain is concentrated where the round's two changes bite: 1080p and
low-residual screen content, where the quiet-macroblock shortcut and the
full-sample chroma copy both apply often. `0404af1890da.0.1080p` is
0.775 and `00d2057ba2cd.0.720p` 0.912; `03be19c3a47b.0.720p`, which has
little of either, is 0.976.

That machine is 8 vCPU where this note's other figures are from a
4-core machine, so its absolute ratios are not comparable with them.
Only the before/after difference within it is quoted.

**Wall clock on the measurement machine, both symmetric pairings**
(`scripts/h264_parity.py`, 10
tuning streams across four classes, 300 frames, three interleaved rounds,
before and after in one session). Evidence:
[`h264-rounds/deblock-chroma-ab.json`](h264-rounds/deblock-chroma-ab.json).

Ratios to FFmpeg, not seconds:

| Pairing | Before | After |
|---|---|---|
| Both discard | 3.585 [2.886, 4.134] | **3.144** [2.769, 3.552] |
| Both write | 3.620 [2.935, 4.089] | **3.235** [2.789, 3.742] |

**What the instruction counts here do not see.** Callgrind hides
AVX-512 from the program it runs. This machine has it (`avx512f`,
`avx512bw`, `avx512vl` and more) and these kernels do not use it, so our
own code runs the same path either way — but glibc picks its `memcpy`
and `memset` variants from what the CPU reports, so under callgrind it
takes an AVX2 variant where a native run takes an AVX-512 one. Any
figure here for `libc mem*`, and any change whose effect is mostly
fewer or cheaper `memcpy` calls, is therefore measured against a
different `memcpy` than the one that actually runs.

That is the most likely reading of the constant-width chroma copy
below: it removes `memcpy` calls, which the instruction count rewards
at the AVX2 variant's price, while the native AVX-512 variant was
already cheap enough that the time did not move. The change is kept —
it is strictly less work and the output is identical — but it is the
clearest case in this note of an instruction count flattering a change,
and it is why the round's claim rests on the cumulative wall clock
rather than on the instruction total.

**Instructions fell 29.6% and time 4.2%, and the difference is the
point.**

(4.2% is the per-stream *paired* median on the discard pairing, 6.6% on
the write pairing, computed from the same file. An earlier version of
this line said 12.3%, which was one median of FFmpeg ratios divided by
another — the weaker statistic this note argues against two sections
above. Quoting it here while arguing against it elsewhere was wrong, and
the corrected figure is consistent with the pinned cumulative below.) The
work removed was already being retired efficiently — this
decoder's instructions per second are higher than FFmpeg's, not lower —
so removing instructions buys less time than their share suggests. A
round that cuts instructions is worth doing anyway, because the gap is
made of instructions, but the conversion is not one for one and no future
round should be budgeted as though it were.

**Rejected: hoisting the reference-geometry check out of the prediction.**
Every reference picture's coded size was re-validated for every partition
and every list, re-checking the same few pictures for every block of
every macroblock, and doing it once when the slice's reference lists are
built is plainly less work. It is also worth **0.09%**: 5,870 to 5,865
instructions per macroblock. That does not pay for what it changes — an
invalid stream whose references disagree in size would fail at the start
of the slice rather than part-way through it, so a damaged picture would
keep fewer decoded macroblocks and conceal differently. A behaviour
change on damaged input needs a better reason than a tenth of a percent,
so it is reverted. Recorded because the redundancy is real and obvious,
and the next reader will otherwise spend the same afternoon on it.

**Rejected: the same full-sample shortcut for luma.** 92.6% of luma
motion compensation calls on this stream are at a full sample position
too, so the chroma result looked like it should generalise. It does not,
and the reason is worth keeping: both luma kernels *already* predict a
full-sample block by copying it, so the change replaced a good
implementation rather than a bad one. Taking the copy in `Auto` made it
a `copy_from_slice` of a runtime length, which compiles to a `memcpy`
call, where the vector kernel was issuing one inline 16-byte move per
row.

Measured: instructions per macroblock **rose** from 7,617 to 7,757 —
luma MC fell 955 to 348 while MC control rose 1,048 to 1,634 (the copy
inlined into its caller) and libc `mem*` rose 335 to 497. Wall clock was
flat: 3.234 to 3.241 on the discard pairing and 3.599 to 3.588 on the
write pairing, over seven streams, both inside the interval. A change
that costs instructions and returns no time is not worth its risk, so it
is reverted. The equivalence test it came with is kept, because the
kernels' own full-sample paths deserve one.

This is also why the chroma shortcut sits in `Auto` and the luma one does
not, which otherwise looks inconsistent.

Correctness: 97 of 97 conformance streams bit-exact after each change,
and output byte-identical to the baseline on the measured stream.

## Round 3: the cumulative effect

Before `32a4144`, after `7f274dc`, measured on the helper machine in one
session: both binaries built `--profile dist` from clean worktrees,
pinned to one core, five interleaved rounds, eight tuning streams across
three classes. The statistic is the per-stream paired ratio. Evidence:
[`h264-rounds/round3-cumulative-ab.json`](h264-rounds/round3-cumulative-ab.json).

| Pairing | after/before | 95% interval |
|---|---|---|
| Both discard | **0.911** | [0.791, 0.990] |
| Both write | **0.905** | [0.815, 0.989] |

Both intervals lie below 1, and every one of the eight streams is faster
or unchanged. The gain is concentrated where the round's changes bite:
1080p runs 0.69 to 0.90, 720p 0.77 to 0.99, and the two sd streams 0.93
and 1.00.

Against FFmpeg in the same session the decoder goes from 3.078 to
**2.654** on the discard pairing and from 2.857 to **2.488** on the
write pairing. It is faster than FFmpeg on none of the eight, before or
after.

**Instructions fell 41.8% and time 30.9%** on the measured stream
(9,439 to 5,494 per macroblock, paired ratio 0.691). That is a better
conversion than round 2 managed, and the reason is in what the round
removed: the deblocking shortcuts and the motion clear take out work and
memory traffic rather than cheap instructions.

Cross-checked on a second machine. The measurement machine and the
helper machine each built the two commits independently and each
measured the baseline at 9,439 instructions per macroblock; for
`0404af1890da` the paired ratio came out 0.692 here and 0.691 there.

## Round 4: CABAC

The restructure designed in `docs/design/h264.md`, "The CABAC
restructure: our design", against its pre-registered bar: a pinned paired
median at or below 0.97 against the round-3 tip over at least eight
streams. Measured on one machine (an 8-vCPU Xeon cloud VM with AVX-512),
all binaries built `--profile dist`, timed in the same interleaved rounds
on core 3, the eight streams of round 3 (SHA-256 identical to its
evidence). These are tuning streams, not a held-out set: the result is a
comparison of two versions of this decoder, not a claim against FFmpeg,
and the 32 tuning streams are checked for identical output. Evidence:
[`h264-rounds/round4-cabac-ab.json`](h264-rounds/round4-cabac-ab.json).

| Session | Pairing | after/before | 95% interval |
|---|---|---|---|
| Session A, four binaries, 7 rounds: final over `18f67e9` | discard | **0.968** | [0.952, 0.999] |
| | write | **0.957** | [0.937, 0.990] |
| Session A: steps 1 to 4 over `18f67e9` | discard | 0.976 | [0.966, 0.993] |
| | write | 0.971 | [0.941, 0.995] |
| Session A: step 5 over steps 1 to 4 | discard | 0.996 | [0.979, 1.026] |
| | write | 0.990 | [0.968, 0.999] |
| Session B, two binaries, 5 rounds: steps 1 to 4 over `18f67e9` | discard | 0.960 | [0.958, 0.965] |
| | write | 0.968 | [0.949, 0.986] |
| Session C, two binaries, 5 rounds: step 5 over steps 1 to 4 | discard | 0.994 | [0.988, 1.002] |
| | write | 0.991 | [0.983, 1.001] |

The tip as shipped, with the error-path fix described below, timed
directly against `18f67e9` in a session of its own (F, 7 rounds): **0.966
[0.960, 0.968]** on discard and **0.969 [0.962, 0.975]** on write, faster
on seven of eight streams in each.

The bar is met on both pairings, in both sessions. Seven of the eight streams are faster on
each; the eighth differs by pairing (`0bb05726e428.0.sd` at 1.066 on
discard and 0.991 on write, `00d2057ba2cd.0.720p` at 1.012 on write), which
reads as the session's noise rather than a slower stream. Steps 1 to 4
read 0.960 in session B and 0.976 in session A: the same binaries, so
that difference is the spread between sessions, and why only pairs timed
in the same session are quoted as results.

Against FFmpeg 7.0.2 in session A the decoder goes from 2.615 to
**2.522** on the discard pairing and from 2.472 to **2.372** on the write
pairing. It is faster than FFmpeg on none of the eight.

Instructions on `0404af1890da.0.1080p` (callgrind, 300 frames):

| Step | per macroblock |
|---|---|
| `18f67e9` | 5,494 |
| 1. A residual block written in place; coded_block_flag from two words | 5,425 |
| 2. Refill after consuming, four bytes at a time; branchless bypass; widened tables | 5,457 |
| 3. The engine in registers: one-slice data, the residual's two loops apart, transition by bin | 5,367 |
| 4. The level contexts as one state; the escape path by value | 5,365 |
| 5. Bin strings as lookups; set-bit block clearing; position without dividing | **5,306** |

Step 2 raised instructions: the inlined four-byte refill took registers
the loop needed, and the compiler spilled the engine to the stack. Step 3
is the fix for that, and the generated code was checked for it: no
memory access to the engine per bin in either residual loop.

The CABAC stage (`scripts/h264_stage_insn.py`) went from 1,189 to 1,058
instructions per macroblock, 71 to 63 a bin. The design's target of 35
is not met; the bar that decides is the clock, and it is.

Where the time is now, pinned `perf` on the same stream: chroma motion
compensation 12.7%, the residual levels 9.3%, luma motion compensation
control 8.6%, the CABAC macroblock loop 7.0%, the deblocking driver 6.5%;
the significance map is 5.9%.

A packed transition entry (one per state and range quarter), measured in session A
against this design, did not beat it: 1.002 [0.986, 1.013] on
discard and 1.018 [1.000, 1.045] on write. It is in
[`h264-rejected.md`](h264-rejected.md).

Correctness: 97 of 97 conformance streams bit-exact, output byte-identical
to `18f67e9` on all 32 tuning streams, and new unit tests check the engine
and the residual decoder against a bit-at-a-time reading of clauses
9.3.3.2 and 7.3.5.3.3 (each was broken on purpose and failed). The
coded_block_flag neighbour words and the I_PCM restart are covered by the
conformance job rather than by unit tests.

After the timing, review found that a block whose levels decoded but whose
data then ran out kept its levels, which a later picture could inherit;
the fix clears the block on that path too, out of line, and drops the
macroblock state of a slice that fails. It changes only paths taken on an
error, and the instruction count on the measured stream stays at 5,306
per macroblock. Timed against the measured binary, it shows no change:
1.015 [0.996, 1.030] on discard and 1.005 [0.994, 1.072] on write (session
E). A first form with the clear inline read 1.013 [1.004, 1.017] on write
in one session (D) and 0.999 [0.978, 1.015] in the next (E), and cost 3
instructions per macroblock; it is not the one kept.

## Round 5: motion compensation and per-macroblock work

Designed and pre-registered in `docs/design/h264.md`, "Round 5: motion
compensation's stores", before each run. Same machine, method and eight
tuning streams as round 4, core 3, seven interleaved rounds per session,
both symmetric pairings; FFmpeg 7.0.2. Tuning streams, so a comparison of
versions of this decoder and not a claim against FFmpeg.

**Session 1, the chroma path**
([`h264-rounds/round5-chroma-ab.json`](h264-rounds/round5-chroma-ab.json)).
The full-sample copy inlined into motion compensation: 0.988 [0.961,
1.001] discard and 0.988 [0.966, 1.002] write against the round-4 tip,
over its bar of 0.98. Reading the picture's lines ahead of the
macroblock: no distance with an interval below 1.00. Both rejected, both
in [`h264-rejected.md`](h264-rejected.md).

**Session 2, the three candidates**
([`h264-rounds/round5-candidates-ab.json`](h264-rounds/round5-candidates-ab.json)),
each against the round-4 tip:

| Build | Discard | Write |
|---|---|---|
| dequantising store | 0.980 [0.949, 0.987] | 0.963 [0.946, 0.990] |
| skip first, lighter start | 0.986 [0.975, 0.996] | 0.987 [0.971, 0.999] |
| mvd and ref_idx edges | 0.991 [0.979, 1.007] | 0.992 [0.989, 0.997] |
| all three | 0.971 [0.942, 0.977] | 0.959 [0.946, 0.964] |

**Session 3, the edges inside the combination**
([`h264-rounds/round5-edges-ab.json`](h264-rounds/round5-edges-ab.json)):
all three against the first two, 0.999 [0.992, 1.000] discard and 0.998
[0.996, 1.006] write, over the bar, so the edges are not merged. The
first two against the round-4 tip, which is what ships: **0.964 [0.956,
0.981]** discard and **0.963 [0.956, 0.978]** write, faster on all eight
streams. Against FFmpeg in the same session: **2.501 to 2.436** on
discard and **2.415 to 2.333** on write. Faster than FFmpeg on none of
the eight.

Instructions on `0404af1890da.0.1080p` (callgrind, 300 frames): 5,306 to
**5,173** a macroblock; CABAC 1,058 to 942, libc `mem*` 152 to 91.

Where the time is now, pinned `perf` on the same stream: chroma motion
compensation 12.9%, the residual levels 10.2%, luma motion compensation
control 8.9%, the deblocking driver 6.4%, the deblocking kernel 6.2%.

Correctness: 97 of 97 conformance streams bit-exact with the detected
and with the scalar kernels, and output byte-identical to the round-4
tip on all 32 tuning streams.

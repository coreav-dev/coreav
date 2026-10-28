# Where the H.264 single-thread gap to FFmpeg is

## Update: what later measurement found, and what below it is superseded

The analysis below is of its date. Three of its conclusions have since
been overtaken by measurement, and the record is clearer with both than
with either alone.

**The 0.433 s this note could not attribute has been attributed, without
reading anyone's source.** It said that row "has no mechanism attached at
all", because the only instruments then available were wall clock and a
stripped binary. Counting instructions instead answered it: the decoder
executed 4.25 times FFmpeg's instructions while retiring them *faster*,
so the gap was never microarchitectural. It was volume of work, and
mostly in two places — deblocking at 3,843 instructions per macroblock
and chroma motion compensation at 2,522, against FFmpeg's **entire**
per-macroblock budget of 2,099. A third measurement, that only 11.6% of
our executed instructions touched a vector register, explained why.

**The reachable-ratio estimates in section 2 are superseded by having
reached past them.** They put safe Rust at 2.0x to 2.5x "if everything
lands at its optimistic value". Round 3 has since taken the decoder from
10,821 instructions per macroblock to 5,849, and to 5,494 with the intra
work: **1.69x FFmpeg's C-only decoder**, where the note's own figures put
us at 5.16x of it. None of that needed the CABAC `unsafe` exception the
note recommended against, and that recommendation stands.

**The "no credible path to 1.00 single-threaded" conclusion rests on a
premise that no longer holds.** It was argued from 58% of the gap having
no mechanism attached. Most of that 58% now has one. The conclusion may
still be right — single-thread parity with FFmpeg remains unproven and
far off — but it should not be cited on the strength of the reasoning
below, which assumed the gap was unexplainable rather than merely
unexplained.

**What stands.** The method and its caveats: that hardware counters are
unavailable here, that FFmpeg's binary is stripped, that
`-cpuflags 0` measures its assembly at 1.41x, that one slice per picture
rules out slice-level threading on real content, and that threading is
measured against an FFmpeg that is itself parallel. Section 3's
arithmetic is unaffected.

Later numbers: `h264-decoder-speed.md` (rounds and method),
`h264-rejected.md` (what measurement rejected), `h264-gap-b/` (the
instruction-count attribution).

---

Status: measured, as of the date below. Scope: the production decoder (`coreav-h264`) against the
N2 spike and against FFmpeg at one thread, decode only. This note answers
three questions: where the time goes, what ratio is reachable, and what
threading would be worth on top of that.

All numbers below are one measurement session on one machine (4 physical
cores, 8 SMT threads, Intel Xeon 2.90 GHz), one stream
(`0404af1890da.0.1080p.h264`, 300 frames), median of 3 runs, every binary
built `--release` with the same profile. Hardware cycle counters are not
available in this VM (`perf stat -e cycles` reports `<not supported>`), so
the attribution is in seconds, not cycles. The held-out median over the
full set is about 3.0; this stream is 3.30, so the shares below are
representative but the ratio is not the headline number.

## 1. The attribution

    FFmpeg, one thread                      0.465 s   1.00
    FFmpeg, one thread, -cpuflags 0         0.656 s   1.41   (no hand-written SIMD)
    N2 spike                                1.350 s   2.90
    coreav-h264 (production)                1.534 s   3.30

Total gap, production to FFmpeg at one thread: **1.069 s**.

| row | seconds | cause |
|---|---|---|
| **Gap A — production to spike** | **0.184** | |
| chroma MC kernel | 0.144 | `CHROMA_MC_VECTOR = false` (x86.rs:29) routes every call to the scalar kernel, which LLVM vectorises to 4 samples per iteration. **89.0% of the 3,585,584 calls are at full-sample position** (xFracC = yFracC = 0), a pure copy, and still run the whole 4-tap filter. |
| deblocking filter kernel | 0.108 | our `simd::x86::filter_edge` is 128-bit, so a 16-line luma edge needs two gather/transpose/filter/scatter rounds; `per_line()`'s `_mm_setr_epi16` of 8 runtime values compiles to `vmovd` plus seven `vpinsrw`, twice per pass |
| intra prediction | 0.049 | `intra::pred_chroma` recomputes its four DC sums *inside* the 64-sample loop — 512 adds where 8 suffice — and `intra::neighbours` is an out-of-line call that materialises 32 `i32` with `checked_add`/`checked_sub` before every prediction |
| luma MC kernel | 0.046 | the scalar fallback for 4-wide and full-pel 8-wide blocks; the tap helpers stay out-of-line and every row re-checks slice length. **92.6% of luma MC calls are also at full-sample position.** |
| IDCT + residual add | 0.016 | the two sides inline differently, so only the area total is comparable |
| macroblock bookkeeping | 0.008 | parity within noise |
| picture setup/clear | −0.103 | production is *faster*: the picture-clear change |
| memmove, OS, other | −0.015 | noise |
| CABAC residual, CABAC parse, MC control, deblock control | −0.033 | **production is at parity or faster than the spike** (residual −0.015, deblock control −0.021, MC control +0.002, parse +0.001) |
| **Gap B — spike to FFmpeg** | **0.885** | |
| deblocking beyond FFmpeg | 0.261 | see below |
| FFmpeg's hand-written assembly | 0.191 | measured directly: `-cpuflags 0` costs FFmpeg 1.41× |
| unexplained | 0.433 | the shipped binary is stripped, so no symbol-level attribution is possible |

Profiling the two decoders attributes 0.221 s to the gap-A rows against
0.184 s of wall-clock difference; the 0.037 s is profiling overhead that
falls differently on the two builds. The rows, not the total, are the
useful part.

**The single most important line in this table is the last group of gap A.**
CABAC residual decode, CABAC macroblock parse, motion-compensation control
and deblocking control — the four areas that were the planned targets of
optimisation rounds 3 and beyond — are already as fast as the spike's, or
faster. All of gap A is kernel quality, in five kernels.

Dispatch policy is ruled out as a cause, by experiment: forcing the vector
chroma kernel costs +0.023 s, forcing the vector edge filter everywhere
+0.060 s, forcing the vector luma kernel +0.033 s. Every one of our vector
kernels loses to the scalar path it would replace. The gap is in the kernel
bodies, not in the `Auto` rule that chooses between them.

### The deblocking row

`-skip_loop_filter all` isolates FFmpeg's loop filter: 0.033 s on this
stream, 1.4% to 12.2% across five tuning streams. Ours costs 0.381 s
(0.208 s deriving boundary strengths, 0.173 s filtering); the spike's costs
0.294 s. Ours is roughly eleven times FFmpeg's in absolute terms, where the
decoder as a whole is three times.

A cause follows from clause 8.7.2.1 without reading anyone's source: when
two neighbouring macroblocks are both skipped, carry no coded coefficients
and use the same reference picture and motion vector, every edge between
and inside them has bS = 0. One test per macroblock pair can retire all of
its edges; we test each edge. On low-motion content most macroblocks take
that path, which is consistent with FFmpeg's deblocking share varying from
1.4% to 12.2% with content while ours stays near 25%.

Caveat, stated because it bounds the claim: some of FFmpeg's
boundary-strength derivation may happen during macroblock decode and so not
be captured by `-skip_loop_filter`. 0.033 s is therefore a *lower bound* on
its deblocking cost, and 0.261 s is an *upper bound* on what this row can
return.

## 2. What single-thread ratio is reachable

Starting from 3.30 on this stream (about 3.0 on the held-out median).

**(a) Safe Rust, under the existing policy.** The kernels crate is already
an `unsafe` exception crate, so none of this needs a new policy.
Five rows, in descending order of how well they are evidenced:

| row | seconds | evidence |
|---|---|---|
| chroma MC full-sample copy path | **−0.175** | **built and measured**, md5-identical output, gains on all five 1080p streams tried (0.07 to 0.32 s) |
| deblocking whole-macroblock bS early-out | −0.13 to −0.26 | our deblocking costs 0.381 s against FFmpeg's ≥0.033 s; the rule follows from clause 8.7.2.1; the range is the lower-bound caveat above |
| luma MC full-sample path | −0.03 to −0.08 | by analogy with the measured chroma result: 92.6% of calls qualify, against a 0.145 s kernel |
| intra prediction DC hoist | −0.02 to −0.04 | a counted inefficiency (512 adds where 8 suffice), not an estimate of unknown work |
| 256-bit deblocking filter kernel | −0.02 to −0.05 | partly inside the early-out row above, so counted small to avoid double-counting |

    1.534 − (0.375…0.605)  =  1.16…0.93 s  →  **2.0× to 2.5×**

The 2.0× end requires every row to land at its optimistic value; 2.5× is
the result of the one measured row plus half the deblocking row. Take 2.3×
as the working number.

**(b) With a narrow CABAC `unsafe` exception.** CABAC is 0.287 s of 1.523 s
(residual 0.192, parse 0.095), 19% of the decoder. The evidence against the
exception is in the attribution table: our CABAC is already *at or below*
the spike's cost, and the spike is written with no `unsafe` restrictions at
all. Bounds-check elision and unchecked table reads plausibly return 5% to
20% of 0.287 s, i.e. 0.014 to 0.057 s, which moves the ratio by 0.03 to
0.12.

    **1.9× to 2.5×** — indistinguishable from (a).

**Recommendation: do not take the CABAC `unsafe` exception.** It buys
single-digit hundredths of a ratio point against a permanent `unsafe` site
to keep, with its row in `docs/unsafe-sites.md`, its twin and its fuzz target. Our CABAC is
not the problem; our kernels are, and the kernels crate already has the
exception it needs. If the exception is wanted later for a different
reason, the measurement should be redone then; on today's numbers it is not
justified.

**(c) What nothing we know would close.** FFmpeg's assembly advantage
(0.191 s) plus the unexplained remainder (0.433 s) is 0.624 s — **58% of the
whole gap**. Matching hand-written assembly with intrinsics is possible in
principle but we have no demonstration of it; every vector kernel we have
written so far *loses* to its scalar counterpart. The 0.433 s has no
mechanism attached at all: the binary is stripped and we may not read the
source, so it cannot be decomposed further by any method available to us.

One caveat in our favour, which is why (a) can reach below the spike's
2.90×: the full-sample-position finding is a *shared* gap. Both our
decoders run the general interpolation filter for the ~90% of calls that
are a copy, so some unknown part of the 0.433 s is the same defect, and
closing it moves us past the spike on that row rather than up to it
(chroma MC becomes 0.143 s against the spike's 0.174 s). That is the
reason the (a) range starts at 2.0× and not at the spike's 2.90×. How much
more of the 0.433 s it accounts for cannot be established without reading
source we may not read.

    Floor with every row in (a) fully realised: about 2.0×.
    Floor if FFmpeg's assembly advantage were also matched: about 1.6×.

**There is no credible path to a single-thread median at or below 1.00
against FFmpeg.** Anyone who wants one has to find the 0.433 s first, and
we currently have no instrument that can see it: no hardware counters in
this VM and no symbols in the binary.

## 3. What threading would be worth

Measured on this machine, same session, quiet: FFmpeg at its default thread
count is **2.51×** its own single thread (0.463 s → 0.185 s) on 4 physical
cores and 8 SMT threads.

Slice-level parallelism is not available on real content. Every one of six
tuning streams codes **exactly one slice per picture** (676, 326, 41, 452,
394 and 829 pictures, the same number of slices each). H.264 has no
entropy-synchronised wavefront, so intra-picture parallelism needs multiple
slices and there are none. Frame-level threading with row-level reference
synchronisation is the only mechanism available, and its efficiency is
bounded by the same reference-chain dependency that holds FFmpeg to 2.51×.

**Threading does not improve our ratio against FFmpeg.** The comparison is
against FFmpeg at its default threads, which is already parallel, so

    ratio at default threads  =  single-thread ratio × 2.51 / our speedup

| our single-thread | our parallel speedup | ratio vs FFmpeg default |
|---|---|---|
| 2.0× (best of (a)) | 2.51× (matching FFmpeg) | 2.00× |
| 2.5× (worst of (a)) | 2.51× | 2.50× |
| 2.0× | 2.0× (plausible first implementation) | 2.51× |
| 2.5× | 2.0× | 3.14× |
| 2.0× | 1.6× | 3.14× |
| 2.5× | 1.6× | 3.92× |

Matching FFmpeg's parallel efficiency exactly reproduces the single-thread
ratio; falling short of it makes the ratio worse. The only way threading
improves our standing is to *exceed* FFmpeg's parallel efficiency, and both
decoders are bounded by the same dependency structure in the same streams,
so there is no reason to expect that.

This corrects an earlier framing in which threading was described as the
lever that would close the gap. It is not. It is the work required to stop
the gap widening to between 2.5× and 3.9× once the comparison is made at
default threads, which is how a user runs both tools. That still makes it
the right next step — the measured `--exact` gap at default threads is
the number a user sees — but it should be funded as parity work, not as a
route to 1.00.

## What this does not say

The decode-boundary backend is not affected by any of this. Keyframe
thumbnails through the `native` backend run at **0.437** of the `ffmpeg`
command line (CI 0.410 to 0.471) and are byte-identical to the fallback on
201 of 201 held-out files, because that operation decodes only keyframes
and the comparison is against a tool that decodes more. The user-facing
operation meets its bar; the raw decoder does not, and on the evidence
above will not.

## Method

- Decode only: ours `--discard`, FFmpeg `-f rawvideo -` to `/dev/null`,
  `-threads 1` unless stated, `-frames:v 300` against our `--frames 300`.
  Both binaries of every comparison differ only in what is being measured
  (`docs/measurements/h264-decoder-speed.md`).
- `-skip_loop_filter all` was confirmed to change decoded output before its
  timing was used.
- The two decoders are not built with identical flags: production uses the
  workspace `dist` profile (fat LTO), the spike its own `release` (thin
  LTO); neither sets `-C target-cpu`, so both are baseline x86-64 with
  runtime AVX2 detection. That can move a percent between rows; it cannot
  explain a 1.8× or 2.7× row.
- Decode counts were verified equal, not assumed: production, the spike and
  FFmpeg each produce 667,699,200 bytes with md5
  `5048310bdbb49c522789b14dfd17f5de` at 300 frames. All three decode
  bit-identical output.
- Gap-A rows come from `perf record -e cpu-clock` sampling (73k–81k samples
  a side) in interleaved round-robin runs, 9–11 repetitions. The
  `picture setup/clear` row is the least reliable — it carries most of the
  kernel-mode samples and is an allocation and page-fault row, not a
  decode-loop row. Read it as "production does less work here", not as
  headroom. Rows under 0.01 s are inside one noise band; treat them as
  parity.
- No FFmpeg, Libav, GPAC, Bento4 or x264 source was read. FFmpeg was used
  as a black box: wall-clock timings and two documented command-line
  options. Its binary is stripped, which is why gap B has an unexplained
  row instead of a function list.

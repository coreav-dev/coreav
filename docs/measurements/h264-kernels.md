# H.264 kernels: the SIMD step

What the SIMD kernels of `crates/coreav-kernels-h264` give, measured per
kernel and in the decoder. Bar C3 of `docs/design/h264.md` (every SIMD
kernel bit-identical to the reference) is met; the rest of this note is the
measurement that decides which implementation a call goes to.

## C3 equivalence: met

| Check | Result |
|---|---|
| `src/simd/tests.rs`: every vector kernel against the reference, called directly, over every shape, line count, fractional position, boundary strength and weighting | no difference, x86-64 |
| `tests/equivalence.rs`: the same through the public `Kernels` implementation, which is what the decoder calls | no difference, x86-64 |
| The conformance suite with the vector kernels on | 97 of 97 bit-exact |

The in-crate tests call the architecture's kernels directly, so a kernel the
dispatch currently routes around is still compared. Both run on arm64 in the
`h264-conformance` workflow's `kernels-arm64` job, and the suite itself runs
there on a push to main.

## Per-kernel timings, x86-64

`cargo run -p coreav-kernels-h264 --release --example bench`, on the
development machine (Intel Xeon at 2.90 GHz, AVX2). The ratio is the vector
kernel's time over the reference's, so below 1 is faster. A row whose
dispatch rule sends the call to the reference measures the two paths to the
same code, and lands near 1 with a few percent of noise.

| Kernel | Ratio | Where the call goes |
|---|---|---|
| luma interpolation 16x16, half or quarter sample | **0.71 to 0.90** | vector |
| luma interpolation 8x8, half or quarter sample | **0.65 to 0.86** | vector |
| luma interpolation 16x16, integer position | **0.72** | vector |
| luma interpolation 8x8, integer position | 1.06 | reference; vector on x86-64 since the luma interpolation round (`docs/design/h264.md`, "Kernel round: luma interpolation by position"), whose kernel copies a row with one load and one store |
| luma interpolation 4x4 | 1.00 to 1.06 | reference |
| inverse 8x8 transform | **0.50** | vector |
| inverse 4x4 transform | 1.00 | reference |
| inverse 4x4 transform, four blocks a call (`idct4x4_add_quad`, round 9; measured on the 2.50 GHz class) | **0.34** with four blocks, **0.53** with one of each pair | vector |
| prediction combining 16x16, weighted | **0.93** | vector |
| prediction combining 16x16, average | 0.92 to 1.09 | reference |
| chroma interpolation 8x8, 4x4, 2x2 | 1.00 to 1.02 | reference on x86-64 |
| deblocking, luma edge, both directions | **0.38 to 0.39** | vector |
| deblocking, chroma edge, along a row | **0.58** | vector |
| deblocking, chroma edge, down a column | 1.01 | vector since the training-set run (`h264-kernels-training.md`, candidate 1b), which gathers the lines by shuffle |

**A benchmark that measured nothing.** The deblocking rows first read 0.96
to 1.04, because the benchmark filtered random samples: with random
neighbours the threshold of Equation 8-468 fails on every line, so the
kernel measured its own early-out. The plane now steps by 15 across the
edge with a little noise, which is what a block boundary looks like, and the
filter runs on every line.

**What the table says.** A kernel that cannot be inlined — and a
`#[target_feature]` function cannot be — has to do enough work to pay for
the call. The kernels that win are the ones over a 16x16 or 8x8 block, the
8x8 transform, and the deblocking filter of a whole edge. A 4x4 block, or
four chroma samples a row, is as fast in the reference, which the compiler
inlines into the decoder and vectorizes itself.

## Per-kernel timings, arm64

The same benchmark in the `kernels-arm64` job (GitHub `ubuntu-24.04-arm`,
Neoverse). It runs on every change to the crates, so these numbers are
current by construction.

| Kernel | Ratio | Where the call goes |
|---|---|---|
| luma interpolation 16x16, half or quarter sample | **0.70 to 0.90** | vector |
| luma interpolation 8x8, half or quarter sample | **0.75 to 0.85** | vector |
| luma interpolation, integer position, and 4x4 | 1.01 to 1.10 | reference |
| inverse 8x8 transform | **0.69** | vector |
| inverse 4x4 transform | 1.01 | reference |
| prediction combining, weighted | 1.32 | reference on arm64 |
| chroma interpolation | 1.01 to 1.05 | reference |
| deblocking, luma edge, both directions | **0.66 to 0.69** | vector |
| deblocking, chroma edge, along a row | **0.89** | vector |
| deblocking, chroma edge, down a column | 1.01 | reference |

Two rules differ from x86-64's, and both are the measurement's:

- the weighted prediction goes to the reference, where x86-64's vector path
  wins; the NEON kernel splits each row into 32-bit halves, which is what
  the performance step would change;
- chroma interpolation goes to the reference on both.

## In the decoder

`cargo run -p coreav-h264 --release --example time`: both kernel sets decode
the same stream in one process, interleaved and with the order alternating,
median of nine repetitions over the first 150 frames.

| Stream | Reference | SIMD | Ratio |
|---|---|---|---|
| sd | 0.425 s | 0.405 s | 0.951 |
| sd | 0.104 s | 0.098 s | 0.943 |
| 720p | 0.379 s | 0.360 s | 0.950 |
| 1080p | 1.279 s | 1.244 s | 0.973 |
| 4k | 15.221 s | 14.869 s | 0.977 |

About 5% of the decoder's time, and the same harness with every vector path
switched off measures 0.998, so the method's own bias is below half a
percent.

**Two rules came from this harness, not from the per-kernel table.**

- *Chroma interpolation goes to the reference on x86-64.* In isolation the
  two measure the same (1.00 to 1.02); in the decoder the vector path cost
  about 5% on a 1080p stream, because the dispatch call replaces an inlined
  kernel. arm64 keeps its kernel until the `kernels-arm64` job's numbers are
  read.
- *Deblocking keeps the vector path* even though the decoder's edges filter
  far fewer lines than the benchmark's. Over four streams the median ratio
  is 0.94 with it and 0.96 without, and the gain is largest on the streams
  that filter most.

## What is left, and for whom

The profile of the decoder with these kernels puts the remaining time in
places the kernels cannot reach: the boundary-strength derivation (6%), the
inter-prediction control code (4.5%), the macroblock layer and CABAC. The
performance step (`docs/design/h264.md`, step 6) is where that goes, and the
kernel-side items it will want are the ones the N2 spike also named: one
call for both chroma planes, one call for a whole macroblock's deblocking,
and narrower luma interpolation variants.

## Spend

None: every run is local.

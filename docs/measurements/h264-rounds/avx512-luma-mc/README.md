# AVX-512 luma interpolation: rejected on local measurements

Information only: no bars were set and no gating run was made, because
the local decoder runs below showed no gain to gate. The change is kept
on the branch `rejected/h264-avx512-luma-mc` (at `7ac2f2c`) (its draft design note is in
that branch's `docs/design/h264.md`, "Kernel round: AVX-512 luma
interpolation").

What it did: the AVX-512 tier (with VNNI) computed the luma interpolation's
horizontal 6-tap from bytes with `vpdpbusd` and the centre sample j's
vertical 6-tap with `vpdpwssd`, the rest being the AVX2 kernel's code made
generic over its row types; `Auto` sent positions with xFrac not 0 to it.

All on the 2.90 GHz class (family 6, model 106), the training set, one
thread, pinned, `--discard`, three interleaved rounds, the median
per-stream ratio with a 95% bootstrap interval:

| Comparison | File | Result |
|---|---|---|
| Kernel benchmark, the tier over AVX2, every position of 16x16, 16x8, 8x8 and 8x16 (sum of times) | `mctier.txt` | 0.921, then 0.961 on the branch tip (splitting `luma_mc_ptrs` out changed what the compiler inlined) |
| Two builds made alike: the branch tip over `0290be72` (the AVX-512 deblocking round's measured commit) | `ab-tier.json` | **1.020 [1.016, 1.023]** |
| The same, with only the centre by `vpdpwssd` (horizontal back to the AVX2 code) | `ab-centre-only.json` | 1.021 [1.013, 1.030] |
| The same, with the refactor but no tier dispatch for interpolation | `ab-refactor-only.json` | 1.012 [0.995, 1.036] |
| One binary of the branch tip, both tiers' kernels against `--kernels avx2` | `local-tiers.json` | 0.986 [0.977, 0.991], as the deblocking tier alone measures (0.985 gated, `../avx512-deblock/`) |

Reading: in one binary, where placement cannot differ, the interpolation
tier adds nothing measurable to the deblocking tier (0.986 locally against
the deblocking tier's 0.985 gated, two methods), whatever the kernel
benchmark says. A likely reason, not measured here: the decoder's
interpolation waits on its reference and destination lines (round 12 of
`docs/design/h264.md`), which a kernel benchmark from the cache does not
see. The two-build losses include the refactor's and
the placement's effects as well, and were not separated further.

`tools/`: `ab2.py` (two binaries), `tiers.py` (one binary, two tiers),
`mctier.rs` (the kernel benchmark, an example of the branch).

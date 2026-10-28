# Inter prediction's excess over FFmpeg, by part, and a probe of the largest (written before the probe run)

## Where inter's excess is (from an earlier change's samples, `../profile-acf3ee2f-pmu/`)

The profile of `main` at `acf3ee2f` ranks inter prediction second overall (+0.028 of FFmpeg's cycles) and first at 4k
(+0.056). Its samples are read again here with each CoreAV function named by its path rather than its file name
(`stages_pmu-items.diff` on that directory's `stages_pmu.py`; `fn.py` lists the functions, `grp2.py` sums them by
part): `predict.rs` names two files, the safe placement (`coreav-kernels-h264/src/predict.rs`) and the SIMD block
drivers (`simd/x86/predict.rs`), which the file names alone had merged. FFmpeg is attributed at function level only.

Cycles as a share of FFmpeg's (mean per stream; `grp2.txt`), CoreAV against FFmpeg, and the excess:

| part | 720p | 1080p | 4k | all 32 |
|---|---|---|---|---|
| reference requests (`prefetch.rs`, `request`, `hint_read`, `hint_rows`; FFmpeg's `ff_prefetch_mmxext`) | 0.022 / 0.007, +0.015 | 0.022 / 0.008, +0.014 | 0.043 / 0.016, **+0.027** | 0.022 / 0.010, +0.012 |
| placement and its checks (`place`, `holds`, `Windows::fit` and the rest of the safe `predict.rs`) | 0.019 / none, +0.019 | 0.018 / none, +0.018 | 0.034 / none, **+0.034** | 0.024 / none, +0.024 |
| decoder side (`inter.rs`, `fast/mod.rs`; FFmpeg's `mc_part_420_simple_8`) | −0.003 | −0.003 | −0.003 | −0.002 |
| SIMD drivers and kernels (`simd/x86/predict.rs`, `simd/x86.rs`; FFmpeg's assembly) | 0.101 / 0.107, −0.006 | 0.104 / 0.104, −0.000 | 0.124 / 0.126, −0.002 | 0.110 / 0.115, −0.005 |

The SIMD side is at parity with FFmpeg's assembly, and the decoder side is a little below. The excess is two parts
with no counterpart of the same size in FFmpeg: the reference requests, and the placement, which turns a block's
position and vector into checked offsets for the kernels (147 instructions a macroblock over the 32 streams, 2.4% of
CoreAV's, at 5.0 instructions a cycle at 4k: executed work, not waiting).

## What is new against the ledger

- The requests have been tried every way the ledger lists: earlier and later, fewer and cheaper, spread out, one
  level down, half the chroma (`h264-rejected.md`: prefetch-helper, prefetch-reorder, prefetch-skip, round 12 and 13
  part R, request variants). Their time is the wait for the lines they ask for, moved ahead of the blocks that read
  them (the request round's finding). This profile adds nothing that changes that, so they are not probed again.
- The placement has not been a round of its own. Round 11's part P (one window test for three planes, rejected) and
  the chroma inline copy (rejected) changed the window test and the copy; neither priced the placement as a whole,
  and before this profile it was not separated from the SIMD drivers.

## Probe (output byte-identical to `main`; checked before the run)

`place2` (`probe/r35-place2`): the SIMD driver computes the block's placement a second time from opaque inputs
(`std::hint::black_box` on every argument) and discards the result. It adds one more of everything `place` does,
the interior test, and where it applies the window test, so its price is that of the placement as it runs now.

## Run and threshold (fixed now)

`ab1t.py 10` (this directory): CPU time at one thread pinned to CPU 3, `--discard`, the class frame counts, the 32
training streams, three builds rotated each round (`m`, `place2`, and `n`, a second copy of `m`'s binary), one
warm-up round; per stream the median of 10 rounds over `m`'s. The host is a development VM of the bar class (Intel Xeon
@ 2.90 GHz, family 6 model 106). `m` is `main` at `acf3ee2f`, both built alike (`cargo build --locked --profile dist
--example decode -p coreav-h264`, one toolchain, `RUSTFLAGS` unset).

- PP-1 (an earlier change's threshold, as the earlier probes apply it): a change to the placement is built only if `place2` over
  `m` is at least **1.015** (median per stream) overall or in a class.
- `n` over `m` is reported as the run's noise, not judged.
- A crashed or suspended run is a host fault and voids the run; nothing else re-runs it.

If PP-1 is met, the change aims at most of the placement's instructions, not a trim: an earlier change found that an instruction
cut needs about 1.5% of a stream's instructions to show in time, and the placement is 2.4% of them.

## Result

`probe.txt`, `probe.json` (every round's CPU seconds); one run, no host fault; `place2` = `probe/r35-place2`
`226818fd`, byte-identical to `m` at 1 and 8 threads on the 32 streams (`identity-probe.txt`).

| | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| `place2` over `m` | **1.0475** [1.0375, 1.0633], 0 of 32 faster | 1.0496 | 1.0334 | 1.0308 | **1.0876** |
| `n` over `m` (noise) | 1.0001 [0.9986, 1.0010] | 1.0005 | 1.0004 | 1.0000 | 0.9986 |

PP-1 is met, overall and in every class: a change to the placement is built. The probe over-prices it. On
`1d4b1775d78a` (callgrind, the callgrind class counts, `instructions-probe.txt`) it adds 5.2% of the instructions,
where the profile puts the placement at 2.4% of them over the 32 streams: `black_box` makes every argument and the
result go through memory, which the inlined placement does not. The price says the placement is worth a round. It is
not an estimate of the gain, which the round's own bars measure.

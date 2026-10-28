# Chroma windows above a band's chroma apron at 8 threads: the probe plan (written before the probe run)

## What led here (main `b3664291`, the development VM: Xeon @ 2.90 GHz, 4 cores, 8 logical CPUs)

At 8 threads, a count by reason of the blocks `Kernels::predict` declines (`band-slack/counts-summary.txt`)
leaves 754,451 declines over the 32 training streams that neither reach past the picture's top or bottom row nor use
samples outside the picture. They are up to 11.7% of the banded blocks (`3c593b8f0809`, 720p; 6.6% on
`65ec61f95a00`, 4.6% on `278dbf5a63bb`). None of them is declined with one thread. Split by plane (`counts-3c59.txt`):
all 70,986 on `3c593b8f0809` are chroma windows that start above the chroma rows of the band their luma window is
read from, while the luma window fits. The band is chosen by the luma window's last row. Where that row is one of
the three rows the next band's deblocking filter changes, the reader takes the next band, whose top apron holds 23
luma rows but 9 chroma rows (`TOP_CHROMA`). The chroma window then starts up to two chroma rows above it (2 * cy
can be yi - 1, and yi as low as S - 21 for band start S, so cy >= S/2 - 11 against a first row of S/2 - 9).
Neither the band slack round nor kept band views changes these blocks. The change in view is a chroma
apron two rows taller (`TOP_CHROMA` 8 + 1 + 2), so the kernel takes them.

## Probe (output byte-identical to `main` at 1 and 8 threads on the 32 streams; checked, `identity.txt`)

`capron2` (`probe/r32-capron` `e2f9d6dd`): a banded block whose chroma window starts above the chroma apron of the band its luma
window is read from, every used sample inside the picture and the window clear of the picture's top and bottom rows
(the declines the change removes), takes `predict_list_any` twice. The block's samples in the picture are saved
before the first call and put back after it. The change removes the declined path's cost less the fast path's for
these blocks, and adds two chroma rows to each band's apron copy. The probe adds the whole declined path and a
384-byte save and restore. It over-prices the removal, so it is a bound.

## Run and bars (fixed now)

`scripts/h264_parity.py` (main `b3664291`), one call per class: `--decoder base=<main> --decoder probe=<capron2>
--threads 8 --rounds 10 --shuffle-seed 20261027 --null-arm base --gate-stream 36501d448eaa.0.720p.h264
--gate-every 4`, the class frame counts, FFmpeg 9.0.2 as the harness's reference, unpinned. The harness's host
checks apply as it applies them (null deviation 3% per stream, round spread 10%, two retries, in-call gate drift 5%).
A stream still `unresolved` stays in the medians and is listed. A call in which more than two of its eight streams
end `unresolved`, a call the harness stops, or a crashed or suspended run is a host fault and is void; a void call is
run again, at most twice.
- PC-1 (an earlier change's threshold, as the r31 and r32 probes applied it): the change is built only if the probe's discard time
  over `base`'s is at least 1.015 (median per stream) overall or in a class. Otherwise no round is built and the
  record says so.
- Information: write mode, the null arm's deviations, and the ratios to FFmpeg.

## Result: void

`run.sh`, `progress.txt`: every call ended void. Under an earlier change's host checks at eight threads every stream of every
call ended `unresolved` on the 10% round-spread rule (per-command max over min 1.07 to 1.59 over 10 rounds, FFmpeg's
included; the calls last 0.17 to 0.7 s at the class frame counts): sd and 720p void three times, 1080p twice before
the script's own timeout stopped its third attempt, 4k never ran. PC-1 was not read. The probe is not run again:
the void came from the method, not from a host fault, and a method found after the run applies only to later work
with new code. An earlier change (`--min-seconds`, calibrated thresholds) has since measured the noise of the eight-thread median
over 32 streams at about 1.5% each way, the size of PC-1's 1.015 itself, so a probe of this size would not resolve.

## What follows

The change is built as new code and goes to bundle 2 as item I (`perf/h264-bundle2-i` `4776091e`:
`TOP_CHROMA` from 9 to 11, derived from `TOP_LUMA`), where the bundle's run decides it. Its own reading
(`instructions-8t-item-i.txt`, callgrind at eight threads, `--discard`, the callgrind class counts, over `main` at
`70a4cec4`; information): 3c593b8f0809 0.9907, 65ec61f95a00 0.9655, 278dbf5a63bb 0.9896, the three streams with the
most of these declines, and ec473c99c538 (sd, bands of one row, no such declines) 1.0072, the two rows more each
band copies. Output byte-identical to `main` at 1 and 8 threads on the 32 streams (`identity-item-i.txt`).

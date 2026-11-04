# Transcode decoder hosting: sandbox versus in-process

Moving the decoder library in-process changes the paired median transcode time by about 0.6% to 1.0% on these files. The VP9 `x264` and AV1 `ffmpeg` intervals lie below 1.00; the other two include no change. The baseline gap remains. This diagnosis judges no bar, changes no isolation policy and does not decide whether a native VP9 decoder is needed.

## Scope and method

The [registered method](../design/transcode-decoder-host.md) and its tool merged before any registered input command, in coreav-dev/coreav#368. Operational corrections merged in coreav-dev/coreav#374 and coreav-dev/coreav#377 before capture. Both attempts use exact merged source `4840da143cafcd71d0ac59e169cde9cc3550364a`, with no tracked changes.

The cohort is exactly the ten VP9 and six AV1 files previously timed in [round two](transcode/round-2/speed/files.jsonl), with the full hashes, sizes and stream maps in [the registration](../../bench/transcode-decoder-host/inputs.json). No file is replaced or excluded. These are previously measured files, not a fresh held-out sample.

Each file has three rounds of six complete process invocations in the registered rotated order. The baseline runs twice, as `base` and `base2`. The four CoreAV arms combine sandboxed or in-process decoding with the `ffmpeg` or `x264` encoder. Settings are medium preset, CRF 23, no scaling and copied audio with the recorded maps. Sources are primed into the page cache twice; outputs use a dedicated tmpfs. External wall time includes startup, decoding, encoding and final muxing.

Each file's arm time is the median of its three calls. Class summaries give the median across files and a 95% bootstrap interval from 10,000 paired-file resamples with seed 518. Ratios and saved seconds are computed per file before taking a class median. They are not ratios or differences of the separately reported class medians. The wide absolute-time intervals reflect the different file sizes and durations.

## Times and baseline ratios

Times are seconds. The baseline ratio is the arm's per-file median divided by that file's `base` median. “Below 1” gives the count and share of files below that reference; `base2` is the A/A control, not a CoreAV arm.

| Class | Arm | Seconds, median (95% interval) | Arm / baseline, median (95% interval) | Below 1 |
|---|---|---|---|---|
| VP9 | `base` | 2.791 (0.205 to 98.492) | 1.0000 (1.0000 to 1.0000) | 0/10 (0%) |
| VP9 | `base2` | 2.781 (0.211 to 98.166) | 1.0010 (0.9972 to 1.0085) | 5/10 (50%) |
| VP9 | `sandbox_ffmpeg` | 2.907 (0.231 to 102.583) | 1.0392 (1.0227 to 1.0652) | 1/10 (10%) |
| VP9 | `sandbox_x264` | 2.839 (0.213 to 101.512) | 1.0267 (1.0157 to 1.0513) | 1/10 (10%) |
| VP9 | `process_ffmpeg` | 2.890 (0.229 to 103.606) | 1.0498 (1.0098 to 1.0643) | 1/10 (10%) |
| VP9 | `process_x264` | 2.858 (0.211 to 101.244) | 1.0275 (1.0099 to 1.0485) | 1/10 (10%) |
| AV1 | `base` | 48.090 (1.890 to 108.941) | 1.0000 (1.0000 to 1.0000) | 0/6 (0%) |
| AV1 | `base2` | 48.005 (1.885 to 109.175) | 1.0005 (0.9968 to 1.0046) | 2/6 (33.3%) |
| AV1 | `sandbox_ffmpeg` | 49.715 (2.011 to 113.989) | 1.0457 (1.0130 to 1.0628) | 1/6 (16.7%) |
| AV1 | `sandbox_x264` | 49.706 (1.982 to 121.964) | 1.0475 (1.0214 to 1.1195) | 0/6 (0%) |
| AV1 | `process_ffmpeg` | 49.417 (1.987 to 112.824) | 1.0353 (1.0064 to 1.0518) | 1/6 (16.7%) |
| AV1 | `process_x264` | 49.131 (1.977 to 121.007) | 1.0373 (1.0154 to 1.1131) | 0/6 (0%) |

The A/A medians are 1.0010 for VP9 and 1.0005 for AV1, with both intervals spanning 1.00. The CoreAV baseline ratios do not rejudge B5, even where a point estimate is below a historical threshold.

## Paired hosting effect

The ratio below is in-process divided by sandbox for the same file and encoder. Saved seconds are sandbox minus in-process; negative values mean in-process took longer.

| Class | Encoder | In-process / sandbox, median (95% interval) | Below 1 | Seconds saved, median (95% interval) |
|---|---|---|---|---|
| VP9 | `ffmpeg` | 0.9935 (0.9892 to 1.0084) | 6/10 (60%) | 0.0015 (-0.5210 to 0.0276) |
| VP9 | `x264` | 0.9939 (0.9906 to 0.9987) | 8/10 (80%) | 0.0036 (-0.0022 to 0.3771) |
| AV1 | `ffmpeg` | 0.9898 (0.9873 to 0.9963) | 6/6 (100%) | 0.3129 (0.0093 to 1.1644) |
| AV1 | `x264` | 0.9922 (0.9855 to 1.0013) | 4/6 (66.7%) | 0.3961 (-0.0056 to 1.1463) |

The paired median reduction is small: about 0.65% and 0.61% for VP9, and 1.02% and 0.78% for AV1. Only the VP9 `x264` and AV1 `ffmpeg` ratio intervals are wholly below 1.00. This is the effect of removing the worker-hosting path while retaining the same decoder library and encoder, not a measurement of a native decoder.

### Fraction of extra seconds removed

For each file, the fraction is `(sandbox - in_process) / (sandbox - base)`. It is undefined when sandbox is at or below baseline. Undefined files are counted separately, not assigned zero, and fractions are not clipped.

| Class | Encoder | Extra seconds removed, %, median (95% interval) | Defined files | Undefined files |
|---|---|---|---|---|
| VP9 | `ffmpeg` | 6.74 (-25.00 to 45.03) | 9 | 1 |
| VP9 | `x264` | 9.93 (-12.34 to 34.71) | 9 | 1 |
| AV1 | `ffmpeg` | 18.99 (15.83 to 35.39) | 5 | 1 |
| AV1 | `x264` | 9.47 (-7.95 to 37.26) | 6 | 0 |

These fractions do not show recovery of most of the baseline gap. Several intervals include negative recovery, and a small denominator can make a per-file fraction sensitive to noise.

## Completeness and host controls

The valid attempt completes all 288 timed calls: 48 per arm, with no missing, duplicate or failed call. Every CoreAV timing receipt reports the expected libvpx or dav1d backend, actual isolation, encoder, positive frame count and geometry. Capture also checks the actual stream maps against the registration before retaining each receipt.

Before timing, all sixteen files pass the complete four-arm preflight. Each hosting pair has identical decoded frame hashes, timestamps and dimensions for its encoder, and identical copied-audio packet hashes. The retained preflight has 12,120 frames across the files per arm. The fixture-only stability check precedes every registered input command; its two three-call medians are 3.488452 and 3.505995 seconds, a 0.5029% difference.

The capture's gate is one warm-up and three timed B5 baselines without audio, before and after the cohort. Its unchanged rule voids the whole attempt above 5% median drift.

| Attempt | Before gate calls, seconds | Before median | After gate calls, seconds | After median | Drift | Outcome |
|---|---|---|---|---|---|---|
| First | 3.206583, 3.186356, 3.203254 | 3.203254 | 3.456847, 3.445159, 3.408680 | 3.445159 | +7.5518% | Void |
| Replacement | 3.526821, 3.481378, 3.483330 | 3.483330 | 3.468549, 3.459393, 3.450278 | 3.459393 | −0.6872% | Valid |

The first attempt also completed all 288 calls and sixteen preflights, but its failed gate makes every performance conclusion from it void. The registered host-fault rule permits the identical replacement. No source, binary, input, command, round or threshold changed between attempts.

An earlier source-staging pilot failed before compilation or media because its Git bundle was not self-contained. The corrected bundle and synthetic decoder fixtures passed before the registered runs. The original gate `2b811dc59ae7a0b5d15f0818876b2de5b064b4a85c9b647ae84109afa2e523e3` was lost with an ephemeral machine. Its replacement `62448e7e81fa2b9607211636400bfa2cc723347840f0af73d2de0755c3eaf6cd`, generation command and repeatability requirement were merged before capture. Gates compare drift only within each attempt, never against historical timings of the lost file.

## Host and tools

Both attempts ran on 2026-11-04 UTC, alone on standard on-demand Compute Engine `c4-standard-8` instances, with SMT disabled and four available physical cores: Intel Xeon Platinum 8581C, Debian 13, kernel `6.12.113+deb13-cloud-amd64`. Commands are not pinned to fewer cores. Both CoreAV hosting arms use Debian libvpx `1.15.0-2.1+deb13u1` and dav1d `1.5.1-1` with the same decoder thread defaults.

The clean merged build uses `cargo build --locked -p coreav-cli --release --features native-h264,dav1d,libvpx,ffmpeg-fallback`, without native VP9 or compiler/profile overrides. Rust is `1.99.1 (58cf8f9c3 2026-10-13)`, LLVM 23.1.1. The build receipt binds this command, source and executable digest.

FFmpeg and ffprobe are the pinned `n9.0.2-17-g2a571b6068-20260930` build, verified with `oracle_pin.py --verify-bin`. x264 is `0.165.3223 0480cb0`, built by `scripts/install-x264.sh`.

| Executable | SHA-256 |
|---|---|
| CoreAV | `d8dc3afe29892a5d9857e8901400c8aa151e657be6557ba1efc12b27b7b88351` |
| FFmpeg | `0d709a0bed1a43f24cd2c420878c63afea4580671b6949ad19ed124572f3bc3d` |
| ffprobe | `a5bd5e9f8d74ab2c6d7d9e2e2738ff6d67bf9613e7143e143ba8ebe9b06385fb` |
| x264 | `9d94e82089c882bb5439f38c8fa60181cc359df1195930a935e9340a7853d4a8` |

## Evidence and replay

The [valid evidence archive](transcode-decoder-host/valid/evidence.tar.gz) and [void evidence archive](transcode-decoder-host/void/evidence.tar.gz) each contain the exact captured registration, per-call times and invocations, build/tool/host provenance, paired preflight receipts, frame hashes and raw control times. Each has `capture.json`, `fixture-repeatability.json` and 32 `checks/*.framemd5` files. No numerical field is redacted or changed. Private launcher logs and resource accounting are not published.

The [valid summary](transcode-decoder-host/valid/summary.json) includes every per-file arm median and the unrounded class statistics. Media-free replay exactly reproduces the host's summary; an independent check of the 288-cell call grid and per-file median arithmetic also passes.

From this results checkout, use Python 3.11 or later to replay with the measured tool version:

```sh
(
set -eu
work=$(mktemp -d)
trap 'git worktree remove --force "$work/source" >/dev/null 2>&1 || :; rm -rf "$work"' EXIT
git worktree add --detach "$work/source" 4840da143cafcd71d0ac59e169cde9cc3550364a
mkdir "$work/valid" "$work/void"
tar -xzf docs/measurements/transcode-decoder-host/valid/evidence.tar.gz -C "$work/valid"
tar -xzf docs/measurements/transcode-decoder-host/void/evidence.tar.gz -C "$work/void"
python3 -I "$work/source/scripts/transcode_decoder_host.py" summarize --out "$work/valid"
cmp docs/measurements/transcode-decoder-host/valid/summary.json "$work/valid/summary.json"
if python3 -I "$work/source/scripts/transcode_decoder_host.py" summarize --out "$work/void" >"$work/void/replay.log" 2>&1; then
    printf '%s\n' 'Void capture was unexpectedly accepted' >&2
    exit 1
fi
grep -Fx 'ValueError: incomplete capture' "$work/void/replay.log"
)
```

The example stops if valid regeneration or the byte comparison fails. It exits zero only when the valid summary matches and the void replay produces the specific recorded-status refusal. The void capture's raw gates also independently exceed the 5% limit. The private replay directory and temporary source worktree are removed on exit.

## Limits of the conclusion

No stage profile was collected. This measures the total hosting-path effect, not an individual copy's cost or a separately identified decoder or encoder bottleneck. It supplies neither a fresh held-out speed claim nor evidence that native VP9 is required or unnecessary.

The in-process mode is diagnostic-only. The common decode admission, packet and frame checks remain, but direct C decoding has no independent worker address-space or CPU limit. Strict-safe rejects it. Ordinary behavior and decoder-isolation defaults remain unchanged.

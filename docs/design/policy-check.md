# Upload policy check

Status: Accepted. Code: `crates/coreav-policy`, `crates/coreav-cli/src/check.rs`.
Schemas: [`policy-v1.md`](../schema/policy-v1.md),
[`verdict-v1.md`](../schema/verdict-v1.md).

## What it does

An operator writes a policy: allowed container formats, stream kinds and
codecs; maximum file size, duration, overall and per-stream bit rate,
stream counts, resolution, channels and sample rate; allowed rotations. A
check probes the file with the existing `coreav-isobmff` parser, its limits
and its allowlist (no decoding; no media sample is read), and returns a
verdict: `accept`,
or `reject` with every failed rule as a machine-readable reason. This is
the policy step of upload normalization, and the gate of safe
upload probing and of a safe gate in front of existing tools:
only files that are well formed and inside the policy reach the rest of the
pipeline (an encoder, a transcription model, storage).

Surfaces:

- Library: `coreav_policy::{Policy, evaluate, Verdict}` (sans-IO: JSON in,
  JSON out) and `coreav_cli::check::{run, run_source}` (probe a path or any
  `Source`, then evaluate).
- Command: `coreav check --policy POLICY FILE` prints the verdict on stdout;
  exit 0 accept, 1 reject, 2 usage error or invalid policy, 3 I/O error
  (also a verdict that cannot be written), 141 stdout closed by its reader.

## Decisions

1. **The policy is evaluated over the `coreav.probe/1` document, not over
   the parser's `Movie`.** Every rule reads one documented value of the
   probe output, and each reason names it by JSON pointer (`path`) next to
   the pointer of the rule in the policy (`rule`). Anyone can reproduce a
   verdict from `coreav probe --scan-fragments` output, a stored probe
   document can be re-checked without the file, and the evaluator is
   container-neutral, so the Matroska probe plugs in by printing
   the same document. Rejected: rules over `coreav_isobmff::Movie` (ties the
   policy to one container, and paths would need a second mapping to the
   output users see); a separate typed probe model (a third description of
   the same values).
2. **No new dependency.** The crate depends on `serde_json` only, already in
   the workspace. The policy reader is written by hand over
   `serde_json::Value`, so that errors name the offending value by JSON
   pointer (`/streams/video/max_widht: unknown key; expected one of ...`).
   Rejected: `serde` derive (a proc-macro dependency and more code in the
   WebAssembly module, whose size is bounded, for worse error messages).
3. **Strict reading.** Unknown keys, wrong types, unknown names and
   `min_count` above `max_count` are errors: a misspelled rule must never
   pass silently. Policies are limited to 1 MiB.
4. **Deny by default where a gap would let a file through.** A stream kind
   without its object under `streams` is not allowed, and streams whose
   codec CoreAV does not identify, whose sample tables are inconsistent or
   whose data is in another file are rejected unless the policy says
   otherwise. `streams` is required, so every policy states the kinds it
   takes. A stream with more than one sample description is rejected
   unless the policy says `"ignore"`: the probe describes only the first,
   and the second can carry another codec or size. Unidentified streams can be accepted
   by sample entry type
   (`["mebx"]`), which is how a video policy tolerates a phone's timed
   metadata without tolerating video in a codec CoreAV does not read;
   `"ignore"` accepts all of them and is meant for policies that use one
   stream and do not care about the rest (the transcription example).
   Limits and allowlists that are absent do not restrict, which is what an
   absent key means everywhere else in CoreAV's schemas. Rejected:
   allowing every kind by default (an upload with an unexpected subtitle or
   data track would pass a policy written for video and audio); an
   all-or-nothing `ignore` as the only way to accept timed metadata (it
   lets an 8K MPEG-4 Part 2 track through a video policy).
5. **Rules follow the codec, not the handler.** A stream's kind for the
   policy is its codec's family (H.264, HEVC, AV1, VP9 and ProRes are
   video; AAC, MP3, Opus and PCM audio; timed text subtitle; timecode
   timecode); the probe's handler-derived `kind` counts only for a codec
   without one. Tools behind the gate decode by codec, so an H.264 track
   under a `data` handler is video to them; keyed by handler, it skipped
   every video rule. Its `video` object
   is `null`, so its size rules reject it as `unknown`.
6. **Fail closed.** A rule whose value the probe leaves `null` rejects with
   the code `unknown`. A check never accepts what it could not verify.
7. **Exact comparison.** Durations and bit rates are compared as exact
   fractions (the probe's `ticks` and `exact` values) against exact limits,
   with an overflow-free comparison. `max_seconds` is read as the shortest
   decimal that names its binary64 value (`10.01` is 1001/100 s, not the
   binary64 just below it), which is the decimal as written for up to 15
   significant digits. Rejected: reading the raw number text, which needs
   `serde_json`'s `arbitrary_precision` feature, and that feature would
   change number handling in every crate of the workspace.
   The verdict prints the exact value next to a floating-point copy.
8. **Duration is the longest of the file's and every stream's.** A movie
   header can understate how long its media lasts; using the maximum keeps
   the declared value from hiding a longer stream. One reason at most, at
   the longest value. Rejected: the movie duration alone (trusts one
   header); one reason per stream over the limit (three reasons for one
   fact on an ordinary file).
   Unidentified streams that a policy accepts are not part of it: the
   probe reports neither their kind nor their duration (decision 4).
9. **Container formats from the major brand**: `mov` (`qt  ` or no `ftyp`),
   `3gp` (`3g*`), `mp4` (any other brand). Exact brand lists were rejected
   as a first version: operators think in formats, and the list of brands in
   use is long. A brand rule can be added within `/1`.
10. **Resolution as decoded, plus `max_pixels`.** `max_width` and
    `max_height` compare the larger of the size the visual sample entry
    declares and a verified size from the codec's parameter sets and frame
    headers, by default from a scan of every sample
    ([`frame-sizes.md`](frame-sizes.md)); `max_pixels` is the
    orientation-free limit, and `rotations` limits the display matrix
    separately. A matrix that is not a quarter turn (mirrored, scaled) is
    rejected whenever `rotations` is given. Rejected: the declared size
    alone (a file can understate it); display-oriented width and height (a
    derived value that the probe does not print and that hides a rotation
    rule inside a size rule).
11. **Bit rates.** Overall: file size × 8 ÷ duration, the definition behind
    the format `bit_rate` of `ffprobe` and `coreav ffprobe`. Per stream:
    the probe's exact `bit_rate` (sample bytes × 8 ÷ duration). Peak bit
    rate over a window was left out: it needs every packet size, which
    probe does not keep, and no use case asks for it yet.
12. **An unreadable file is a verdict, not an error.** When the probe
    rejects a file (malformed, over a limit, an unsupported container), the
    check prints a reject verdict with one `unreadable` reason carrying the
    probe's own `coreav.error/1` object, so a gate gets one machine-readable
    answer for every upload. Only a file that cannot be opened or read is an
    error (exit 3), because then nothing was checked.
13. **Probe settings.** The check probes with the default allowlist, so the
    probe's `dropped_streams` are exactly the codecs CoreAV does not
    identify (`unidentified`), and reads movie fragments when a duration or
    sample size needs them, as `--scan-fragments` does, so that durations
    and bit rates of fragmented files are known.
14. **Every reason, in a fixed order.** A verdict lists every failed rule
    (an operator fixing an upload sees all problems at once) in the order
    of `verdict-v1.md`. JSON keys are printed sorted. Together these make a
    verdict a pure function of the policy and the probe output, byte for
    byte. Reasons are bounded by streams × rules (at most 1,024 tracks by
    the probe's limits, 13 rules each).

## Resources

A check reads only what the probe reads (box headers, sample tables, and
fragment headers when needed) under the probe's limits; it
never reads media samples. Memory is the probe's tables plus the probe
document and the verdict, which are bounded by the stream count. The policy
document is at most 1 MiB.

## Verification

- Unit tests of the reader and every rule (`crates/coreav-policy`), command
  tests (`crates/coreav-cli/tests/check.rs`): schemas, exit codes, policy
  errors, unreadable files, determinism.
- A fuzz target (`crates/coreav-policy/fuzz`, `check`) feeds an arbitrary
  policy and file: the verdict is consistent, serializes identically twice
  and equals the evaluation of the probe document; 60 s per push to `main`
  in CI, 20 s on a pull request (temporary).
- `scripts/policy_ref.py` is a second implementation written from the two
  schema documents. `scripts/policy_check.py` runs `coreav check` and
  checks each verdict for crashes and limits (10 s, 1 GiB), determinism
  (two runs, same bytes), spec agreement with the second implementation
  over the probe output, and precision (each reason's value is the probe
  value at its path). With `ffprobe`, it also compares the failed rules
  with those the second implementation finds in `ffprobe`'s output, over
  the rules `ffprobe` (with an independent box walk for fragmentation) can
  observe. CI runs it on every fixture and every hostile file with the
  three example policies and a tight test policy; threshold 100%.

### Corpus v1 run: bars fixed before the run (2026-10-19)

Files: every corpus v1 file (all sets and extensions).
Policies: the three in `docs/schema/policy-examples/`. Binary: `coreav`
from this branch, release build; oracle `ffprobe` 7.0.2.

| Bar | Set | Threshold |
|---|---|---|
| B1 crashes | every file × policy | 100%: exit 0 or 1 within 10 s and 1 GiB, one valid verdict |
| B2 determinism | every file × policy | 100%: two runs print the same bytes with the same exit code |
| B3 spec | every file × policy | 100%: reasons equal `policy_ref.py`'s over the probe output; an unreadable file has one `unreadable` reason carrying the probe's error |
| B4 precision | every reason | 100%: its value is the probe value at its path (or its documented derivation) |
| B5 oracle | `real` files with an ISO base media extension that both CoreAV and `ffprobe` read, × policy | at least 98% of cases agree with the `ffprobe`-derived verdict on the observable rules; every disagreement is listed and explained |
| B6 speed | held-out `real` ISO base media files | median of per-file ratios `coreav check` ÷ `ffprobe -show_format -show_streams` (process wall time, median of 5 interleaved runs each, warm page cache) at most 0.5; reported per class (fragmented, larger than 1 GB) |
| B7 memory | every ISO base media file | peak resident memory of `coreav check` at most 300 MiB (the probe's table and metadata limits plus overhead), reported against file size |

Misses on B1 to B4 block the merge. A miss on B5 is investigated, and a
cause in CoreAV is fixed before the merge; a cause in the oracle or its
mapping is recorded with evidence. B6 and B7 misses block a release claim,
not the merge. Results go below once the run ends; the bars do not change.

## Results

The corpus runs are described in
[`docs/spikes/policy-check/`](../spikes/policy-check/README.md), with
their evidence in `docs/spikes/policy-check/evidence/`. Final run: this design's
code with the probe fixes of an earlier pull request, on all 2,971 corpus v1 files
with the three example policies (8,913 cases).

| Bar | Result | Status |
|---|---|---|
| B1 crashes | 0 of 8,913 cases | Met |
| B2 determinism | 0 of 8,913 cases | Met |
| B3 spec | 0 of 8,913 cases | Met |
| B4 precision | 0 of 11,065 reasons | Met |
| B5 oracle | 3,123 of 3,123 cases: 3,039 directly (97.31%), 84 through the entries of [`known-divergences-policy.md`](../known-divergences-policy.md) | Met, counting documented divergences; direct agreement alone is below 98% |
| B6 speed | median ratio 0.133 (95% CI 0.125 to 0.141) over 303 held-out files: `coreav check` 0.99 ms, `ffprobe` 7.55 ms at the median | Met |
| B7 memory | largest peak 43,612 KiB (44.7 MB; median 3.1 MB) | Met |

The first run, on the first version of this design (`2f4212b`), met B1 to
B4 and B7 and missed B5 with 97.12% direct agreement and no documented
divergence. Its investigation found two probe defects (fixed since), errors in the oracle
mapping, and the five documented differences. This note's decisions 4 and 5 and the `multiple_descriptions` rule close ways
a file could pass a policy it breaks, and came before the final run.

# AAC-LC decode-tolerance qualification

## Plan

This qualification tests compatibility with the pinned FFmpeg 9.0.2
binary, not ISO decoder conformance or byte identity. A passing command
belongs only to a separate "compatible within decode tolerance" category.
It must never increase an exact-output count.

The candidate scope is AAC-LC in MP4, decoded at its native sample rate
and channel layout by a full build with the opt-in `aac` feature. Raw F32,
raw S16 and WAV serialization retain their separate adapter requirements.
The default build, patent policy and HE-AAC/SBR refusals do not change.
Matroska, resampling, mixing, concealed packets and unsupported timelines
are outside this qualification.

Commit this plan before running the qualification. Use only existing
development partitions of AAC-LC media already stored for the project.
Never open held-out inputs, obtain new media, read other decoder source
or consult other decoder test lists. The specification and black-box
commands against the pinned binary are the only implementation references.

### Acceptance

For every compared interleaved sample, require all of the following:

- F32 absolute error is at most 2^-16, or 0.0000152587890625, in normalized
  full-scale units. This is half of one S16 quantization step. It allows
  small floating-point evaluation differences without accepting errors
  large enough to move a sample by multiple S16 steps. All samples must
  be finite. There is no RMS, percentile or energy-only exception.
- S16 integer error is at most one LSB, including clipping boundaries.
  Compare the actual serialized S16 output, not a Python reconstruction.
- Sample rate, channel order and complete sample count equal the oracle.
  Priming and every edit-list interval must select exactly the same
  sequence. No alignment search, sample insertion, truncation to a common
  prefix or reference-length repair can grant a pass.

Use `scripts/oracle-ffmpeg.env` and the shared
`scripts/oracle-audio-args.txt` input arguments. Compare the oracle's
normal presented output, and separately compare untrimmed packet output
and the independently read edit-list model to isolate timing failures.
Retain complete-input SHA-256, build identity, oracle identity, channel
count, sample rate, both output counts and maximum errors per file.
Compare all samples, including the final packet and presentation boundary.

### Execution and failure handling

1. Select AAC-tagged MP4 development entries using their existing
   partition flags. Verify downloaded input hashes and parsed AAC-LC
   configuration. Report unsupported profiles separately, never as passes.
2. Run a short qualification preflight, then the development selection.
   Add generated boundary fixtures only as regression tests, not as a
   replacement for the existing development selection.
3. Trace failures to packet reconstruction, sample conversion or
   presentation before changing code. A real decoder correction requires
   a regression that fails before the correction and passes afterward.
   PNS waveform differences are failures of this compatibility criterion,
   even if two random realizations both meet the codec specification.
4. Admit only the demonstrated scope. If the bounds or exact timeline
   cannot hold for the candidate scope, leave AAC ineligible and report
   the evidence. Do not widen the tolerance after observing a miss or
   claim ISO conformance from this comparison.
5. Validate the deny-by-default record and its consumer, exercise the
   oracle comparison in CI, obtain independent review and require green
   checks before the eligibility rule can take effect.

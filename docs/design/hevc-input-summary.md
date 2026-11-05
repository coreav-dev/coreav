# HEVC MP4 input summaries

## Goal

Qualify native input summaries for a narrow video-only `hev1` MP4 layout. The current CLI refuses this layout even when its structured report and the same stream stored as `hvc1` agree with the pinned FFmpeg 9.0.2. Commit this plan before implementation and review it first.

The complete diagnostic must match the pinned executable, with empty standard output and the established exit status for each command. The existing deliberate version-banner suppression remains. Unsupported input must refuse before any summary bytes or use the existing original-argument fallback. This is not decoder work.

## Authority and scope

Keep every existing ISO summary guard, including unfragmented sample tables, one description, local data references, qualified geometry, constant cadence, unchanged starts, complete metadata and format timing. Admit only one video track and no other tracks. Structured output, `hvc1` admission and every other codec remain unchanged.

The configuration record must contain exactly one VPS, SPS and PPS with a consistent identifier chain, four-byte sample lengths and valid layer-zero headers. Read the retained SPS metadata through its existing grammar without allowing an incomplete metadata prefix to certify a summary. Validate the admitted PPS subset through its trailing bits. Reject unsupported syntax rather than treating two readable identifiers as a complete PPS. Optional configuration SEI is limited to completely framed `user_data_unregistered` messages.

Inspect every NAL unit of every sample, not a prefix chosen to imitate an external decoder's analysis window. Each unit must be a non-reserved layer-zero coded slice selecting that one PPS. Require an initial random-access picture, at least one slice per sample, exact length framing and at least one sample. In-band parameter sets, SEI, AUD, EOS, additional layers and unknown units refuse, including identical parameter-set repeats. This makes parameter replacement and configuration-array completeness promises irrelevant to admission.

Use ITU-T H.265 clauses 7.3.1.2, 7.3.2.1 through 7.3.2.3, 7.3.5, 7.3.6.1 and the associated semantics for NAL headers, parameter-set identifiers, SEI framing and slice selectors. The existing reviewed configuration-record reader is the record-layout authority. Text spelling is qualified against the pinned black-box executable, never another implementation's source.

## Implementation boundary

Keep configuration and slice-prefix validation inside the CLI video metadata owner. Reuse the SPS reader and the existing strict emulation-prevention owner rather than copying either grammar. Keep the ordinary SPS wrapper's result unchanged for structured reports and `hvc1`.

The ISO adapter performs a summary-only bounded sample walk through the existing packet iterator and source. Read a fixed-size prefix per NAL, use checked extents, and charge sample and NAL visits against an explicit bounded counter. Source and iterator failures cannot become successful completeness. No full frame allocation, decoder dependency, unsafe code, new public API or modified resource limit is needed. Whole-track header inspection has an intentional extra read cost for this newly admitted summary; structured probes do not perform it.

## Verification

Use generated development media only. Positive controls cover ordinary `hev1`, a later CRA picture, metadata and unchanged `hvc1`. Compare the entire stderr and stdout plus exit status for bare `ffprobe`, explicit-input `ffprobe` and no-output `ffmpeg`, including the existing log-level behavior. Preserve complete JSON bytes on the same inputs.

Negative controls cover in-band parameter sets, AUD, picture-hash SEI, mixed tracks, malformed configuration and PPS syntax, identifier mismatches, reserved units, lengths, layers, truncated reads and work limits. Prove atomic refusal and exact real fallback with unchanged arguments. Unit tests exercise the configuration, slice selector and source-error boundaries independently. Generated fixtures are not committed media and establish no command-coverage total.

Run affected Rust and Python tests, formatting, Clippy, public-text and pull-request shape checks. Finish with independent plan-first review, final-head CI and the canonical similarity guard. Do not change frozen inputs, oracle policy, judging, thresholds or published counts. No corpus, held-out, timing or fuzz campaign is part of this change.

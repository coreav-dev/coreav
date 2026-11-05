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

## Adapter scope

The compatibility adapter, not the native decoder, owns the pinned
presentation policy. It decodes listed MP4 packets in order, applies the
first packet's skip count across frames, omits empty-edit silence and
computes terminal padding from the listed final packet's shifted time.
This preserves iTunes delay metadata and the pin's empty-edit padding
policy without changing `decode-audio` or its sample-exact edit model.

Only nonfragmented tracks with no edits or one supported media edit,
optionally after one empty edit, are candidates. Track ticks must be
native audio samples. Configuration must explicitly declare mono or
stereo AAC-LC. Every packet must decode without damage or intensity
under an all-ones M/S mask. No rate or channel conversion is admitted.
The original branch additionally requires no PNS. The separately planned
[PNS extension](aac-pns-eligibility.md) admits separable noise bands using
their coded energies and the unchanged deterministic projection bars;
it does not compare complete noise-containing waveforms pointwise.
Other timelines and profiles refuse before output rather than guessing.
The preflight separately decodes every stored packet, including packets
outside the presented interval, before rendering the probe's packet
listing. This proves the content predicates over the complete stream. Gapless tags
must be a single complete twelve-field iTunes item in a movie-level
`mdir` metadata list. Incomplete metadata, absent handlers, duplicate tags,
composition offsets and multiple sample descriptions refuse.

Admission scans and decodes the complete selected packet listing into a
discard sink before opening any output. Both passes check the predicates.
Each scan permits at most 16,384 packets and 64 MiB of encoded data,
with at most 16 MiB per packet and two 1,024-sample channels of decoded
scratch. Existing MP4 limits bound parsing. Preflight performs a full-stream
scan and a presentation decode without output. The output pass repeats
both, for at most four bounded decodes. An excluded PNS tool combination
in a late packet cannot qualify because an earlier packet was clean.

## Qualification evidence

The initial bounded development selection contained twelve AAC-LC files. Nine
passed complete F32 and S16 sample comparisons through raw stdout, raw
files, WAV stdout and WAV files. Their largest observed F32 error is
3.5762786865234375e-7 and their largest S16 error is one LSB. Three contain
PNS and were refused before writing output in that registration. The
PNS extension reports its later qualification separately. No result contributes to an exact
count. Input identities and individual results are retained separately.
This is PCM sample compatibility. S16 compatibility WAV framing reuses
the existing pinned encoder-metadata header, including known file sizes
and unknown pipe sizes. Default input metadata that is not serialized,
or chapters, retain the prior native WAV output instead of falling back;
that complete output is not qualified against the pin.
The complete S16 framing is compared with the pin independently of the
registered PCM comparison, using explicit metadata discard for decoder
qualification; prior native metadata behavior has separate CLI regressions.
This does not make AAC output byte-exact;
F32 WAV framing and general metadata copying are not certified here.

Four initial count differences were isolated with black-box metadata
controls. Removing an iTunes delay item adds precisely its 2,112 priming
samples. Extending a media header by 28 samples adds exactly those final
samples. Removing one leading-empty edit yields a packet control from
which removing 2,112 initial samples reproduces the original complete
oracle output. Another empty edit shifts the terminal packet by 44
samples and the pin discards exactly those final samples. All four
predicted complete oracle byte sequences matched their controls. A fifth
file's 3,719-sample leading empty edit is omitted by the pin. These are
adapter presentation policies, not reasons to change native decoding.

`scripts/aac_pcm_eligibility.py` reruns the comparison with the verified
pin and shared audio input arguments. Without supplied files it generates
mono/stereo tone and switching fixtures at 22.05, 44.1 and 48 kHz, plus
no-edit, empty-edit, media-header, irregular-clock and canonical iTunes
controls. PNS, all-MS intensity and noncanonical gapless metadata
must refuse without stdout or destination changes
and must replay through the actual pinned fallback. Comparator mutations
reject a one-sample count change, nonfinite values, channel reordering,
terminal corruption and a single sample beyond either bound. CI runs
this driver beside the unchanged AAC vector and agreement gates.

The registered synthetic H.264/AAC command representative contains PNS
in 94 frames. It remains ineligible; this registration does not qualify
its AAC-to-PCM command shapes or change their coverage count.

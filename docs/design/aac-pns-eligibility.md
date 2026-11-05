# AAC-LC PNS decode-tolerance qualification

## Plan

### Fresh energy criterion

The initial 1/1024 relative-energy candidate failed its first synthetic
control: PCM reanalysis reached 0.189048% energy error for CoreAV and
0.208779% for the pin. Native pre-synthesis coefficients had only
0.0000371442% energy error; F32 synthesis and reanalysis of quiet bands
introduced the larger error. Independent F64 synthesis and analysis recovered
the native spectrum within 3.66e-16. The initial candidate remains failed.

This amendment registers a fresh per-band energy criterion of ±0.1 dB,
using the existing project noise-level tolerance independently for each
individual band and window. The deterministic projection, coefficient,
packing, complete count, priming and timeline bars below do not change.
The amendment is committed before any development-file comparison; only
the synthetic control was compared under the initial candidate.

This extends the separate "compatible within decode tolerance" category
established by `aac-pcm-eligibility.md`. It never grants exact-output or ISO
conformance eligibility. The native decoder keeps its own noise generator;
neither its seed nor the pinned decoder's random sequence is a target.

Commit this plan before any decoder comparison. Use only the existing twelve
development inputs and, if necessary, further development entries selected by
the same rule: `holdout == false`, `set == real`, AAC audio, MP4/M4A/MOV,
at most 16 MiB, ordered by complete input SHA-256. Verify input hashes. Do not
open held-out media or any other decoder's source. Use the verified pinned
FFmpeg binary and the shared audio input arguments. No new media acquisition,
default feature change, HE-AAC/SBR admission or noise-seed matching is in scope.

### Fixed numerical acceptance

All calculations below use finite values and complete outputs. No alignment
search, common-prefix comparison, threshold adjustment after a miss or
whole-stream energy average can grant a pass.

For a window with N coefficients, define

`C[n,k] = cos(pi/N * (n + (N+1)/2) * (k+1/2))`.

The native normalized synthesis is `sum_k X[k] C[n,k]`, followed by the
bitstream's window and overlap-add. Analysis is its windowed transpose times
`2/N`. Analyse complete untrimmed packet-domain PCM, not the edited presentation.
Use the coded window sequence, grouping, band boundaries and previous/current
window shapes for every channel. Short windows have N = 128; long windows
have N = 1024. A diagnostic-only export of native decoded spectra and coded
metadata is allowed, but it must not be linked into a production build.

The checker independently constructs analysis and synthesis in F64 from these
definitions. It must first pass perfect-reconstruction controls for every
admitted transition and both window shapes, including initial and terminal
overlap. A round trip alone is not decoder qualification: both actual decoded
PCM outputs must be analysed, and the native diagnostic spectrum must agree
with its own reanalysed PCM.

For each PNS band of each individual window, the coded noise scalefactor `s`
sets normalized coefficient energy

`E = (2^(s/4) / (32768*N))^2`.

Require `abs(10*log10(sum_k X[k]^2 / E)) <= 0.1` independently for CoreAV and
FFmpeg. Equivalently the energy ratio is inclusively between `10^-0.01`
and `10^0.01` (about -2.28% to +2.33%). E and measured energy must be positive
and finite. This applies the existing project noise-level tolerance per
individual band rather than averaging a block or stream, while remaining
far below a scalefactor step (a factor of sqrt(2) in energy).
There is no absolute-energy exception
that could accept a missing quiet band. Report numerical ill-conditioning
as unqualified, not as a pass or permission to widen this limit.

For every non-PNS bin, require coefficient difference at most
`2^-16 * sum_n abs((2/N)*w[n]*C[n,k])`; this is the necessary coefficient
bound implied by the established F32 sample bar. This bin-wise condition
alone is not sufficient. Also zero precisely the coded PNS bins in each
analysed spectrum, independently resynthesize the complete non-PNS projection,
and compare every projected sample: F32 absolute difference at most 2^-16,
and independently rounded/clipped S16 difference at most one LSB. Check
all bins, including uncoded zero bands, and all projection boundary samples.

Actual serialized S16 cannot be linearly separated into noise bands after
rounding and clipping. Check each decoder's actual S16 output against the
quantization of its own F32 output, with at most one LSB of conversion error.
Check native file/stdout packing and WAV fields independently. The complete
PNS-containing waveforms are deliberately not subject to pointwise cross-decoder
identity or the non-PNS sample bounds. This distinction must remain explicit
in the rule and evidence; it is not a looser whole-waveform error threshold.

The complete presented sample count, sample rate, channel order, priming and
edit-list timeline must match the pinned decoder exactly, as in the earlier
qualification. Relate the normal presented output to the untrimmed packet
output using the existing adapter's independently checked presentation policy.
Do not infer an alignment from samples, or repair an output using the oracle's
length. Preserve native `decode-audio` presentation behaviour.

### Separability and boundaries

- Admit only explicit mono/stereo LC with 1024-sample frames, the existing
  bounded nonfragmented MP4 timelines, and clean finite decoding. The earlier
  no-PNS scope remains unchanged.
- Valid long/start/short/stop transitions and sine/KBD shape changes are
  candidates only with the exact AAC windows and complete neighbouring PCM.
  The production predicate must reject illegal transitions. Eight-short
  grouping does not combine noise energies across windows.
- A nonzero-order TNS filter whose actual coefficient range intersects any
  PNS band is ineligible. TNS outside noise bands can retain the deterministic
  comparison; no inverse-TNS experiment may waive this exclusion.
- M/S-correlated PNS, intensity sourced from a PNS band, and coupling-channel
  elements in a PNS stream remain ineligible in this change. Their joint
  energy/correlation contracts are not established by independent band norms.
  The earlier all-ones-M/S intensity refusal also remains.
- Obtain untrimmed output by transporting the unchanged stored packets in a
  diagnostic ADTS stream with no MP4 edits or gapless tags. A packet must fit
  the ADTS frame-length field. Append spec-constructed zero-spectrum packets
  with legal closing transitions to expose the final stored overlap. Check
  the padding packet metadata and exact decoded length; padding is never
  included in a development result's presented count.
- Reject damaged frames, unsupported extension data, incomplete diagnostics,
  unsupported window histories, missing closing overlap and nonfinite values.
  Scan every stored packet, including packets omitted from presentation,
  before any production output opens. Retain the prior 16,384-packet, 64-MiB
  encoded-data and 16-MiB-per-packet limits; diagnostic storage is bounded
  separately and is never a production allocation.

The presented listing must retain every stored packet in its original order
and start at the first stored byte position. This keeps the same decoder
initial state for the packet-domain and presented checks. The production
preflight checks the first position and complete count; qualification checks
every position against the stored table. Each decoder's complete presented
F32 PCM must equal the metadata-predicted interval of its own untrimmed
output. Explicit terminal discard metadata must agree with the independently
computed end. These are identity checks within each decoder, not an alignment
search or a comparison between independent noise realizations.

### Execution and retained evidence

1. Prove analysis/synthesis against independently constructed basis and
   boundary controls before using a decoder result. Negative controls must
   reject a changed deterministic bin, a PNS energy change larger than the
   fixed bound, a missing final overlap, count drift, a wrong band mask,
   illegal window history and each excluded stereo/TNS/coupling class.
2. Export native coded metadata and spectra through an opt-in diagnostic
   build. Keep ordinary decoding free of diagnostic buffers, file writes
   and callbacks. Production admission uses only bounded content predicates.
3. Check generated fixtures with the pinned black-box oracle. Then run the
   existing twelve development files, adding further hash-ordered development
   PNS entries only if needed to exercise an admitted class. Report pass,
   ineligible and failure separately, with a concrete reason per input.
4. Retain complete input and binary hashes, qualifier identity, oracle
   identity, window and PNS-band counts, predicate failures, complete output
   counts, maximum deterministic-bin/projection errors and maximum per-band
   energy errors for each decoder. No unexplained failure can be converted
   into a content exclusion after observation.
5. Recheck the unchanged registered command representative and report its
   exact admission outcome. No command count changes before qualification
   and the separate command-shape recheck.
6. Ship the checker, production predicates, strict rule JSON/consumer updates,
   documentation and generated oracle CI in one PR after this first plan
   commit. Require green CI, the similarity guard and independent review.

No speed claim or default-build change is part of this work.

## Qualification evidence

The existing twelve development inputs give ten passes and two refusals.
The refusals are a fragmented input and an input with TNS intersecting PNS.
The newly admitted development input has three noise bands, exact 441,300
presented samples per channel, maximum energy error 0.001171 dB for CoreAV
and 0.000701 dB for the pin, and deterministic projection differences at
most 2.385e-7 F32 and one S16 LSB.

The unchanged registered command representative also qualifies: exact
96,000 presented samples, 824 individual PNS bands, maximum energy errors
0.008219 dB and 0.009058 dB, and projection differences at most 6.873e-8 F32
and one S16 LSB. It includes short windows and shape changes. Command-shape
counts remain separate from this qualification.

Generated comparisons include independent mono/stereo PNS, and late-packet
TNS, shared M/S noise and illegal-window refusals. Refused inputs preserve
their destinations and replay through the actual pinned fallback. Complete
raw file/stdout and WAV file/stdout output, format fields, per-decoder S16
packing and exact presentation intervals are checked. No result belongs
to an exact-output or ISO conformance count. Retained reports bind input,
binary, diagnostic, qualifier, transform checker and oracle identities.

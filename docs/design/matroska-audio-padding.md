# Signed Matroska audio padding

## Plan

[RFC 9559 section 5.1.3.5.7](https://www.rfc-editor.org/rfc/rfc9559.html#section-5.1.3.5.7)
defines positive DiscardPadding at a Block's end and negative padding at
its beginning. The
[Opus codec mapping](https://www.matroska.org/technical/codec_specs.html#a_opus)
separately binds CodecDelay to OpusHead pre-skip. Both affect presentation,
not the samples consumed to establish decoder state.

1. Preserve the signed metadata at the audio boundary by converting it
   into separate beginning and end sample trims. Use unsigned magnitude
   conversion even for the minimum signed value.
2. Carry both trims through serial and queued packet commit. Decode the
   whole packet, then select the kept PCM range; concealed silence uses
   the same count rule. Preserve existing positive-padding accounting,
   limits, warnings and CodecDelay behavior.
3. Refuse nonzero padding on laced Blocks and on packets requiring MP3
   reframing. Existing metadata repeats a Block's padding on every lace,
   and reframing loses the packet's trim. Neither is a correct generic
   padding implementation. Unpadded laces remain supported; full
   block-scoped lacing support is not added here.
4. Add exact ordered-output and count regressions, including a 48-sample
   beginning trim in SILK, Hybrid and CELT, positive interior and terminal
   controls, CodecDelay, signed extremes, concealment and pooled commit.
   Compare generated fixtures with pinned FFmpeg 9.0.2 where it follows
   the specification. Document any oracle disagreement without changing
   existing waveform acceptance criteria.
5. Run audio tests, lint and conformance CI and the clean-room similarity
   guard. This correction does not enable Opus in the PCM command.

Only generated development inputs and public specifications are used.
No other decoder source, held-out input or performance run is involved.
The separate MP4 edit-list correction is not part of this branch.

## Implemented boundary

`Trim` carries beginning and end sample counts through the same serial
and queued packet paths. Signed nanoseconds are converted once, with
`unsigned_abs()` and the existing nearest-sample conversion. Ordered
commit decodes the whole packet, then emits the kept count with a
sample-aligned source offset. Concealed silence follows the same count
rule. Oversized padding saturates at the packet's decoded length; even
the minimum signed nanosecond value cannot overflow its magnitude.

The existing compacted presentation clock, output limit and CodecDelay
plan are unchanged. Decoder state always consumes the full packet.
Priming copies in pooled decoding never commit presentation samples, so
they do not apply a trim twice.

Nonzero padding on laced Blocks is explicitly unsupported. The demuxer
exposes Block metadata on each lace; treating it as a per-frame trim
would repeat the operation. Full support needs block-scoped beginning
and terminal buffering across decoded frames, not a first-or-last-lace
shortcut. Unpadded laces remain supported. Padded MP3 packets that require
reframing are also refused because splitting them would otherwise lose
their presentation boundary.

## Regressions and oracle comparison

Five Rust regressions cover mode-specific 48-sample trims, complete S16
and F32 byte order in serial and multi-job pooled decoding, exact output
limits, concealed silence, signed extremes, and the explicit lacing and
reframing refusals. The existing tests retain their expected output;
positive padding is unchanged for the supported packet-aligned path.

`scripts/opus_matroska_padding.py` generates 4,800 input samples and
independently reads the Ogg Opus packets, pre-skip and EOS granule before
wrapping those same payloads in Matroska. The Opus mapping's CodecDelay
and terminal padding yield 4,800 presented samples in all three modes.
Adding either -1 ms beginning padding or +1 ms end padding to the second
Block yields 4,752 samples. Low-delay CELT is stereo; SILK and Hybrid
are mono. A native untrimmed decode supplies sample values only, never
the expected count or trim positions. Complete native S16 and F32 output
must equal those independently sliced samples at one and eight threads.

Pinned default FFmpeg 9.0.2 agrees with all eighteen mode, sign and
format controls' specified counts. No numerical oracle divergence is
introduced or accepted. Decoder waveform differences still follow the
existing Opus conformance bar; these controls do not assert byte identity
between independent lossy decoders. The report is retained by the
audio-conformance workflow.

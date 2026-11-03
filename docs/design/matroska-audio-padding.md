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

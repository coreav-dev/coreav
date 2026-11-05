# Integer WAVE input to WAV output

Plan committed before implementation. Refs coreav-dev/coreav#25.

## Scope

Support standalone integer PCM WAVE input to a WAV file with unchanged mono/stereo channels and sample rate, using the existing exact WAVE-to-S16 conversion. This closes the WAV-source form of `-ac 1 -ar 48000 -f wav`, without claiming that its frozen AAC representative now passes or changing any historical measurement. Default output codec and explicit `pcm_s16le` must behave identically to pinned FFmpeg.

Reuse the existing bounded WAVE parser, typed PCM data span, integer packing and full-input preflight. Do not add an input parser, decoder, resampler or channel mixer. Float-to-S16, changed rate/channels, timing, stdout WAV and unproved metadata remain refused. Keep the existing container-decoded WAV jobs and general library WAV serialization unchanged.

## Implementation

Route only standalone WAVE-to-WAV commands through the exact raw-audio planner. Admit integer U8/S16/S24/S32 storage already qualified for S16 packing. Default metadata accepts no tags other than the input encoder; explicit metadata stripping may discard source tags. Chapters and stream metadata mappings remain unsupported.

Derive the output sample count from the complete preflight pass, check RIFF size bounds before opening output, and emit a finalized WAV header followed by the second bounded conversion pass. Match the pinned file's INFO encoder chunk and RIFF/data sizes, not only its decoded samples. The emitted encoder identifier is observed black-box output, not a new FFmpeg pin owner. Require execution pass metadata and written payload length to agree with preflight. Use the shared atomic file owner, preserving same-file checks and terminal errors after execution begins.

## Verification

Compare complete output bytes with checksum-verified FFmpeg 9.0.2 for mono/stereo integer sources, default/explicit codecs, unchanged channel/rate options, metadata stripping, extrema, empty/short and multi-buffer inputs. Include a high-rate extensible-header control. Test unproved tags, float sources, changed channels/rates, damaged inputs and aliases without destination replacement or staging residue. Retain the existing raw-audio tests unchanged. Run focused CLI tests, formatting, Clippy, public-text checks and the exact-head similarity guard. No held-out inputs, frozen records, tolerances or baselines change.

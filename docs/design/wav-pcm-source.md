# Bounded WAVE PCM input

Plan committed before implementation and generated comparisons. Refs coreav-dev/coreav#25. Review this plan commit first.

## Scope and evidence

Feed standalone WAVE PCM into the registered headerless F32/S16 pipeline without loading a data chunk or decoded file into memory. Reuse the landed RIFF/WAVE, RF64 and BW64 parser, not a second parser. Its current entry returns an ffprobe report only, so first expose the same parse result's validated sample representation and data span through a crate-private interface. Existing ffprobe bytes, truncation reporting, metadata policy and caller limits must remain unchanged.

The representation candidates are unsigned 8-bit PCM, signed 16/24/32-bit little-endian PCM and finite IEEE F32, with one or two unchanged channels. Admit only complete, full-precision storage and conventional mono/stereo or unspecified channel masks. F64, reduced valid precision, RIFX, compressed WAVE, unsupported metadata, multiple data chunks and incomplete declared data remain on fallback. A probe's ability to describe another representation does not register its decoding.

Use the Microsoft RIFF, WAVEFORMATEX and WAVEFORMATEXTENSIBLE references linked in [standalone audio probing](standalone-audio-probe.md), EBU Tech 3306 and ITU-R BS.2088. Compare only with checksum-verified FFmpeg 9.0.2 executables as black boxes. No other implementation source, new resampler arithmetic, coefficient characterization, frozen command record or held-out input changes are included.

## Ownership and interfaces

The existing WAVE parser remains the sole owner of RIFF extents, padding, ds64 substitutions, format validity and metadata bounds. Return its report together with a typed PCM descriptor containing storage representation, valid precision, rate, channels, speaker mask, data offset, declared data length and available data length. Keep the current report-only entry as its projection. The source adapter consumes that descriptor rather than recovering offsets or codec facts from display strings.

Resolve WAVE source eligibility separately from channel, rate and final packing eligibility. The raw-audio planner owns all command-shape refusals, selected-stream and effective-codec checks. Only headerless output reaches the new source path; other commands retain their existing refusal. Parsing failure or a caller-limit/source-I/O failure is terminal, not permission to fall back. An explicitly unsupported representation or unregistered stage can fall back before output.

Represent container-decoder and PCM-source pass metadata as distinct variants; do not manufacture a compressed-codec summary for PCM. WAVE execution validates its descriptor against the plan, reads only the admitted data span in frame-aligned chunks of at most 64 KiB and feeds the existing bounded S16/F32 conversion state. Shared conversion and output ownership retain final drain, atomic files, stdout flush and EPIPE behavior. PCM source packing starts no decoder workers, just as declared raw S16 input does.

Integer-to-S16 representation conversion and integer-to-F32 scaling must match the pinned oracle before registration. F32 input stays unquantized until final packing; reject nonfinite samples during preflight. Do not use intermediate S16 to implement F32 rate conversion. Every changed-channel stage remains denied. Changed-rate admission keeps the existing Linux x86-64 boundary and supported-rate tables; complete WAVE-source compositions need their own oracle comparisons before being enabled. No short identity result qualifies another rate or platform.

Reuse the existing encoded-stdin spool and original-argument replay, with its 1 GiB cap and cleanup owner. Preflight the complete bounded source to a discard sink before opening output. Reparse and validate the source for execution, compare pass metadata and make all post-publication errors terminal. Do not spool decoded samples or retry fallback after output begins.

## Verification

Generate every input from deterministic sample equations. Compare complete native output with pinned FFmpeg, without trimming or tolerance, for each registered representation and F32/S16 destination, mono/stereo, implicit and explicit codec, files and encoded stdin, RIFF/RF64/BW64, extensible headers and known metadata/padding. Include signed extrema, rounding boundaries, signed zero, short and empty input, awkward chunk partitions and complete output lengths. Unsupported F64, reduced valid precision and nonfinite floats must fail before output and retain original-byte fallback where applicable.

Parser tests prove that the descriptor points only to the data payload, excludes trailing metadata/padding and retains checked declared/available bounds without reading payloads. Preserve the existing probe parity and caller-limit/source-error tests. Adapter tests bound requested read sizes and conversion storage independently of source length, cover partial frames and late source failures, and prove that unsupported headers do not create output. Lifecycle tests cover stdin replay and spool cleanup, atomic destination preservation, same-file aliases, quiet final-flush EPIPE and no fallback after publication.

Extend the maintained generated raw-audio checker and its mutation tests; existing CI owns Linux composed-rate qualification and refusal on other platforms. Run the smallest relevant local suites, complete CLI tests, Clippy, formatting, help snapshots and policy checks, then final-head CI and the exact-head similarity guard. This PR makes no throughput or wall-clock claim. Keep the startup guard's baseline and 5% tolerance unchanged, and report any remaining instruction cost.

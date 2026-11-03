# Registered raw audio output

Plan committed before implementation and generated comparisons. Refs coreav-dev/coreav#25. Review this plan commit before the implementation commits.

`coreav ffmpeg` will accept raw `f32le` and `s16le` output, including standard output, with optional output `-ar` and `-ac` and input `-threads`. A command runs natively only when every stage has an eligibility registration against the pinned FFmpeg 9.0.2. Channel conversion, rate conversion and packing require exact registration. Decoder registration distinguishes `exact` and `conformance`, as described below. Otherwise the command uses the existing fallback, or exits 4 under `--strict-safe`.

## Existing components

The audio decoder has native FLAC, MP3 and Opus and opt-in AAC-LC, consuming MP4/MOV or Matroska/WebM through a known-length positioned `Source`. Its `Sink` writes interleaved S16 or F32 chunks without collecting whole-file decoded PCM. The compatibility PCM owner currently writes WAV and raw S16 files and deliberately refuses lossy sources. Decoder conformance and the ability to serialize samples do not establish exact FFmpeg output.

Two rate-conversion policies already exist. The decoded-audio converter uses a normalized BS.775 downmix and a silence-extended sinc filter with ceil-scaled lengths. It intentionally does not promise FFmpeg sample identity. The raw S16 compatibility path has separate coefficients, reflected edges and draining, verified with exact generated comparisons. Do not replace one policy with the other or weaken either registration.

Native stdin ingestion is absent. The HTTP cache is not a stdin spool. The CLI already has a binary stdout writer and quiet exit 141 for a closed reader.

## Eligibility data and execution

Use one deny-by-default registry for exact decoder representations, channel conversions, rate conversions and serialization paths. A plan resolves the complete sequence before output creation. Missing registrations are refusals, not invitations to use a tolerant native path. Keep the registry beside its compatibility planner and name the evidence for every admitted nonidentity stage. Codec execution uses the existing build-aware decoder API, so proving a decoder exact requires a registration change rather than another codec branch in CLI execution.

Register lossless FLAC representations covered by generated identity tests and explicitly declared raw S16 as exact. The decoder conformance registrations below separately admit qualified MPEG-1 Layer III in MP4. AAC and Opus remain refused. Known presentation differences and unsupported layouts remain outside admission, even for a registered codec. Standalone WAV and FLAC probing does not itself provide audio decoding. Reuse a landed input API if one is available; do not add a new decoder or duplicate a container parser in this change.

Identity channel and rate stages are exact. Test the existing mono/stereo downmix separately with the pinned oracle before registering any changed-channel path. F32 rematrixing is not assumed exact because FFmpeg's default float gain normalization differs. An empty changed-channel registry is valid; the execution stage still exists and refuses unregistered paths.

The S16 compatibility kernel can serve decoded **mono 16-bit FLAC to mono S16 at 16 kHz** if an adapter feeds exactly the decoded S16 frames and preserves its startup, edges, arithmetic and drain. Validate complete commands against the pinned default oracle before registration. Stereo S16 rate conversion with unchanged channels is the analogous candidate. Restrict admission to the existing coefficient and rate bounds.

It does **not** establish the usual stereo-to-16-kHz-mono combined conversion. Quantizing a fractional mono mix to S16 before resampling can change the result. Nor does it establish decoded 24-bit input, F32 rate output, or a lossy decoder's parity. Such combinations remain on fallback. No new resampler arithmetic or coefficient characterization is planned.

Adapt the existing S16 rate engine to bounded sequential input rather than writing whole decoded PCM to a temporary file. Reuse its coefficient construction and final quantizer. Store only the filter window, startup and tail context and bounded input/output chunks. At EOF use the existing complete-frame length calculation and reflected tail. Preserve existing sized raw S16 callers and compare chunk partitions against them.

## Input and output lifecycle

Recognize only stdin descriptors `-`, `pipe:` and `pipe:0` and stdout descriptors `-`, `pipe:` and `pipe:1`; other descriptors remain unsupported. Parse and validate the command before consuming stdin.

Container input requires positioned reads, so spool **encoded input only** to an exclusively created temporary file using bounded reads and a fixed byte cap. Keep ownership through native preflight and execution, remove it on every exit, and replay it from offset zero if native preflight refuses. The fallback retains the original argument vector. Do not send fallback an exhausted stdin or a temporary filename in place of the person's pipe argument. Explicit raw S16 stdin can stream directly after its declared format and full command have passed eligibility.

For decoded files and spooled containers, validate the complete presentation with a discard sink before committing stdout. This bounded-memory extra decode avoids late unsupported features causing partial native bytes followed by fallback bytes. Do not buffer decoded audio as an alternative. Failures after output starts are terminal and never invoke fallback. Document the extra decode cost; no speed claim is planned.

File output retains atomic replacement, overwrite and alias checks. Raw stdout uses the shared buffered binary writer, checks flush errors, emits no summary to stdout, and returns 141 without diagnostics on EPIPE. Apply packet and output-sample limits to both preflight and execution. Explicit raw stdin with malformed trailing frames fails rather than retrying an already consumed stream.

Existing explicit raw S16 file input to S16 file output retains its sized-input compatibility adapter, just like raw input to WAV. Do not apply the new pipe/F32 platform registrations to that pre-existing command. Its historical rate behavior is unchanged and does not establish a new exact registration.

## Verification

Generate deterministic media from our own sample equations, using only the checksum-verified pinned oracle as a black box. Run short local fixture checks and unit tests on the development host. Existing CI runs the maintained checks; larger matrices belong in GitHub Actions. Run no held-out or judged inputs before merge and change no frozen record, roster, bar or oracle predicate.

Compare full raw output bytes, with no trimming, alignment or tolerance. Cover F32 and S16, implicit and explicit codec selection, unchanged format/rate/channels, mono/stereo, FLAC sample widths admitted by the registry, MP4 and Matroska, input thread options, files and both pipe directions. Compare encoded stdin and file inputs to the same oracle output. Check the candidate decoded S16 rate path at 44.1 and 48 kHz to 16 kHz, short startup/drain boundaries, extrema and irregular samples. Register only paths whose complete outputs agree.

Unit tests cover empty registrations, rejected decoder/precision/layout combinations, exact-stage composition, byte identity, partition-independent sequential rate conversion, output bounds and malformed frames. CLI tests require unsupported options and codecs to fall back with complete original stdin, strict-safe exit 4 without output, no fallback after output begins, quiet EPIPE including final-buffer flush, and unchanged existing WAV/raw file behavior.

An oracle disagreement follows the specification and file bytes. Record a provable oracle defect without changing exact admission. Escalate a specification ambiguity rather than choosing undocumented arithmetic. Missing codec or resampler capabilities are separate work; this change registers none by assumption.

## Initial platform checks

The first generated stereo S16 probe at 48 kHz produced exact unchanged-rate mono S16 output, but every F32 mono sample differed. Neither observation authorizes combined mixing and rate conversion.

The existing raw S16 kernel and the pinned macOS arm64 oracle produced equal lengths at 48 to 16 kHz, but 1,396 of 2,732 stereo samples differed by one S16 step. Its arithmetic remains unchanged. No changed-rate stage is registered for that platform. The existing Linux x86-64 evidence does not transfer to macOS. Complete decoded-S16 composition must pass the Linux oracle checks before its registration can be admitted; a failure disables that candidate rather than changing the numerical bar.

## Float-preserving rate adapter

A separate bounded investigation found the existing kernel's unquantized mono F32 adaptation equal to the Linux x86-64 oracle for seven isolated signals at 44.1 or 48 kHz to 16 kHz, including complete lengths and both F32 and S16 destinations. Those were rate-only comparisons over identical oracle-decoded float input, not an exact decoder or complete-command registration. The S16 API differed when the same floats were quantized before filtering.

Include a bounded F32-input adapter in this implementation, sharing the existing kernel, rational positions, reflected edges, accumulation and drain. Preserve float samples until the final destination packing. Register candidate rate rows only for mono 44,100 or 48,000 Hz to 16,000 Hz, on the verified platform, and only after complete lossless-source commands pass. Other rates, channel conversions and platforms remain denied unless this change's independent generated checks establish a separate exact row. This adds no new coefficient recipe or numerical tolerance. These rate checks do not themselves register any decoder.

The streaming engine must handle oversized chunks incrementally, preserve signed-zero and finite F32 sample bits on identity paths, bound startup and tail storage independently of duration, and drain exactly once. Check both F32 and S16 outputs against the oracle without passing native output through another audio conversion. Test short signals, irregular values, impulses, extrema, decoder block boundaries and multiple chunk partitions. F32 mixing is still a separate unregistered stage; mono input does not imply stereo input can use this path.

Standalone WAV input is not covered in this change. It requires a bounded PCM source API in addition to native probing. A follow-up will add a WAV/PCM source adapter over the landed probe parser's data-span API and register the verified paths. Do not duplicate RIFF parsing or treat a report as decoded frames.

## Decoder conformance registrations

The decoder policy now permits two eligibility kinds. `exact` requires identical decoded samples. `conformance` permits the decoder's registered specification-conformance criterion and explicitly documents its sample divergence from the default oracle. This exception belongs only to the decoder stage. It never admits an unregistered downmix, rate conversion, presentation policy or packing operation.

Consume the separately reviewed [decoder eligibility record](../compat/audio-decoder-eligibility.json). Its decoder eligibility and presentation eligibility are independent. Missing entries, unknown kinds and disabled decoder or presentation entries deny admission. The merged record admits only MPEG-1 Layer III in MP4, subject to parsing every frame header and accepting the complete native timeline without warnings or nonzero composition offsets. AAC's decoder eligibility is false. Both Opus presentations and every Matroska lossy presentation are false. Decoder conformance alone cannot authorize Opus pre-skip, end trimming or drain.

FLAC and declared raw S16 paths remain byte-exact within their registrations. The MPEG-1 MP3 path is conformance-only, not byte-identical to FFmpeg. Its generated command checks compare packing with the native decoder's complete presented samples and require the finite edit's exact count. Its rate-stage checks feed those same native decoded floats to the pinned oracle, independently of decoder differences. Neither near-equality of S16 output nor an alternative oracle decoder qualifies as decoder exactness. The [lossy eligibility note](lossy-pcm-eligibility.md) owns decoder evidence and divergence.

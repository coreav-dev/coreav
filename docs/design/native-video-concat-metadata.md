# Native video concat metadata

## Scope

Audit native MP4 concat and `coreav join` for AV1, VP9 and HEVC against the checksum-verified FFmpeg 9.0.2 pin. AV1 currently loses the visual sample entry's `fiel` and compressor name, so the exposed field order and stream encoder tag differ. The existing AV1 concat control removes those source declarations before comparison; this audit must retain them.

One pull request contains this plan as its first commit, the native corrections and their regressions. H.264 configuration, packet transformation, sample-entry labels and its compatibility refusal are separate work and must not change here. No decoder admission, tolerance, benchmark threshold, frozen representative, generator, judging record or public count changes. Only independently generated development inputs are executed.

## Native execution is a gate

Capture the unchanged baseline binary, source revision, input identities, complete arguments and native outcomes before implementation. Every baseline-native command must remain native afterward in both normal and strict modes, with an unusable fallback executable and no fallback log entries. A disagreement must be fixed natively, not turned into a new refusal. If a native correction cannot preserve the existing contract, stop before pushing and report the affected command count, complete difference and reason. There is no new known-divergence or comparison exception in this plan.

Retain the existing bounds and terminal treatment of malformed input, source changes, I/O, resource limits and execution failures. Do not relax sample-entry matching, decoder configuration matching or clean-boundary requirements to manufacture a native result.

## Development matrix

Generate short, bounded MP4 inputs separately with the pin's AV1, VP9 and HEVC encoders, one encoding thread and fixed dimensions, pixel format and frame rate. Keep natural progressive field declarations and nonempty encoder metadata. Use two-input repetition and three-input sequences with the same admitted configuration. Include video-only sources and sources with copied AAC; retain the latter's original priming and packet clocks. Add distinct named controls for absent field-order metadata and valid alternate stored field declarations rather than replacing natural metadata-bearing cases.

For each codec, exercise ordinary inferred-MP4 concat, explicit video selection with audio disabled, and faststart output. Exercise native `coreav join` with its existing audio selection and video-only form on the same inputs. Establish actual baseline-native cases before assigning any passing credit. Do not silently omit a codec, an audio-bearing case, a source declaration or a command that exposes a difference.

Native join and compatibility concat have distinct placement contracts. Preserve native join's per-input presentation for the specification's reason: ISO/IEC 14496-12 sections 8.6.5 and 8.6.6 define the edit list and its media-time mapping, so each input's AAC priming before its media edit is not presented. Do not change native join's timeline in this metadata correction. Cite section 8.6.6 at its timing owner and in the pull request.

Report the three audio-bearing native join comparisons separately under Not covered: the declared native presentation is 48,000 audio ticks, while ordinary pinned concat presents 49,024; default probing exposes 75 versus 50 AAC packets, although ignoring edit lists exposes 50 stored AAC packets in both. The extra 1,024 presentation ticks in the reference present the second input's priming. Keep the complete records, do not register a known divergence, and do not change a predicate or any counted/frozen result. These three direct native join calls are not top-200 ffmpeg/ffprobe commands. Stop before further work if a counted command is found to have a changed result.

## Comparison

Compare the entire `-show_streams -show_data -show_data_hash sha256` record, including tags, dispositions, extradata, field order, sample-entry identifiers and every field not anticipated by the test. Compare complete globally ordered packet records with their complete payload bytes, hash, size, flags, side data and clocks. Packet file position alone may be removed because native container layout is different; no stream field is removed or replaced. Check complete presentation extent and output sample count as well as the first packet.

Reuse the shared concat comparison owner for the regression matrix rather than creating another comparator. Mutation coverage must reject a changed field order, encoder tag, codec tag, extradata byte, unknown stream field, payload byte/hash/size, flag, clock, packet order or missing terminal packet. A changed file-position value alone is the explicit layout control. Existing registered oracle conditions remain visible and do not authorize editing source metadata to avoid a discrepancy. Retain full format captures as diagnostic evidence without claiming complete container-file byte identity; differing native layouts do not justify removing any stream or non-position packet field.

## Implementation boundary

Integrate the shared native join metadata changes before implementing this correction. Their bounded first-input scan and preservation of the selected source's compressor name and media/audio configuration fields must remain the single owner, rather than adding another scan or encoder-tag source. Preserve field-order bytes through that same selected-track metadata path. Do not add a second generic metadata parser, copy arbitrary unbounded child payloads, or expose a new public model field without a current consumer that needs it.

Prefer one visual-entry builder that receives the existing compressor-name value and the optional two-byte `fiel` payload, rather than patching offsets and box sizes in several owners. Clip and split keep their current default entry construction. Field-order preservation is enabled only for AV1, VP9 and HEVC; H.264 sample-entry output remains unchanged by this delta. Keep the first input's declarations without adding a new cross-input mismatch refusal, and verify first-present/absent and differing-declaration controls against the pin. A field-order child beyond a summary-list display cap must not silently disappear: cover that boundary explicitly through the bounded scan owner.

Keep fixed sample-entry syntax grounded in ISO/IEC 14496-12 section 12.1.3 and the existing AV1, VP and HEVC container bindings. Use public specifications and black-box pinned outputs only, never implementation sources or their history. Before implementing, obtain the scoped architecture review and reconcile the shared join interface with its owner.

The `fiel` payload is the two unsigned-byte field count and detail declaration defined by [Apple Technical Note TN2162, Field/Frame Information](https://developer.apple.com/library/archive/technotes/tn2162/_index.html). Retain those stored bytes rather than inferring a new declaration from coded packets; the absent declaration remains absent.

## Verification

First remove the AV1 control's source metadata suppression and show the complete-record regression fails on the unchanged baseline. Then verify every corrected baseline-native case remains native, with no new fallback, and that the full records and packet payloads agree. Retain existing split/join sample, timing, mismatch, limit and output-publication regressions. Exercise malformed or oversized metadata and late source/I/O failures through the existing error owners.

Run the affected complete CLI and writer targets with the required pin, comparator mutation tests, formatting and all-target Clippy. Run the canonical required-oracle workspace suite with its complete generated fixture set. Before the batched publication, validate public text, signed commit shape and unchanged frozen identities. Obtain exact-head similarity evidence, CI and independent final-head review before handoff. Refs coreav-dev/coreav#25.

## Development outcome

The unchanged baseline captured 63 generated command executions: 51 native and twelve existing positive-AAC-edit compatibility refusals. After integrating the shared metadata owner and retaining field declarations, all 51 stay native, 48 complete stream/packet comparisons match, and only the three audio-bearing native joins differ as described above. There are zero native-to-fallback moves. These development results do not change any frozen or public count.

The non-frozen `ffmpeg_concat` regression covers 63 successful joins across the three codecs, including selected second-video-track metadata, nine terminal fixed-prefix read failures, 36 rejected stream/packet mutations and three position-only record controls. The scan regressions retain field metadata beyond the display cap while preserving missing/short payload behavior and box budgets. The existing natural AV1 regression no longer suppresses source metadata and passes after wiring.

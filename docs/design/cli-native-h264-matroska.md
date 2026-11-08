# Native H.264 Matroska cadence

## Scope

Extend the opt-in CLI selector to Matroska, without changing native MP4 admission, encoder algorithms, default or release features, or commands that do not select the native encoder. WebM remains excluded. Every command admitted natively before this change must remain native with identical complete output bytes.

## Clock representation

Matroska DefaultDuration is a positive integer number of nanoseconds, independent of TimestampScale (RFC 9559 Section 5.1.4.1.13). Block timestamps use TimestampScale. The current writer rounds each absolute presentation timestamp to the nearest millisecond and uses TimestampScale 1,000,000. A constant 30 fps source consequently has alternating 33 and 34 ms packet intervals. Inferring cadence from identical rounded intervals loses its nominal frame duration.

For native H.264 Matroska replacements only, derive DefaultDuration from the encoder's admitted exact rational frame rate: round half up `1,000,000,000 * fps_den / fps_num` with checked integer arithmetic. Keep the current millisecond packet timestamps and TimestampScale. Do not infer nominal cadence from their rounded differences. Do not write a per-packet BlockDuration that supersedes the nominal default.

The exact rational remains in the H.264 VUI. DefaultDuration differs from its exact period by at most half a nanosecond, and each absolute block PTS retains the existing millisecond quantization rather than accumulating rounding drift. Common rates must read back exactly through the pinned oracle; unusual rates are checked against the independently parsed integer duration and timestamp quantization, not an assumed ffprobe fraction.

For nominal 29/7 fps, the pinned oracle reports `r_frame_rate=58/7` and `avg_frame_rate=29/7`. This one field-clock readback is recorded explicitly; the independent SPS check still requires `time_scale / (2 * num_units_in_tick) = 29/7` (AVC E.2.1), and DefaultDuration still requires the rounded nominal frame period (RFC 9559 Section 5.1.4.1.13). No other guessed rate is accepted by this generated comparison.

New native Matroska admission requires a nominal period of at least one millisecond and strictly increasing output PTS after rounding. Higher rates or collisions retain their previous external path. No native Matroska command exists before this change, so these boundaries do not move an existing native command. Keep the merged native timing predicate, including its legacy single-picture and terminal-zero defaults, unchanged for MP4.

## Boundary

Add a checked video-cadence value and an explicit writer entrypoint for one H.264 video replacement. Existing writer calls retain their current behavior. The new entrypoint validates that the named stream is the actual video replacement, supplies its nominal default duration, and rejects rounded timestamp collisions before publication. Audio payloads, stream selection, display dimensions and cluster policy remain unchanged.

For copied Opus from ISO base media, this new entrypoint follows the original source edit list, not the informative dOps PreSkip field (Opus ISO mapping Sections 4.3.2 and 4.4). It carries that edit's trim into CodecDelay and OpusHead and retains its declared terminal presentation. The ordinary writer now follows the edit the same way ([Matroska writer](mkv-write.md)), so media time zero with PreSkip of 312 and a declared 7,992-sample presentation retains all 7,992 samples in both. The two differ only for a track without an edit list, where the ordinary writer still trims PreSkip and this entrypoint trims nothing. Native MP4 remains byte-identical to its actual base.

The transcode pipeline uses that entrypoint only for a completed native H.264 Matroska encode, with the admitted encoder frame rate. MP4 never constructs the cadence override. Unsupported native cadence retries the unchanged external command under the original deadline; I/O, malformed-input and timeout errors remain terminal.

## Verification

- Build and retain a separate native binary from the actual pre-change main revision. Compare complete MP4 bytes for the existing metadata, quality, thread, GOP, legacy duration, input, audio-copy and selector axes, with external programs unavailable. Record named cases and counts, not just candidate self-comparisons.
- Preserve complete default, feature-enabled unselected and external Matroska output bytes against that revision. The one exception is the edited Opus source copied beside an external H.264 encode: its default and unselected Matroska outputs must instead decode to the source edit's declared presentation, because the ordinary writer now follows that edit too. Retain unsupported-option, WebM, VFR, high-rate and timestamp-collision fallback controls and transactional deadline tests.
- Check new Matroska output with an independent EBML parser: DefaultDuration, TimestampScale, block PTS, absence of conflicting BlockDuration, display ratio, codec private data and keyframe flags. Independently parse VUI timing and aspect.
- Cover integer, NTSC and unusual rational rates, absent and explicit aspects, single pictures, legacy zero durations, source origins, the 250-picture GOP boundary, thread counts and copied audio.
- Compare complete encoded access units and pinned-oracle reconstruction between same-head MP4 and Matroska. Verify frame counts and timestamp error against the declared quantization.
- Run affected Rust and Python tests in Linux CI by name, unchanged feature-graph checks, public-text checks and exact-head similarity before readiness.

## References

- https://www.rfc-editor.org/rfc/rfc9559.html#section-5.1.4.1.13
- https://www.matroska.org/technical/notes.html
- https://www.matroska.org/technical/elements.html
- https://www.opus-codec.org/docs/opus_in_isobmff.html

Refs coreav-dev/coreav#25.

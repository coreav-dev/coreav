# Copied AAC and Opus bitrate metadata

`coreav ffmpeg` can encode video while copying audio after an earlier AAC selection has been overridden by `copy`. Its existing admission rules stay unchanged. On this path a representable `-b:a` value also controls the copied AAC or Opus track's MP4 bitrate metadata; it does not encode or alter audio packets.

The writer derives the average from complete copied sample tables as `data_bytes * 8 * timescale / duration`, rounded down. The maximum is the larger of that average and the requested bitrate. The decoder buffer size is zero. These are the pinned FFmpeg's observed values for generated AAC and Opus controls, including requests below and above the average, stale source metadata and missing `btrt` boxes.

## Ownership

The ffmpeg adapter parses the existing unsigned integer and decimal SI-suffix grammar without introducing a new error. Missing, malformed and out-of-range requests use the absent-request policy. Native `coreav transcode` and the ordinary remux entry points default to no override. The request passes the shared policy to the transcode options and then only to the MP4-source remux options. Matroska-source copies keep their existing output byte for byte. The [MP3 extension](copied-mp3-bitrate.md) distinguishes the source-clock hint from an explicit zero request.

After all samples of a copied AAC or Opus track have been listed, a single `Muxer` operation updates its sample entry from the completed tables. Encoded replacements, video and copied MP3 never enter this operation. The `esds` parser remains owned by `coreav-isobmff`; the writer obtains validated rate-field offsets from that owner rather than adding another descriptor parser. Existing `btrt` fields are updated in place, or one bounded `btrt` is appended when absent. Added bytes stay within the writer's existing box and retained-metadata allowances. An incomplete or malformed entry, insufficient allowance or an unrepresentable average leaves the complete entry unchanged rather than adding a refusal.

No packet, sample table, edit, priming field, tag or top-level layout policy changes. Matroska output ignores the MP4 metadata option. The complete legacy native-transcode comparison remains unchanged.

## Verification

Generated pinned-oracle references cover MP4-sourced AAC and Opus, low and high rates, multiple copied tracks, stale fields, missing `btrt`, and overridden codec selections. Tests compare each complete output file with a native control that has only the independently observed oracle metadata applied, including dependent box sizes and chunk offsets. This preserves packet bytes, timing, tags and layout rather than hiding their differences through normalization. Raw CoreAV and FFmpeg files still differ in the deliberately unchanged tags and layout. Default, invalid, out-of-range and absent-audio controls preserve native output and command admission. Copied MP3 and Matroska-sourced audio output remain byte-identical.

Matroska copy-clock parity is a known gap, not a passing oracle case. A generated AAC witness has 13,312 stored ticks in CoreAV and 13,000 in FFmpeg, yielding different averages for the same packets. Opus also differs. Replacing only the metadata would not reconcile those clocks, so both the Matroska tables and its existing metadata remain unchanged in this change. Exact Matroska copy-clock parity is a separate follow-up.

Copied MP3 uses the separately verified [complete-frame clock](copied-mp3-bitrate.md), not the AAC/Opus table formula. Unproved frame clocks keep their metadata without changing command admission.

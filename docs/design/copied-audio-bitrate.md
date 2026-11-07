# Copied AAC and Opus bitrate metadata

`coreav ffmpeg` can encode video while copying audio after an earlier AAC selection has been overridden by `copy`. Its existing admission rules stay unchanged. On this path a representable `-b:a` value also controls the copied AAC or Opus track's MP4 bitrate metadata; it does not encode or alter audio packets.

The writer derives the average from complete copied sample tables as `data_bytes * 8 * timescale / duration`, rounded down. The maximum is the larger of that average and the requested bitrate. The decoder buffer size is zero. These are the pinned FFmpeg's observed values for generated AAC and Opus controls, including requests below and above the average, stale source metadata and missing `btrt` boxes.

## Ownership

The ffmpeg adapter parses the existing unsigned integer and decimal SI-suffix grammar without introducing a new error. Text that does not parse or fit the 32-bit bitrate fields retains its previous ignored behavior. Native `coreav transcode` and the ordinary remux entry points default to no override. The request passes the optional rate to the shared transcode options and then to the MP4-source or Matroska-source remux options.

After all samples of a copied AAC or Opus track have been listed, a single `Muxer` operation updates its sample entry from the completed tables. Encoded replacements, video and copied MP3 never enter this operation. The `esds` parser remains owned by `coreav-isobmff`; the writer obtains validated rate-field offsets from that owner rather than adding another descriptor parser. Existing `btrt` fields are updated in place, or one bounded `btrt` is appended when absent. Added bytes stay within the writer's existing box and retained-metadata allowances. An incomplete or malformed entry, insufficient allowance or an unrepresentable average leaves the complete entry unchanged rather than adding a refusal.

No packet, sample table, edit, priming field, tag or top-level layout policy changes. Matroska output ignores the MP4 metadata option. The complete legacy native-transcode comparison remains unchanged.

## Verification

Generated pinned-oracle references cover AAC and Opus, low and high rates, MP4 and Matroska input, multiple copied tracks, stale fields, missing `btrt`, and overridden codec selections. Tests compare complete output files after the existing ordinary remux gives oracle and candidate media the same container representation, or compare a complete native control with only independently observed oracle metadata differences applied. Direct metadata and packet checks prevent unrelated differences from being normalized away. Default, invalid, out-of-range and absent-audio controls preserve native output and command admission. Copied MP3 output remains byte-identical.

The copied MP3 bitrate clock is a separate follow-up. In a generated control, its stored durations imply 67,511 bits/s, while the pinned oracle's complete frame clock implies 64,000. Applying the AAC/Opus formula to it would replace one incorrect value with another; this change does not guess frame timing or add packet-header admission.

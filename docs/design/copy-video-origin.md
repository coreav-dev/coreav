# Normalize the origin of an unedited copied video

## Problem and contract

Ordinary `coreav ffmpeg -i INPUT -c copy OUTPUT.mp4` currently reuses the native full-file remux contract, which preserves presentation time. Pinned FFmpeg 9.0.2 instead subtracts the input format start. On an independently generated unedited H.264 source, both tools copy all 78 frames, but CoreAV starts at 1024/12288 seconds and FFmpeg starts at zero. Faststart and explicit copy selection have the same difference. This is a compatibility policy difference, not permission to change the native `coreav remux` timeline.

This change supports a single selected unfragmented ISO video with no source edits, whose first composition time equals the positive input origin rounded to its media ticks. It preserves every sample, composition offset, sample entry and supported metadata. Other nonzero-origin ordinary remux shapes refuse before output and retain the existing external fallback. Seeking, audio extraction, concat, multiple inputs, encoding, native remux defaults and probe output are not changed.

## Ownership and interface

The compatibility planner passes the existing raw report's whole-microsecond input start as `RemuxOptions::video_origin: Option<Rational>`. The default is `None`, preserving the native API's timeline. It does not use the output-only ffprobe timing overlay. Nonzero origins with chapters, timecode, pictures, several selected tracks, Matroska or negative starts are refused by the compatibility planner.

The full-file writer owns the remaining byte-derived eligibility checks. Before writing, it requires one unfragmented video track, no replacement, no edits and a positive finite representable origin. Its existing bounded table pass also records the minimum composition time. That minimum must equal the rounded origin, and the first decode time must be zero. A mismatch refuses rather than clipping or dropping preroll. These checks cannot be inferred from the compatibility report alone.

The writer uses ISO/IEC 14496-12 8.6.1.3 composition times and 8.6.6 edit mapping: one unit-rate media edit begins at the first composition time and lasts through the maximum composition end. The track's presentation duration is recomputed from that edit; the media duration and sample tables remain unchanged. The movie timescale becomes the least common multiple of the original movie and video media timescales, refusing if that common clock cannot fit 32 bits. Thus the edit length is exact rather than rounded. No additional payload pass or unbounded storage is introduced.

Alternatives: reusing the seek/clip writer did not produce exact output on this source; changing native remux defaults would break its preservation contract. Refusing every positive origin would be safe but would leave the directly reproducible single-video case without native support. The explicit, checked option separates those contracts without making a general timestamp-shifting API.

## Verification before delivery

- Add synthetic writer regressions for sample and metadata preservation, exact edit duration, unchanged default output, invalid origins, edited or multiple tracks, fragments, and unchanged write/read/work-limit failures.
- Add compatibility-plan regressions and real command comparisons against pinned FFmpeg 9.0.2 using generated non-judged inputs: plain copy, faststart, explicit and mapped video copy, strict-safe refusal without replacing a destination, and exact ordinary fallback for unsupported origins.
- Compare complete decoded video and frame timing with the existing unchanged exact comparator. Record source and binary identities. Prove native remux, zero-origin copy, seek and probe behavior remain unchanged.
- Run the affected writer and CLI targets, relevant feature-enabled library checks, formatting, Clippy, pin verification and public-text checks. No performance or coverage measurement is claimed.

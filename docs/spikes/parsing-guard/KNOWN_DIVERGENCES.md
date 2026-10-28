# Spike-local known divergences

These entries have machine-readable twins in `known-divergences.json`. All file
patterns are `*`; predicates use container/codec properties, never fixture names.
`compare.py` preserves the harness position-paired unit denominator and the
original normalized captures. An item with another uncovered gating field still
fails. Informational duration/discard differences do not affect rates.

## C-PCM

Fields: grouping, missing/extra boundaries, size and associated timestamps.
One packet per container sample is required. ffprobe
may group consecutive constant-size PCM samples. The predicate requires a PCM
codec, byte-aligned bit depth, fixed size equal to channels times bytes per
sample, exact consecutive tiling of **every** oracle byte range by original
samples, positive durations whose sum equals the oracle duration, timestamps
anchored at the oracle packet plus preceding durations, and identical keyframe
flags. No gaps, leftovers, mixed fields or partial mappings are covered.
Grouping coverage is reported for the original samples, without merging them.
`packet_runs` delivers constant-size samples in runs for reading only; each run
expands to one packet per sample, so this entry and its counts are unchanged.

## C-CTTS

Field: DTS only. Signed negative composition offsets are preserved without
shifting decode timestamps. A version-1 `ctts`
with a negative minimum may make ffprobe shift DTS by that minimum. Coverage
requires exactly that shift and direct agreement of position, size, PTS and
keyframe. No other timestamp difference is covered.

## C-STRICT

Fields: probe status, stream count, missing packets and unavailable format
duration following rejection. Strict rejection of invalid/truncated input and
limit violations is permitted. Coverage requires
an identified ISO BMFF container and the parser's bounded structural error or
nesting-limit diagnostic. It does not cover unsupported containers, codec
features, arbitrary parser errors, or a successful parse with field differences.
A zero-packet rejected file is recorded separately as a covered file status;
it does not manufacture a packet denominator.

## C-FMT-ORIGIN

Field: `format_duration_us` only. CoreAV reports the container-declared
presentation duration, the movie header (`mvhd`) duration, which counts a
leading empty edit as part of the presentation. ffprobe 7.0.2 reports the span
from the earliest stream start to the latest stream end, so a leading empty
edit shared by all streams is subtracted. Example: `edit_list.mp4` declares
2,500 ms with empty edits of 500 ms (video) and 476 ms (audio); CoreAV reports
2,500,000 µs, ffprobe 2,023,991 µs (2.5 s minus 0.476009 s).

Basis: an earlier pull request, which settled on reporting the declared presentation
duration, and ISO/IEC 14496-12 semantics for the movie header
duration (clause 8.2.2.3) and empty edits (clause 8.6.6). The clause numbers
come from the black-box research notes; the wording was not checked against the
ISO text, which was not available to this spike (only through the ISO store
checkout), so no wording is quoted here.

Coverage requires all of:

- a successful parse and a candidate value equal to the movie header duration;
- on **every** stream, exactly one leading empty edit with a positive duration
  followed by one media edit, both at rate 1;
- a movie header duration equal to the longest stream's edit timeline (empty
  plus media edit), so the declared value is internally consistent;
- identical stream durations (`duration_ts`) in both captures;
- an oracle value equal, to the microsecond, to the latest stream end minus the
  earliest stream start, where each start is the empty edit rounded to the
  nearest stream tick, and the latest stream end equal to the declared duration.

So the only difference allowed is the subtracted origin. A file without an
empty edit on some stream, a stream-duration difference (for example
`vfr.mp4`), or any other mismatching stream field stays uncovered.

## C-STTS-SHORT

Fields: `duration_ts` and, where it follows, `format_duration_us`. Basis:
the accepted rule for stream duration conflicts
(an earlier pull request). CoreAV reports the declared
presentation duration: the media edit converted to media ticks, capped by the
media header (`mdhd`) duration, or `mdhd` without an edit list. When `mdhd`
differs from the sum of the `stts` decode durations, CoreAV keeps `mdhd`, adds a
`duration conflict` warning and exposes `mdhd_duration_ticks`,
`stts_duration_ticks` and `media_edit_duration_ticks` in the stream properties.
ffprobe 7.0.2 reports the smaller `stts` sum. Example: `vfr.mp4` declares
25,088 ticks at 12,800 Hz (1.96 s, matching `mdhd`, `tkhd`, `mvhd` and the
edit); its `stts` deltas add up to 23,040 ticks (1.8 s), which ffprobe reports,
together with a 1,800,000 µs format duration, although its own packet list
presents the last frame at 1.92 s. Packet durations stay the `stts` deltas, so
their sum can differ from the stream duration.

A stream's `duration_ts` is covered only when all of:

- `mdhd` is not 0 and the candidate carries the `duration conflict` warning;
- the oracle value equals `stts_duration_ticks` exactly, and that sum is
  smaller than CoreAV's value;
- CoreAV's value equals the stream duration rule exactly: `mdhd`, or the media edit
  duration when it is smaller, rounded to the nearest tick.

`format_duration_us` is covered only when all of:

- at least one stream's `duration_ts` is covered by this entry, and every
  other stream's `duration_ts` agrees directly;
- the candidate value equals the movie header duration;
- every stream has no edit list or the supported edit shape, and the oracle
  value equals, to the microsecond, the latest oracle stream end minus the
  earliest stream start, computed from the oracle's own stream durations
  (start = leading empty edit rounded to the nearest stream tick, else 0).

A longer oracle value, an oracle value that is not the `stts` sum, a fragmented
track with `mdhd` 0, or any other differing field stays uncovered.

## C-EDIT-DISCARD

Field: `extra` only, one packet at a time. Basis: one packet is
exactly one sample, never merged or split, and the supported edit list is applied
exactly; ISO/IEC 14496-12 edit list semantics (clause 8.6.6 per the research
notes; the ISO text was not available, so no wording is quoted) and an earlier pull request. CoreAV emits every sample, including
one that the edit list does not present, and marks it `presented: false` and
`discard: true`. ffprobe 7.0.2 omits some such samples (black-box: trailing
samples of tracks without non-sync samples), so they have no oracle packet.
Example: the final `mov_text` sample of `multitrack.mp4` starts at media time
2,000,000 µs with duration 0, exactly at the end of the only media edit
(2,000 ms, `media_time` 0).

Coverage requires all of:

- the candidate packet has no oracle packet at its byte position (the only
  differing field is `extra`), and the stream count and probe status agree;
- the stream's edit list has the supported shape (an optional leading empty
  edit and one media edit at rate 1);
- the candidate packet is marked `presented: false` and `discard: true`;
- independently recomputed from `media_pts`, `duration` and the edit list, its
  media composition interval `[media_pts, media_pts + duration)` does not
  overlap `[media_time, media_time + segment_duration)` (a zero-duration
  sample: `media_pts` lies outside that interval).

A presented sample, a missing packet, a sample in a track without an edit list
or with an unsupported one, or any differing timestamp, size or keyframe stays
uncovered. Paired packets outside the edit (kept by ffprobe) are compared
directly as before.

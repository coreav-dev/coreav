# Known divergences: `coreav-isobmff` versus `ffprobe`

Every intentional difference from the `ffprobe` oracle is documented, with a
machine-readable twin. The twin is [`known-divergences.json`](known-divergences.json);
`scripts/oracle_kd.py` applies it to the oracle harness report, and
`tools/oracle/tests/test_kd.py` keeps this page, the twin and the predicates in
sync. Each entry covers only the fields it names; an item counts as agreement
only if every gating difference is covered. Covered items stay in the
denominator and are reported separately from direct agreement.

These entries restate, for the hardened crate, the divergences that Spike C
documented and that the oracle policy accepts as known divergences. Predicates use container
and codec properties, never fixture names. The oracle is `ffprobe` 9.0.2.

## Evidence the overlay checks

A predicate that takes a raw value from CoreAV (a sample's position, size
or media times, a media header duration, an edit list) would excuse a
CoreAV bug that moves the value and that evidence together. So the overlay
reads those values from the file itself, with a reader written apart from
CoreAV's parser (`Facts` in `scripts/oracle_kd.py`, ISO/IEC 14496-12 and
the QuickTime File Format), and an entry applies only where the two agree:

- **Stream values** (`Facts.stream_ok`): the movie header's timescale and
  duration and the brand; each stream's timescale, media header duration,
  stored edit entries, the list's shape as the edit model sorts it (none,
  simple, several, or outside the model), the simple list CoreAV derives
  from them, the media edits' duration in media ticks (reported exactly
  when there is a media edit of known duration) and the leading empty
  edits' presentation offset, `ctts` version and signedness, `stts` sum,
  first `tfdt` and segment index end. A kept stream must report each of
  these, and no two streams may claim one track.
  `KD-STTS-SHORT`, `KD-EDIT-DURATION`, `KD-FRAG-START`, `KD-PCM-END` and
  `KD-TEXT-END` need them for their stream; the format clause of the first
  four needs them for every stream, dropped streams included.
- **Samples** (`Facts.samples_ok`): every packet CoreAV lists for the stream
  is the file's sample at that place in decode order (position, size, media
  decode and composition times, duration and, but for H.264 whose flag
  comes from the coded data, the sync flag), no sample is missing and none
  is added. Runs of PCM samples are compared run by run, in the per-sample
  and the reference packet unit alike.
  `KD-PCM`, `KD-EDIT-DISCARD`, `KD-CTTS`, `KD-CTTS-UNSIGNED`,
  `KD-TRUN-SHIFT`, `KD-EDIT-SNAP`, `KD-EDIT-MULTI` and the packets of
  `KD-CHAPTER-TEXT` need them.
- **Edit lists outside the model** (`Facts.outside_model`): the stored list
  has a rate other than 1, a media time below −1, media edits whose windows
  do not increase without overlap, a media edit of duration 0 other than
  the simple model's open-ended edit in a movie with fragments, or several
  media edits in a movie with fragments (`docs/design/edit-lists.md`,
  decision 1). `KD-EDIT-UNPLACED` needs it for each unplaced stream.
- **AAC configuration** (`Facts.aac_plain_lc`): the file's
  AudioSpecificConfig is two bytes, object type 2, at the sampling rate
  CoreAV reports, with no extension. `KD-AAC-IMPLICIT-SBR` and
  `KD-AAC-LC-UNNAMED` need it.

The entries whose shift applies to a whole stream (`KD-CTTS`,
`KD-CTTS-UNSIGNED`, `KD-TRUN-SHIFT`, `KD-EDIT-SNAP`) also need it to show on
every paired packet of the stream: none may agree with the reference on the
shifted field. `KD-STRICT`, `KD-DECLARED-SIZE`, `KD-HEVC-WINDOW`, `KD-QT-TEXT`,
`KD-NOT-ALLOWED` and `KD-COVER-ART` read their evidence from the file or
from the reference already. The audit that motivated these checks, and the
mutations it tries, is in `docs/measurements/kd-audit.md`.

## KD-STRICT

- Fields: `probe_ok`, and every packet of the rejected file.
- When: CoreAV rejects a file as truncated, structurally invalid or over a
  limit, `ffprobe` reads it, and the file's bytes independently show that kind
  of defect. `scripts/oracle_kd.py` walks the box headers itself (ISO/IEC
  14496-12, section 4.2) and requires: for a truncation, a top-level box that
  runs past the end of the file; for an invalid structure, a box that runs
  past its parent, a container inside the wrong parent (for example a track
  box outside the movie box), a second top-level movie box (ISO/IEC 14496-12
  8.2.1: exactly one), or a top-level movie box without a movie header among
  its children (8.2.2: exactly one, mandatory), except one holding a
  QuickTime compressed (`cmov`) or reference (`rmra`) movie, which the
  QuickTime File Format defines without one (CoreAV rejecting those is a
  missing feature, not covered); for a limit, nesting deeper than the default
  depth limit. A valid file that CoreAV
  rejects is therefore never covered.
- What each reports: `ffprobe` reports streams (for example `truncated.mp4`,
  whose last box runs past the end of the file, and `nested_10000.mp4`, a track
  box nested 9,999 levels deep). CoreAV fails with a structured error.
- Why: structurally invalid or truncated files are rejected, and nesting
  is bounded by a resource limit.
- Not covered: unsupported containers (for example WebM), disallowed codecs,
  input/output errors, or any successful parse with differing fields.

## KD-FMT-ORIGIN (retired)

Retired. The oracle adapter now reports CoreAV's derived
presentation duration (probe's `presentation_duration`: the latest stream end
minus the earliest stream start) as `format_duration_us`, which is how
`ffprobe` defines its format duration; the declared movie header value stays
probe's `duration`. The case this entry covered (every stream starting with an
empty edit, so that the declared duration counts the empty time) now agrees
directly.

## KD-STTS-SHORT

- Fields: `duration_ts` and, where it follows, `format_duration_us`.
- When: the media header duration differs from the sum of the decode
  durations (`stts`), and CoreAV warns about the conflict.
- What each reports: CoreAV reports the declared duration: the media edit,
  capped by the media header. `ffprobe` reports the smaller `stts`
  sum. Example: `vfr.mp4` declares 25,088 ticks (1.96 s); its `stts` sum is
  23,040 ticks (1.8 s).
- Why: a stream's duration is the declared one; a conflict with the `stts`
  sum is reported as a warning, not resolved by the sum.
- Coverage requires: the candidate carries the duration-conflict warning; the
  reference value equals the `stts` sum exactly and is smaller than the
  candidate value; the candidate value is exactly the declared duration
  rule. The format duration is covered only if every other stream agrees or is
  covered by `KD-STTS-SHORT`, `KD-EDIT-DURATION`, `KD-FRAG-START` or
  `KD-PCM-END`, and each side's format duration is the span of its own
  streams, computed from its own stream durations and kinds as `ffprobe`
  counts a format duration (`docs/design/probe-duration.md`, "The
  oracle"): the audio and video streams, and each other stream that ends
  less than a second after them. Starts are the empty edits rounded to
  stream ticks, on both sides; starts and durations are rounded to
  microseconds each. A stream the allowlist dropped counts with the
  duration the adapter reports for it (`duration_ticks`); one whose
  durations differ may stay only when it decides neither side's span.

## KD-EDIT-DURATION

- Fields: `duration_ts` and, where it follows, `format_duration_us`.
- When: a stream with a simple edit list whose media edit is shorter than
  the media, or that starts after a leading empty edit.
- What each reports: CoreAV reports the declared duration, the media edit
  capped by the media header: what the edit list presents.
  `ffprobe` reports the smallest of the media header duration, the `stts`
  sum and all the edits' durations added up, the leading empty edit
  included (its black-box rule). Examples: an AAC track whose edit skips 2,112
  priming samples (CoreAV the 836,160 presented samples, `ffprobe` the
  838,656 stored); a track after a 506-tick empty edit (`ffprobe` adds the
  empty time to the duration).
- Several edits (`docs/design/edit-lists.md`): CoreAV reports the media
  edits' durations added up, capped by the media header (decision 5);
  `ffprobe` the smallest of the media header duration, the `stts` sum and
  every entry's duration rounded to whole ticks and added up, empty edits
  included. They differ when the list has empty edits, or when the media
  edits' sum rounds otherwise. Example: an empty edit of 600 ticks between
  two media edits.
- Why: a stream's duration is what its edit list presents, capped by the
  media header.
- Coverage requires: the edit list makes the difference (the media edit
  is shorter than the media header duration, or a leading empty edit
  precedes it) and the `stts` sum is not shorter than CoreAV's value (that
  case is `KD-STTS-SHORT`'s); the candidate value is exactly the declared
  duration rule; the reference value is `ffprobe`'s rule exactly and differs from
  it; for several edits, with the entries the adapter reports as stored.
  The format duration is covered as for `KD-STTS-SHORT`: every other
  stream agrees or is covered, and each side's format duration is its own
  streams' span.

## KD-FRAG-START

- Fields: `duration_ts` and, where it follows, `format_duration_us`.
- When: a fragmented stream whose first fragment starts at a decode time
  after 0 (its first `tfdt`), as in a CMAF segment or a stream cut from a
  longer one; or a fragmented stream with a segment index (`sidx`) whose
  end lies after the stream's samples' span, even when its first fragment
  starts at 0.
- What each reports: CoreAV reports the stream's samples' span, the sum of
  the fragment sample durations. `ffprobe`
  counts from decode time 0, so its duration is the first `tfdt` more.
  Example: a 600 s HEVC stream whose first fragment starts at 10 s, which
  `ffprobe` reports as 610 s. A stream the allowlist drops has no
  stream entry to cover; its file's format duration stays uncovered.
- Why: the stream's duration is what its samples span; the
  decode time of the first sample is where the stream starts, not part of
  its length.
- Coverage requires: the adapter's `fragment_start_ticks` (the first
  `tfdt` CoreAV read, reported only for a stream whose samples are all in
  fragments) is above 0 (0 or above with a segment index), and the reference `duration_ts` equals the
  candidate's plus it exactly. With a segment index (`sidx`) for the
  stream, `ffprobe` takes the index's end instead (its earliest
  presentation time plus its subsegment durations, `sidx_end_ticks`, from
  the first index for the track in its timescale): the reference must
  equal that end, the candidate's value plus the first `tfdt` must not
  exceed it, and a kept stream's value must equal its samples' summed
  durations (`stts_duration_ticks`). A stream the allowlist dropped counts
  with the timing the adapter reports for it (`duration_ticks`, its
  fragment start). The format duration is covered as for `KD-STTS-SHORT`,
  except that a stream whose index ends after its samples' span plus its
  first `tfdt` starts that much later in the reference's span: `ffprobe`
  9.0.2 reports `fragmented_sidx.mp4` as 20.16 s, CoreAV as 20.08 s.

## KD-EDIT-UNPLACED

- Field: `format_duration_us`.
- When: a stream, kept or dropped by the allowlist, has an edit list
  outside CoreAV's model (`unsupported`: rates other than 1, dwells, media
  ranges presented twice or backwards).
- What each reports: CoreAV does not place such a stream on the
  presentation timeline, so the derived duration spans the other streams.
  `ffprobe` applies the edit list its own way and counts the stream.
  Example: a dropped SVQ3 track whose four edits present one media range
  twice, ending 5.7 ms after the AAC track.
- Why: an edit list outside the model is reported unsupported, with a
  warning, and never approximated.
- Coverage requires: every unplaced stream's edit list is `unsupported` on
  CoreAV's side; every other stream agrees or is covered as in the format
  clause of `KD-STTS-SHORT`; CoreAV's format duration is the span of the
  streams it places; the reference's is that span with each unplaced stream
  added from time 0 for its reference duration, and differs from it.

## KD-EDIT-DISCARD

- Field: `extra` packets only, one at a time.
- When: a sample lies outside the media edit and the track has only sync
  samples.
- What each reports: CoreAV emits every sample (one packet per sample) and
  marks this one not presented (`discard`). `ffprobe` omits it. Example: the
  last `mov_text` sample of `multitrack.mp4` starts exactly at the end of the
  edit.
- Why: CoreAV emits one packet per sample and applies the simple edit list
  exactly; first registered in the parsing-guard spike, as
  `C-EDIT-DISCARD`. The entry here is dated from the commit
  that added it, in `tools/stage3-report/report.py`.
- Coverage requires: the extra packet is marked discarded and its composition
  interval lies outside the media edit window, recomputed by the overlay from
  the stream's edit properties; its `pts` and `dts` are the edit's mapping of
  its media times (the media time minus the edit's, plus the presentation
  offset of a leading empty edit), since the reference lists no such packet
  to compare them with.

## KD-CTTS

- Field: `dts` only.
- When: the track's composition offsets are signed (a version 1 table, or
  any table of a QuickTime file, whose format defines them signed) and the
  smallest is negative.
- What each reports: CoreAV keeps decode times as stored. `ffprobe` shifts them
  by the smallest offset. Example: `negative_cts.mp4`, shifted by 1,024 ticks.
- Why: composition offsets are reported as stored, never shifted.
- Coverage requires: the adapter reports the offsets as read signed
  (`ctts_signed`, or `ctts_version` 1 from an adapter without it); the
  reference decode time equals the candidate decode time plus the track's
  smallest composition offset, and no other gating field of the packet
  differs.

## KD-EDIT-SNAP

- Fields: `pts` and `dts`, every packet of the stream by the same amount.
- When: a video track's simple edit list starts at a media time that is not
  the composition time of any sample, including media time 0 when the first
  sample has a positive composition offset.
- What each reports: CoreAV maps the edit's media time to presentation time 0
  (or to the end of the leading empty edit), so a frame that starts before
  it and is still shown at it has a negative `pts`. `ffprobe` maps the first
  composition time at or after the media time instead, so every timestamp is
  smaller by the difference. Measured on edited copies of one file: any
  media time in (217,718, 221,472] gives the shift to 221,472, and audio
  tracks are shifted by the media time exactly. Example: a 90 kHz H.264
  track with media time 220,234, shifted by 1,238 ticks.
- Why: ISO/IEC 14496-12 8.6.6 (`media_time` is the starting time within the
  media of the edit), and the simple edit list is applied exactly.
- Coverage requires: the stream is video; the overlay recomputes the shift
  from the stream's edit properties and the composition times of its
  packets; the reference `pts` equals the candidate's minus that shift, and
  the reference `dts` the candidate's minus that shift plus KD-CTTS's shift
  where KD-CTTS applies (signed offsets with a negative smallest, as in
  QuickTime files that trim their start); no other gating field of the
  packet differs.

## KD-EDIT-MULTI

- Fields: every packet of the stream (`pts`, `dts`, and the packets one
  side lists and the other does not).
- When: CoreAV applies the track's edit list as several edits
  (`EditList::Multiple`, `docs/design/edit-lists.md`): empty edits and
  media edits at rate 1 whose windows increase without overlap.
- What each reports: CoreAV lists each sample once, at the presentation
  time its start has under its edit, applied exactly (decision 2 of the
  design note). `ffprobe` lists each media edit in turn:
  - from a preroll sample (the sync sample before the edit for video, one
    second earlier for audio) to one or two sync samples past the edit's
    last presented sample, so a sample can be listed several times;
  - with times anchored on the decode time of the edit's first sample, and
    edit durations rounded to whole ticks;
  - an empty edit after a media edit as media from time −1.

  The design note gives the measured model, which reproduces `ffprobe`
  packet for packet on the fixtures and on the corpus files with such
  lists. Example: a 20-edit splice of an H.264 MP4 whose edits each skip
  one tick, where `ffprobe` lists 143,828 packets for 140,959 samples.
- Why: one packet per sample, and edit lists applied exactly, never
  approximated (`docs/design/edit-lists.md`).
- Coverage requires: CoreAV reports the list as `multiple`; every CoreAV
  packet's `pts`, `dts` and `discard` equal decision 2's mapping; and the
  reference's packets equal the model's listing, one for one in order
  (`pos`, `size`, `keyframe`, `pts`, `dts`, `discard`). Both are recomputed
  from CoreAV's raw media times and sync flags, and from the edit list,
  timescales, handler and `ctts` presence read from the file. One packet
  that differs leaves the whole stream uncovered.

## KD-CTTS-UNSIGNED

- Fields: `pts` and `dts`.
- When: an ISO (non-QuickTime) file's version 0 `ctts` stores negative
  offsets, as some writers do.
- What each reports: CoreAV reads version 0 offsets as unsigned, as
  ISO/IEC 14496-12 8.6.1.3 defines them, so such an offset is 2^32 minus
  its value and the sample's `pts` is near 2^32 ticks. `ffprobe` reads
  them as signed, and shifts every decode time by the smallest of them, as
  for `KD-CTTS`. Example: a recording (`isom`) whose offsets
  include −3,000, shifted by −10,500. (QuickTime files' version 0 offsets
  are signed by their format; CoreAV reads those signed, for the brand
  `qt  ` and for a file without `ftyp` whose media handler is of component
  type `mhlr`, and `KD-CTTS` covers their shift.)
- Why: ISO/IEC 14496-12 8.6.1.3, and offsets are reported as stored.
- Coverage requires: the file declares a brand other than `qt  `
  (`major_brand`; a QuickTime file read unsigned would be a CoreAV bug);
  the adapter reports a version 0 table read unsigned (`ctts_signed`
  false) with some offset of 2^31 or more; the overlay
  recomputes the signed offsets from each packet's composition and decode
  times: the reference `pts` equals the candidate's minus 2^32 exactly
  where the offset wraps, and equals it elsewhere; the reference `dts`
  equals the candidate's plus the smallest signed offset; no other gating
  field of the packet differs.

## KD-TRUN-SHIFT

- Fields: `pts`.
- When: a track whose samples all come from track fragments, with no edit
  list, and some negative composition offset (`trun` version 1).
- What each reports: CoreAV reports each sample's composition time as
  stored: decode time plus offset. `ffprobe` adds minus the smallest
  offset of the whole track to every presentation time, even when that
  offset first appears in a late fragment, and keeps the decode times.
  Example: an HEVC CMAF recording whose offsets are 0, 100 and −100 ticks,
  where every `ffprobe` `pts` is 100 ticks later. (For offsets in a `ctts`
  box, `ffprobe` shifts the decode times instead: `KD-CTTS`.)
- Why: ISO/IEC 14496-12 8.8.8, and offsets are reported as stored, never
  shifted.
- Coverage requires: the adapter reports a fragment-only track
  (`fragment_start_ticks`) with no edit list; the overlay recomputes the
  smallest offset from each packet's composition and decode times; the
  reference `pts` equals the candidate's minus it, for a negative
  smallest offset; no other gating field of the packet differs.

## KD-TEXT-END

- Field: the stream's `duration_ts`.
- When: a 3GPP timed text track (`tx3g`, `mov_text`) without an edit list.
- What each reports: CoreAV reports the track's media header duration
  (without an edit list nothing else applies). `ffprobe`
  often extends the last sample, and with it the stream, to the latest end
  of any track of the file: that track's edit list durations added up, or
  without an edit list its media header duration. It does not always: on
  two corpus files it keeps the media header duration, and crafted copies
  (edit lists, cover art, track header fields, stored durations) did not
  isolate why. Measured on crafted copies of an `ffmpeg` MP4: shortening the
  video's edit to 4 s makes `ffprobe` end the text track at 4 s, while the
  movie header and the track header durations play no part. Example: a
  text track of 4,430.909 s in a movie whose video edit ends at 4,453.916 s.
- Why: without an edit list, a stream's duration is its media header's. The track's samples end where its media header says;
  the extension is `ffprobe`'s.
- Coverage requires, with every value read raw from the file (the `Facts`
  reader, as for every entry that takes values from CoreAV): CoreAV's
  stream is `mov_text`, its track has a text handler and no edit list, the
  movie has no fragments, CoreAV's duration comes from the media header
  (`duration_source`) and its `duration_ts` equals the track's raw media
  header duration, whose samples end within it; the reference's is the
  latest track end recomputed from the raw `elst`, `mvhd` and `mdhd`
  values, in the stream's ticks rounded half up, and is longer. Any other value is
  not covered. The recomputed end approximates `ffprobe`'s, which goes
  through the other stream's own reported duration: a file whose other
  track has an edit longer than its media, or an end that rounds otherwise
  in that track's ticks, stays uncovered.

## KD-PCM

- Fields: packet grouping (`size`, `extra`, `missing`) and the timestamps that
  follow from it.
- When: a linear PCM track with a constant sample size.
- What each reports: CoreAV reports one packet per container sample (for
  example 96,000 two-byte samples). `ffprobe` groups consecutive samples into
  larger packets (94 packets), and ends the stream's last packet where it
  ends the stream, so that packet may last less or more than its samples.
- Why: one packet is exactly one sample, and samples are never merged or
  split.
- Coverage requires: a PCM codec, a constant sample size equal to channels
  times bytes per sample, and candidate samples that tile every reference packet
  exactly, with consecutive positions, durations summing to the reference
  duration, timestamps anchored at the reference packet, and equal keyframe
  flags. The one exception is the stream's last reference packet, after at
  least one other: when every candidate sample of the stream lasts the same
  and the packet before it lasts exactly its samples, the last packet's
  duration may instead run from its timestamp to where `ffprobe` ends the
  stream, read from CoreAV's own parse. With one edit at media time 0, that
  is the edit's duration in media ticks (also after an empty edit that
  shifts the timestamps); without an edit list, the movie's duration
  (`mvhd`) in media ticks; either rounded to the nearest tick (ISO/IEC
  14496-12 8.2.2, 8.6.6), in a time base equal to the media timescale. In
  every file seen, this end is also `ffprobe`'s stream duration. The one
  file with an empty edit cannot tell the edit's duration from the media's
  (they are equal there); if `ffprobe` takes the shorter of the two, an edit
  longer than its media leaves a packet uncovered. Other edit lists are no
  evidence. Exact half ticks round up, which no file has confirmed (a wrong
  guess, too, only leaves a packet uncovered).
  Packets are never merged or split to hide the difference. The candidate's
  samples may come as runs (`tools/oracle/SPEC.md`, "Runs"); the tiling is
  then checked a run segment at a time, with the same conditions.
- Counting: in `ffprobe`'s packet unit, which the packets bar uses, each
  reference packet is one unit and is covered when the candidate's samples
  tile it exactly; a packet they do not tile is a mismatch
  (docs/design/stage3-report.md, "Packet unit"). In the per-sample count,
  reported beside it, every sample counts as one packet, and the stream is
  covered only when every reference packet of it is tiled.

## KD-PCM-END

- Fields: `duration_ts` and, where it follows, `format_duration_us`.
- When: a constant-size PCM stream (as for `KD-PCM`) whose samples end
  before or after where `ffprobe` ends its last packet.
- What each reports: CoreAV reports the declared duration of the samples. `ffprobe` ends the stream where it ends its last packet
  (`presentation_end`, as `KD-PCM` checks it): the single media edit's
  duration, or without an edit list the movie header's duration in the
  stream's ticks. Example: 24-bit QuickTime PCM of 509,304 samples in a
  movie of 318,318/30,000 s, which `ffprobe` reports as 509,309.
- Why: a stream lasts what its samples last.
- Coverage requires: the stream is constant-size PCM; the reference
  `duration_ts` equals `presentation_end`, recomputed from the stream's
  edits and the movie header, exactly; the candidate's is exactly the
  declared duration rule (the media header duration, capped by the media
  edit) and differs from it. The format duration is covered as for `KD-STTS-SHORT`.

## KD-AAC-IMPLICIT-SBR

- Field: stream `profile` only.
- When: an AAC stream whose `AudioSpecificConfig` declares object type 2 (LC)
  with no explicit SBR or parametric-stereo signalling, at a core sample rate
  of at most 24 kHz, where the SBR data is signalled only implicitly in the
  audio bitstream (ISO/IEC 14496-3, implicit signalling).
- What each reports: CoreAV reports `LC`, the profile the configuration
  declares. `ffprobe` reports `HE-AAC`, which it finds by decoding. Three files
  of the corpus v1 development split.
- Why: profile and level come from the stored configuration, and probe
  never decodes; registered as a known divergence in
  an earlier pull request.
- Coverage requires: `aac` on both sides, candidate profile `LC` with code
  points of object type 2 and no explicit SBR or parametric stereo, reference
  profile `HE-AAC`, and a candidate sample rate of at most 24,000 Hz. Any
  other profile pair, explicit signalling or a higher core rate is not
  covered.

## KD-AAC-LC-UNNAMED

- Field: stream `profile` only.
- When: an AAC stream with a valid LC configuration (object type 2, no
  explicit SBR or parametric stereo).
- What each reports: CoreAV reports `LC`, following ISO/IEC 14496-3. `ffprobe`
  prints no profile for the stream. Three files of the corpus v1 development
  split.
- Why: the oracle adapter maps the stored code points to `ffprobe`'s
  profile names; registered as a known divergence in
  an earlier pull request. CoreAV reads the profile from the
  configuration, which names it.
- Coverage requires: `aac` on both sides, candidate profile `LC` with the code
  points above, and no reference profile. A reference that names a different
  profile is not covered.

## KD-QT-TEXT

- Fields: the stream count (`streams/count`), and the dropped streams'
  packets when `ffprobe` lists them (it does for a text track it reads as
  `mov_text`, not for a chapter track).
- When: the file has QuickTime text tracks (sample entry `text`), such as the
  chapter tracks the Apple QuickTime File Format describes, and CoreAV's
  allowlist does not hold their codec.
- What each reports: `ffprobe` lists each such track as a stream (a `data`
  stream with codec `bin_data` for a chapter track). CoreAV leaves it out of
  the streams and reports it as dropped (`codec_not_allowed`, fourcc
  `text`). `coreav chapters` reads these tracks (`docs/design/text.md`).
- Why: streams whose codec is not on the allowlist are dropped and reported; QuickTime text is not on the default allowlist
  (`docs/design/text.md`).
- Coverage requires: every stream CoreAV drops has fourcc `text` and reason
  `CodecNotAllowed`; the reference's streams at exactly those indexes have
  fourcc `text`; and every other stream index is present on both sides. Any
  other dropped codec, or a missing stream CoreAV does not report as
  dropped, is not covered here; a file that also drops other tracks is
  KD-NOT-ALLOWED's.

## KD-NOT-ALLOWED

- Fields: the stream count (`streams/count`), and the dropped streams'
  packets that `ffprobe` lists (streams pair by index, so a dropped stream
  has no candidate and its other fields are not compared).
- When: CoreAV's codec allowlist does not hold the codec of one or more
  tracks, such as camera metadata tracks (`rtmd`, `gpmd`, `cndm`, `mebx`,
  `djmd`), encrypted audio (`enca`), timed text (`wvtt`) or audio codecs
  outside the default allowlist.
- What each reports: `ffprobe` lists every track as a stream. CoreAV leaves
  the track out of the streams and reports it as dropped
  (`codec_not_allowed`, with its fourcc); it never reaches a decoder.
- Why: streams whose codec is not allowed are rejected at probe time and
  never reach any decoder; disallowed codecs are dropped and
  reported. KD-QT-TEXT is the same divergence for
  QuickTime text tracks, which `coreav chapters` reads; this entry covers
  every other dropped codec.
- Coverage requires: every stream CoreAV drops has reason
  `CodecNotAllowed`; the reference's streams at exactly those indexes have
  the dropped fourcc and a codec the default allowlist does not hold (as
  `ffprobe` names it: not `h264`, `hevc`, `av1`, `vp9`, `prores`, `aac`,
  `mp3`, `opus`, `mov_text`, a `pcm_` codec or a `tmcd` track), so an
  allowed codec CoreAV fails to recognise is never covered; or, for a
  protected sample entry (`enca`, `encv`, `enct`, `encs`, ISO/IEC 14496-12
  8.12), dropped for its protection and reported by `ffprobe` under its
  original format, that format is what the `frma` box of the entry's
  `sinf` holds; and every other stream index is present on both sides.
  A missing stream CoreAV does not report as dropped, or a dropped stream
  whose fourcc differs from the reference's, is not covered; nor are the
  other fields of the file (a format duration that differs because of the
  dropped track is judged on its own).
- Mixed drops: when CoreAV drops QuickTime text tracks and other tracks in
  one file (a file with a `priv` data track and a `text` track, for
  example), this entry covers all of them: the `text` tracks must meet
  KD-QT-TEXT's conditions and the others this entry's, each part judged as
  if the other part's streams were absent. The file is counted here alone,
  since only this entry makes it coverable.

## KD-COVER-ART

- Fields: the stream count (`streams/count`), and the cover pictures'
  packets that `ffprobe` lists.
- When: the file's metadata item list (`moov/udta/meta/ilst`, or, when
  that holds no picture, `moov/meta/ilst`) holds cover art: the `covr` item
  of iTunes-style metadata, or, in QuickTime metadata with a `keys` box,
  the item of the key `com.apple.quicktime.artwork`.
- What each reports: `ffprobe` lists each cover picture as a video stream
  (an attached picture, `mjpeg`, `png` or `bmp`, with no sample-entry
  fourcc) with one packet; in every corpus file, after the tracks. CoreAV
  lists tracks only: a cover picture is metadata, not a track (ISO/IEC
  14496-12 8.3 and 8.11; the item list is QuickTime File Format metadata),
  so it is neither a stream nor dropped.
- Why: the streams CoreAV reports are the file's tracks.
- Coverage requires: the reference streams that CoreAV neither lists nor
  reports as dropped are exactly the last streams of the reference by
  index, as many as the picture item has `data` boxes (read from the file;
  `meta` must start with its `hdlr`; at most 64); each a video stream with
  no sample-entry fourcc (a track always has one, so a track CoreAV fails
  to list is never covered) whose codec is the one the picture's type
  indicator names (type-set 0 and type 13 JPEG: `mjpeg`, 14 PNG: `png`, 27
  BMP: `bmp`), in order; at the packets level, one packet each. Any other
  missing stream must be covered by KD-QT-TEXT or KD-NOT-ALLOWED (the two
  then apply together). A picture of another type, one more or fewer
  stream than pictures, pictures listed before a track, or pictures in both
  `udta/meta` and `moov/meta`, are not covered.

## KD-CHAPTER-TEXT

- Fields: the stream's `kind` and `codec_name`, and its packets.
- When: a 3GPP timed text track (sample entry `tx3g`) is named by another
  track's `chap` track reference (`tref`, ISO/IEC 14496-12 8.3.3; chapter
  lists in the Apple QuickTime File Format), as podcast and audiobook M4A
  files do for their chapter titles.
- What each reports: CoreAV lists the track as what its sample entry is, a
  `mov_text` subtitle stream with one packet per sample; `coreav chapters`
  reads it as the chapter track (`docs/design/text.md`). `ffprobe` lists it
  as a `data` stream with codec `bin_data`, lists no packet of it, and
  shows its samples as chapters. Measured on crafted copies of an `ffmpeg`
  MP4 with a `mov_text` track: adding a `chap` reference to it from the
  audio track turns `ffprobe`'s stream into `bin_data` with no packets, and
  so does a reference from a disabled track. Example: the two `tx3g`
  tracks of an M4A podcast, 9 and 15 samples.
- Why: CoreAV reports what the file stores, with one packet
  per sample; a chapter reference does not change the track's
  sample entry.
- Coverage requires: the reference stream at that index has kind `data`,
  codec `bin_data` and fourcc `tx3g`; CoreAV's has kind `subtitle`, codec
  `mov_text` and fourcc `tx3g`; the track at that position in the file has
  a `tkhd` track ID listed in a `tref/chap` box of some track (read from
  the box headers; a `chap` box over 4 KiB is ignored); at the packets
  level, the reference lists no packet for the stream and the units are
  CoreAV's extra packets, each with the `pts` and `dts` the stream's edit
  list maps its media times to (the reference has no packet to check them
  against). A `tx3g` track no `chap` reference names, or any
  other codec, is not covered.

## KD-DECLARED-SIZE

- Fields: stream `width`, `height` and `display_aspect_ratio`.
- When: the first sample entry of the track declares a width and height
  (read from the box headers) that differ from those of the stream's first
  decoded frame, which `ffprobe` reports.
- What each reports: CoreAV's `width` and `height` are what the sample
  entry declares (`docs/design/frame-sizes.md`, decision 1); the sizes the
  codec's parameter sets and frame headers give are printed apart, as
  `video.parameter_set_size` and `video.scanned_size`, with a warning.
  `ffprobe` reports the decoded size. Example: `size_entry_understated.mp4`
  declares 320×240 and decodes to 1280×720.
- Why: the declared size is what the file states; replacing it would hide
  the disagreement a policy gate needs to see.
- Coverage requires: CoreAV's values equal the sample entry's, `ffprobe`'s
  differ from them, and `ffprobe`'s equal the first frame `ffprobe -show_frames`
  decodes.
- `coreav ffprobe` prints the same declared `width` and `height`, and the
  coded size the bitstream gives, as `ffprobe` does; its fixtures with a
  declared size other than the decoded one are set aside under this record
  (`tools/compat-corpus/run.py`, `SET_ASIDE`).

## KD-HEVC-WINDOW

- Fields: stream `profile`, `level` and `color_range`.
- When: an HEVC stream whose sequence parameter set has a conformance
  window that leaves no picture, which H.265 7.4.3.2.1 does not allow (the
  window's left and right offsets must leave part of the width, and its
  top and bottom offsets part of the height).
- What each reports: `ffprobe` 9.0.2 drops the whole parameter set
  ("SPS 0 does not exist") and prints none of the three fields. CoreAV
  reports the profile and level, which come before the window in the set
  (H.265 7.3.3), and the colour range. `ffprobe` 7.0.2 printed them too.
  Examples: `size_hevc_window.mp4` and `size_hevc_mixed_window.mp4`.
  Decoding changed with the parameter set: `ffmpeg` 9.0.2 decodes no
  picture of such a stream and fails, where 7.0.2 ignored the window and
  decoded the coded size. `coreav transcode` and `coreav ffmpeg` reject
  the stream as an invalid structure before decoding
  (`docs/design/transcode.md`, "Pipeline").
- Why: the profile and level are readable whatever the window says, and a
  policy gate needs them.
- Coverage requires: the codec is HEVC on both sides, the reference has none
  of the three fields, and `scripts/oracle_kd.py` reads the first sequence
  parameter set of the sample entry's `hvcC` itself (ISO/IEC 14496-15
  8.3.3.1): its window leaves no picture, and its `general_profile_idc` and
  `general_level_idc` give CoreAV's profile and level.

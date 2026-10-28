# Known divergences: policy verdicts versus the oracle

Every intentional difference from the black-box oracle is documented with
a machine-readable twin. This page covers the oracle
comparison of `coreav check` (`docs/design/policy-check.md`): the failed
rules of a verdict against those that `scripts/policy_ref.py` finds in
`ffprobe`'s output. The twin is
[`known-divergences-policy.json`](known-divergences-policy.json);
`scripts/policy_ref.py` applies it with one adjustment per entry
(`KNOWN_DIVERGENCES`), and `tools/oracle/tests/test_policy_ref.py` keeps
this page, the twin and the code in sync. A case counts as covered only if
the adjusted oracle document gives exactly the verdict's failed rules;
covered cases are reported apart from direct agreement.

The oracle is `ffprobe` 7.0.2. Conditions are checked from the file's own
box headers (for Matroska and WebM, its EBML elements) and the oracle's
output, never from file names or from CoreAV's output. The Matroska
entries (`KD-P-MKV-*`, `MATROSKA_DIVERGENCES`) apply to Matroska and WebM
files only, whose mapping (`from_ffprobe_matroska`) is described in
`scripts/policy_ref.py` and `docs/design/policy-matroska.md`. Probe divergences that the mapping already follows (the
presentation end as duration, while `ffprobe`'s format duration starts at
the earliest stream start; no stream bit rates for
fragmented files) are described in `scripts/policy_ref.py`.

## KD-P-PROTECTED

- Field: the codec of a stream, and through it the rules `codecs`,
  `min_count`, `max_count` of its kind and `unidentified`.
- When: the stream's first sample entry is a protected one (`encv`, `enca`,
  `enct`, `encs`, `encm`, `encf`; ISO/IEC 14496-12, 8.12), read from the
  box headers (`moov`, `trak`, `mdia`, `minf`, `stbl`, `stsd`).
- What each reports: CoreAV does not identify the codec of a protected
  sample entry, so the probe drops the stream (`dropped_streams`, fourcc
  `encv`) and the policy treats it as unidentified. `ffprobe` names the
  original format from the protection scheme's `frma` box (for example
  `h264`) and lists the stream as an ordinary one. Example: an encrypted
  CMAF file with one `encv` video track; `strict-gate` rejects it for an
  unidentified stream and no video, the oracle's verdict would accept it.
- Why: an encrypted stream cannot be decoded or inspected without its key,
  so a gate must not take it as the codec it wraps: only allowlisted
  inputs reach the tools behind the gate. Streams whose codec is not identified are dropped.
- Adjustment: the oracle's streams at those positions become dropped
  streams with the walked fourcc.

## KD-P-FRAGMENT-ORIGIN

- Field: the stream and file durations, and through them `max_seconds` and
  the overall bit rate.
- When: the file is fragmented and the first track fragment of every
  stream the oracle lists has a decode time (`tfdt`, ISO/IEC 14496-12,
  8.8.12) above 0, read from the box headers with the track's `mdhd`
  timescale.
- What each reports: CoreAV's stream duration is the length of the media,
  the observed sample span after the fragments are read. `ffprobe` reports a duration that runs from 0 to the
  stream's end, so it counts the first fragment's decode time. Example: an
  HLS rendition in fragmented MP4 whose fragments start at 10 s with 600 s
  of media; CoreAV reports 600 s, `ffprobe` 610 s.
- Why: a policy limits how long the media lasts; a segment cut from the
  middle of a stream carries its position in the decode times, not extra
  media.
- Adjustment: each oracle stream duration minus its track's first decode
  time; the file duration is the longest of them.

## KD-P-UNIDENTIFIED-DURATION

- Field: the file duration, and through it `max_seconds` and the overall
  bit rate (`unknown` on CoreAV's side).
- When: the file is fragmented, its movie extends box has no
  `fragment_duration` (`mehd`), and every stream the oracle lists is
  unidentified by the mapping of `scripts/policy_ref.py` (for example
  E-AC-3 or a protected entry).
- What each reports: CoreAV takes the duration of a fragmented file from
  `mehd`, else from its identified streams; with neither the
  duration is unknown and the rules that need it reject with `unknown`.
  `ffprobe` measures the unidentified stream's fragments.
- Why: unidentified streams are dropped, and their samples are not read, and the fail-closed rule of `docs/design/policy-check.md`
  applies.
- Adjustment: the oracle's file duration becomes unknown.

## KD-P-CHAPTER-TEXT

- Field: the kind and codec of a stream, and through them the rules of the
  `subtitle` kind and `unidentified`.
- When: the stream's track is named by another track's `chap` track
  reference (`tref`, ISO/IEC 14496-12 8.3.3; a QuickTime chapter list), its
  handler is `text`, `sbtl` or `subt`, and the oracle lists it as a data
  stream.
- What each reports: CoreAV describes every track by its handler and its
  sample entry: a `tx3g` chapter track is a subtitle stream with codec
  `timed_text`, and the policy applies the subtitle rules to it. `ffprobe`
  turns chapter tracks into chapters and lists the track itself as
  `bin_data`. Example: a podcast M4A with two `tx3g` chapter tracks;
  `web-upload` rejects it for subtitle streams it does not allow.
- Why: a gate checks the tracks a file carries, whatever another track
  uses them for; tools behind the gate see the tracks too.
- Adjustment: such oracle streams with a `tx3g` entry become subtitle
  streams with codec `timed_text`; others stay unidentified.

## KD-P-DECLARED-DURATION

- Field: the file duration, and through it `max_seconds` and the overall
  bit rate.
- When: the file is fragmented and its movie extends box declares a
  `fragment_duration` (`mehd`, ISO/IEC 14496-12 8.8.2) above 0.
- What each reports: CoreAV takes a fragmented file's duration from `mehd`, so a file whose fragments stop early, such as a truncated
  DASH representation, is as long as it declares. `ffprobe` measures the
  fragments present. Example: a file declaring 664.6 s with 60 s of
  fragments; `strict-gate` rejects it for a duration above 600 s.
- Why: in a movie with fragments, the movie duration is `mehd` when present; the declared length is what a player of the complete
  stream presents, and the longer of the two values is the one the
  duration rule must not miss.
- Adjustment: the oracle's file duration becomes `mehd` over the movie
  timescale (`mvhd`).

## KD-P-EDITED-BIT-RATE

- Field: a stream's bit rate, and through it `max_bit_rate` of its kind.
- When: the track's edit list presents less than its media. That is a
  simple list (one media edit after at most one empty edit), or several
  edits CoreAV applies (`docs/design/edit-lists.md`), whose media edits
  add up to less than the media header duration. All of it is read from
  the box headers (`mdhd`, `elst`).
- What each reports: CoreAV's bit rate is every sample's bytes over the
  stream duration, and the duration is what the edit list
  presents. `ffprobe` reports the stored rate: the `btrt`
  average of the sample entry, or the bytes over the media header
  duration. Example: an AAC track of 10 s whose edit presents 6.5 s;
  CoreAV 106,848 b/s, `ffprobe` 69,290 (`btrt`). The entry also applies,
  with a small difference, to most `ffmpeg` AAC tracks, whose edit hides
  the encoder delay (441,000 of 442,024 ticks).
- Why: the bit rate is the exact one, the stored bytes over the stream
  duration. The samples the edit list hides still occupy the file, so
  the presented rate is the one a delivery budget meets.
- Adjustment: the oracle's bit rate of the stream becomes the sum of the
  `stsz` sample sizes × 8 over the stream duration,
  recomputed from the box headers. It applies only when the oracle's rate equals the `btrt`
  average, or the bytes over the media header duration within 1 b/s.
  Fragmented files are left out: their stream bit rates are not compared.

## KD-P-MKV-UNSIZED

- Field: the verified picture size of a Matroska video stream, and through
  it `max_width`, `max_height` and `max_pixels`.
- When: the oracle names the stream's codec `mpeg1video`, `mpeg2video`,
  `mpeg4`, `theora`, `mjpeg` or `ffv1`, and gives it a size.
- What each reports: the block scan reads picture sizes from H.264, HEVC
  and AV1 parameter sets and VP8, VP9 and ProRes frame headers only, so
  CoreAV's `parameter_set_size` and `scanned_size` of these codecs are
  `null` and a resolution rule under the default `size_source` rejects the
  stream as `unknown`. `ffprobe`'s decoder gives a size. Example: a DVD rip
  in MPEG-2 video; `web-upload` rejects it for unknown width, height and
  pixel count, the oracle's verdict would accept it.
- Why: `docs/design/policy-matroska.md`, decision 3 (fail closed): a size
  CoreAV has not verified is not a bound; `"declared"` is the choice for
  callers who trust `PixelWidth` and `PixelHeight`.
- Adjustment: the oracle's verified sizes of those streams become `null`.

## KD-P-MKV-LACE-TIMES

- Field: the duration and bit rate of a Matroska stream, and through them
  `max_seconds`, the overall bit rate of a live recording and the stream's
  `max_bit_rate`.
- When: the stream's TrackEntry has no `DefaultDuration`, a SimpleBlock or
  Block of its track is laced (the lacing bits of the block header, RFC
  9559 10.2), both read from the EBML elements, and the oracle names a
  codec other than `opus`.
- What each reports: a later frame of a lace has no timestamp of its own
  (RFC 9559 10.3); without `DefaultDuration` its time follows from the
  durations of the frames before it, which only the codec states. CoreAV
  reads them for Opus (RFC 6716 3.1) and for no other codec, so the
  stream's duration and bit rate are `null` and the rules that need them
  reject the file as `unknown`. `ffprobe` infers the times with its codec
  parsers. Example: Vorbis audio laced by a muxer without `DefaultDuration`;
  every example policy rejects it for an unknown duration.
- Why: `docs/design/policy-matroska.md`, decision 3 (fail closed): a span
  that might miss frames whose times are unknown does not bound the
  duration.
- Adjustment: the oracle's duration and bit rate of those streams become
  `null`.

## KD-P-MKV-FRAME-END

- Field: the duration and bit rate of a Matroska audio or video stream, and
  through them `max_seconds` and the stream's `max_bit_rate`.
- When: the stream's TrackEntry has no `DefaultDuration` (EBML elements),
  the oracle names a codec other than `opus`, and the oracle's span of
  packet starts differs from its span of packet ends.
- What each reports: Matroska states a frame's duration only through
  `BlockDuration` or `DefaultDuration`; without them CoreAV ends the frame
  where it starts (Opus excepted, whose packets state their duration), so
  the stream's span runs to its last frame's start. `ffprobe` adds the
  duration its codec parser derives, one frame longer. Example: AAC audio
  muxed by `ffmpeg` without `DefaultDuration`, against a limit 10 ms above
  CoreAV's span.
- Why: `docs/design/policy-matroska.md`, decision 2: the scan reads block
  and frame headers, not the codec's frame syntax; the difference is one
  frame, and a file's `Duration` still counts for the duration rule.
- Adjustment: the oracle's duration of those streams becomes the span of
  their packet starts, and their bit rate the bytes over it.

## KD-P-MKV-UNLISTED

- Field: the duration and bit rate of a Matroska stream, and through them
  `max_seconds` and the stream's `max_bit_rate`.
- When: the stream's TrackEntry has a `ContentEncoding` CoreAV does not
  undo (compression other than header stripping, or more than one
  encoding), read from the EBML elements.
- What each reports: `coreav-mkv` lists no frames of such a track
  (`packets_available` is `false`), so CoreAV's duration and bit rate of
  the stream are `null` and the rules that need them reject the file as
  `unknown`; `invalid_tables` and `unidentified` also reject it by default.
  `ffprobe` decompresses the frames and times them. Example: PGS or VobSub
  subtitles that a muxer compressed with zlib; every example policy
  rejects the file.
- Why: `docs/design/policy-matroska.md`, decision 3 (fail closed): frames
  that were not listed do not bound the duration.
- Adjustment: the oracle's duration and bit rate of those streams become
  `null`.

## KD-P-MKV-UNVERIFIED

- Field: every Matroska stream's duration, bit rate and scanned size, and
  through them `max_seconds`, the stream's `max_bit_rate` and the
  resolution rules.
- When: the file's EBML elements hold a structure behind which a block
  listing may miss frames: bytes after the Segment's declared end, an EBML
  header or an element of unknown kind among the Segment's children, an
  element running past its parent or the file (a truncated file included),
  a second `Info` or `Tracks`, a BlockGroup with two Blocks, a block of a
  track no TrackEntry defines, an element whose length its schema type
  does not allow, or a string or binary longer than the audit allows
  (`matroska_audit`, an EBML walk written from RFC 8794 and RFC 9559).
- What each reports: CoreAV's scan audits the structure first and fails on
  such a file, so every stream's `frames_listed` is `false` (the
  `invalid_tables` rule rejects it) and its duration, bit rate and scanned
  size are `null`. `ffprobe` reads past the structure. Example: a WebM file
  whose Segment size ends after its first Cluster; `ffmpeg` decodes the
  Clusters after it.
- Why: `docs/design/policy-matroska.md`, decision 8: values that may miss
  frames a decoder reads do not bound the file.
- Adjustment: the oracle's durations, bit rates and scanned sizes of every
  stream become `null`.


# Probe durations: declared and derived

Status: Accepted. Scope: the movie-level durations of `coreav probe` for ISO
base media files (cause ① of the stage 3 probe oracle gap).
Stream durations follow the stream duration rules;
this note does not change them.

## Two values

Probe reports raw values next to derived ones. A movie has a
declared duration and a derived one, and probe reports both:

- **`duration`** (declared, unchanged): the movie header (`mvhd`) duration;
  with fragments, the movie extends header (`mehd`); else the longest known
  stream. This is what the file says about itself.
- **`presentation_duration`** (derived, new): the latest stream end minus
  the earliest stream start, over every stream whose timing is known. It is
  what the streams span, and it is how `ffprobe` defines its format
  duration.

They differ when the movie header is rounded to a coarse movie timescale
(1/1000 or 1/600 s against stream ticks), when a muxer left a stale value,
or when every stream starts after 0 (the declared duration counts the
leading empty time, the span does not).

## How a stream is placed

- **Start:** the stream's first presented instant. With an edit list, the
  leading empty edit, converted to the stream's media ticks and rounded to
  the nearest tick (its samples are presented at media ticks). Without an
  edit list the presentation timeline is the composition timeline, so the
  start is the first sample's composition time: its `ctts` offset, or in a
  fragmented movie whose movie box holds no samples, the first fragment
  sample's (read when the fragments are scanned).
- **Duration:** the stream duration: the media edit capped by the media
  header, or the fragmented movie's rule for fragments.
- **End:** start plus duration.
- **An edit that lasts to the end of the media.** In a movie with
  fragments, a media edit of duration 0 means the edit lasts to the end of
  the media (ISO/IEC 14496-12 8.6.6): the media's length is not known when
  the movie box is written. The stream's duration is then the media from
  the edit's media time on, the part before it cut only where it lies
  after the first sample's composition time (an AAC priming edit cuts its
  1,024 samples; an edit that only undoes a B-frame offset cuts nothing),
  which is what `ffprobe` counts. Packets are presented from the media time
  to the end. Outside movies with fragments the specification gives a
  duration of 0 no such meaning, so the edit list is unsupported there. Rejected: accepting it in every movie, which would give
  timing to a shape the specification does not define. A dropped stream
  with such an edit has no known end, because its sample tables and
  fragments are not read, so it is left out.
- **Dropped streams** (codecs outside the allowlist) are included. They
  describe the file, and `ffprobe` counts them. Their timing comes from what
  is read before the drop: the media header and the edit list. Their
  duration is the media edit capped by the media header duration; their
  sample tables are never read, so there is no `stts` term. In a movie with
  fragments the media header describes only the movie box samples, so a dropped stream's duration is its media edit alone, as
  for a kept stream whose fragments were not scanned. Once the fragments
  are scanned, a dropped stream whose movie box holds none of its samples
  (a media header duration of 0) takes its media from them: the scan sums
  the durations of its fragment samples from the run headers and track
  defaults, and keeps its first sample's composition offset and its first
  `tfdt`, without reading any sample. The first sample's composition time
  is then that `tfdt` plus that offset, for kept and dropped streams alike:
  an edit that lasts to the end of the media from a media time before it
  (encoder priming in a segment that does not start at 0) cuts nothing. An edit that lasts to the end of the
  media then applies as for kept streams. Encrypted (`enca`, `encv`) and
  disallowed audio in CMAF files is the common case. Rejected: reading a
  dropped stream's sample tables, which the allowlist keeps closed.
- A stream whose start or duration is unknown (an unsupported edit list, a
  fragmented track without `mehd` or a scan) is left out. When no stream is
  known, the derived value is `null`.

The value is an exact rational. Rounding happens only where an output needs
integers: in movie ticks for probe's JSON, and in the oracle
adapter, which counts as `ffprobe` counts its format duration: each
stream's duration in whole stream ticks first (its `duration_ts`; a media
edit of 1 ms is 12.8 ticks at 12,800 Hz, so 13), then each stream's start
and that duration in microseconds on their own. Measured on the stage 3
baseline at a213539: of the 48 files whose only uncovered field was the
format duration, 46 agree under this rule; most differed by 2 µs, a
fraction of a tick of an edit that ends between two samples.

## Composition offsets in QuickTime files

The start of a stream without an edit list, and every packet's
presentation time, depend on the composition offsets. ISO/IEC 14496-12
8.6.1.3 makes `ctts` offsets unsigned in version 0 and signed in
version 1. The QuickTime File Format ("Composition offset atom") defines
them as signed in its version 0, and QuickTime writers store negative
offsets there, with a `cslg` box giving the shift. Read unsigned, such an
offset is near 2^32 ticks. A file whose major brand is `qt  ` therefore
reads its offsets as signed (`Track::ctts_signed`). So does a file
without `ftyp` whose media handler (`hdlr`) names QuickTime's component
type `mhlr`: the QuickTime File Format makes `ftyp` optional, and ISO/IEC
14496-12 8.4.3 requires that field (`pre_defined`) to be 0, so the file
says which format it is. A corpus file of that shape (`wide` atom, `cslg`,
no `ftyp`) stores negative version 0 offsets. Other files keep the ISO
reading, including a file without `ftyp` and without that handler, which
ISO/IEC 14496-12 4.3 reads as `mp41`. Rejected:
- reading every version 0 offset of 2^31 or more as negative, which would
  approximate a file the ISO reading defines. An ISO file with such
  offsets (brand `isom` or `mp42`) is reported as stored, and
  `KD-CTTS-UNSIGNED` covers `ffprobe`'s signed reading;
- treating every file without `ftyp` as QuickTime, which contradicts
  ISO/IEC 14496-12 4.3 when nothing in the file names QuickTime;
- a known divergence for the `ftyp`-less QuickTime file, which would keep
  reporting timestamps near 2^32 ticks for a file whose format defines
  them signed.

In track fragments, `trun` version 1 stores signed offsets (ISO/IEC
14496-12 8.8.8), and CoreAV reports them as stored. `ffprobe` adds minus
the track's smallest offset to every presentation time instead of
shifting the decode times as it does for `ctts`. Measured black-box on
`ffmpeg`-written fragmented H.264 with offsets edited in place: the shift
is the smallest offset of the whole track, even when it first appears in
the sixtieth fragment, and it does not depend on the first sample's
offset. `KD-TRUN-SHIFT` covers it, recomputed from CoreAV's composition
and decode times.

## The oracle

The oracle's `format_duration_us` keeps its definition, `ffprobe`'s format
duration. `ffprobe` spans the audio and video streams, and a subtitle,
timecode or other data stream only when it ends less than a second after
them; without audio or video, every stream. Measured black-box beside 2 s
and 4 s of H.264: `mov_text` cues ending at 2.99 s and 4.99 s count, ones
ending at 3.0 s and 5.0 s or later do not; a `tmcd` track 0.5 s past the
video counts, one 8 s past it does not; audio 3 s past the video counts; a
file of only a `mov_text` track spans it.
`Movie::ffprobe_format_duration_seconds` is that span (and
`Movie::ffprobe_format_duration_micros` the value in microseconds, each
stream's duration in whole ticks first); the library adapter uses it.
The native `presentation_duration` keeps spanning every stream: a
subtitle track that runs past the video is presented, and the derived value
describes the presentation, not `ffprobe`'s choice. Mapping `format_duration_us` to that derived span,
rather than to the declared value, states the same quantity.
`KD-FMT-ORIGIN`, which covered the empty-edit case of the old mapping, is
retired. `KD-STTS-SHORT` still covers a format duration that follows from
a covered stream duration, now with each side's span computed from its own
stream durations.

For a track whose samples all come from fragments, `ffprobe`'s stream
duration counts from decode time 0: the first `tfdt` plus the samples'
durations, or, when a segment index (`sidx`) references the track, the
index's earliest presentation time plus its subsegment durations
(measured black-box with and without a `sidx`). CoreAV's duration is the
samples' own span; `KD-FRAG-START` covers the difference,
and the format duration follows from it. The adapter reports the `sidx`
end it reads (`sidx_end_ticks`), and for each dropped stream the timing
CoreAV gives it (`duration_ticks`, its first `tfdt`, its `sidx` end), so
the overlay can count a dropped stream as `ffprobe` counts it. A dropped
stream whose start `ffprobe` takes from its bitstream (a DVD subtitle
track's first packet) stays uncovered: CoreAV does not read a dropped
stream's samples.

`coreav ffprobe -of json -show_format -show_streams` computes its format
`duration` from its own reported stream starts and durations. Those
streams include codecs the native probe drops, and their timing can differ
from the native container-derived values. The policy check keeps
evaluating native `duration` (the declared value), as it did before.

## Measurement

On moov-only copies of all 2,356 corpus v1 ISO base media files (the movie
box and every top-level box header at their offsets, so probe and `ffprobe`
read the same structure), comparing `format_duration_us` with `ffprobe`
7.0.2, before known divergences, with the adapter's first rounding (each
stream's exact duration to microseconds; whole stream ticks came later,
measured on the stage 3 corpus in the oracle section):

| Set | Files | Declared agrees | Derived agrees |
|---|---|---|---|
| `edge` | 1,000 | 642 | 851 |
| `real`, development | 671 | 323 | 603 |
| `real`, held out | 284 | 120 | 250 |
| **All** | **1,955** | **1,085** | **1,704** |

The other 401 files are those where `ffprobe` reports no format duration
(353) and those probe rejects (48). Most of the remaining differences come
from stream durations that differ by definition (the declared presentation
duration against `ffprobe`'s smallest of the media header, the `stts` sum
and the edit list, which `KD-STTS-SHORT` covers where it applies), and from dropped streams,
whose `stts` term probe does not read. The full probe oracle run of an earlier change
reports the results with known divergences applied.

## Alternatives considered

- **Replace the declared value with the derived one:** loses what the file
  says; probe output reports raw values next to derived ones, and the policy check and the fragmented
  probe rely on the declared value.
- **Redefine the oracle's format duration as the movie header:** changes
  the reference to suit the candidate; the oracle stays `ffprobe`'s.
- **Leave dropped streams out:** simpler, but the span would then describe
  only the streams this caller allowed, not the file; 90 files would agree
  less than with the declared value.
- **`ffprobe`'s stream duration rule** (the smallest of `mdhd`, `stts` and
  the edit list) for the derived value: a stream's duration is its declared
  presentation duration; the format span follows that choice.

# Edit lists with several entries

Status: Proposed.

The probe applies one edit list shape exactly: at most one empty edit
followed by one media edit at rate 1 (the simple model). Anything else is
reported as unsupported (presentation timestamps are null, with a warning),
never approximated. Wider support "is added only with a recorded decision
and oracle coverage". This note is that decision, for edit lists with
several media edits. The oracle coverage is the fixtures and the corpus
files listed under "Oracle coverage" below.

Specification: ISO/IEC 14496-12 8.6.5 (`edts`) and 8.6.6 (`elst`). Each
entry has:
- a `segment_duration` in the movie timescale;
- a `media_time` in the media timescale, where −1 marks an empty edit;
- a `media_rate`, where 0 marks a dwell.

The entries play one after another on the presentation timeline.

## Why

On the stage 3 corpus, seven files have an edit list outside the
simple model: 720,025 of the 1,165,848 uncovered packets. Their lists are of
three kinds:
- splices: QuickTime editing, up to 6,069 entries in one audio track;
- cuts that drop the first frames;
- two edits with a gap between them.

All of them are media edits at rate 1, in increasing media order and
without overlap. With unsupported lists, a user gets null timestamps for
every packet of such a track.

## Decisions

### 1. Which lists are applied

A track's edit list is applied exactly (`EditList::Multiple`) when it meets
all of these:
- every entry is an empty edit (`media_time` −1) or a media edit at
  `media_rate` 1 with a `segment_duration` above 0;
- the media edits' windows, `[media_time, media_time + duration)` in media
  ticks with the duration converted exactly from the movie timescale, are in
  increasing order and do not overlap: each starts at or after the end of
  the one before;
- it is not one of the simple shapes, which keep `EditList::Simple`;
- the movie has no fragments;
- every entry's start on the presentation timeline fits in 64 bits.

Everything else stays `EditList::Unsupported`, with null presentation
times and the warning:
- a rate other than 1, including a dwell (rate 0);
- a `media_time` below −1;
- media edits whose windows overlap or go backwards;
- a media edit of duration 0 when it is not the simple model's open-ended
  edit;
- several edits in a movie with fragments.

Rejected alternatives:
- **Applying overlapping or repeating windows.** A sample presented twice
  would need two packets, and the packet model has one packet per sample.
  Reporting only its first presentation would hide the second.
- **Applying dwells and other rates.** No corpus file has them, and the
  presentation duration of a sample at another rate needs its own rules.
  They stay unsupported until a file and a fixture call for them.
- **Applying a zero-length media edit in a movie without fragments**, as
  an edit that presents nothing. ISO/IEC 14496-12 8.6.6 allows that
  reading, and `ffprobe` then lists one packet. Rejected for its effect on
  the rest of the probe output: the track's duration, what its edit list
  presents, becomes 0, its bit rate becomes undefined (the
  probe divides by the duration), and
  `coreav check` then reports a `max_bit_rate` rule as unknown, so
  `strict-gate` rejects a file the reference accepts. A file holding only
  such a track would also get a format duration of 0. Such lists stay
  unsupported, with null timestamps and the warning, and every writer
  refuses them.
- **Applying several edits in movies with fragments.** The model,
  fixtures and corpus files here have no fragments. A crafted fragmented
  file showed `ffprobe` ignoring the edit list there, which is a different
  behaviour to measure first.

### 2. Presentation times

Entry k starts on the presentation timeline at `P_k`, the sum of the
durations of the entries before it, converted exactly to media ticks. This
is a rational number, never rounded, as for the simple model's empty edit.

Take a sample whose composition interval is `[c, c + δ)`:
- **Its edit** is the media edit whose window contains `c`. If no window
  contains `c`, it is the first window that the interval overlaps: the
  sample starts in a gap, or before the first media edit, and is still
  shown.
- **Its times** are `pts = c − media_time_k + P_k` and
  `presentation_dts = dts − media_time_k + P_k`.
- **Presented:** the sample is presented when its interval overlaps any
  window (for a zero-duration sample, when `c` lies in one).
- **Not presented:** a sample that no window shows is not presented
  (`discard`). It is mapped by the next media edit after it, or by the last
  one when it comes after all of them, as a preroll sample is.

This is the simple model's rule applied per edit:
- a frame that starts before its edit and is still shown gets a
  presentation time before the edit's start;
- composition offsets are reported as stored.

**This rule is for packets, and only for packets.** A packet carries one
time, so a sample two media edits show has to be given one of them
(decision 3). Anything that works in *intervals* — a subtitle cue, a
chapter, a presented duration, a span on the timeline — intersects the
sample's composition interval with **every** media edit's window instead,
and maps each overlap through its own edit, merging runs of edits with
nothing between them. The two readings differ wherever the edits are
contiguous: an edit list that cuts the media into pieces and plays them
back to back presents exactly what no edit list does, and the packet rule
would cut every interval at the first boundary it met.
`docs/design/edit-list-writers.md` ("`coreav-text` timing") writes the
interval rule out; `Track::presentation` and `Track::duration` take it for
the track as a whole (decision 5).

Rejected alternatives:
- **Choosing the edit by the largest overlap.** The edit is then not
  where the sample's first instant is shown, so the time is no longer the
  presentation time of the sample's start.
- **Rounding `P_k` to whole media ticks**, which `ffprobe` does
  (below). The simple model keeps exact rationals, and so does this one.

### 3. A sample in two windows

A sample whose interval overlaps two media edits (an audio frame across a
splice, for example) is reported once, at its edit. That is the edit where
its start is shown, as defined above. One packet per sample
cannot express the second presentation. `Track::edits` keeps the full list
for a consumer that needs every presentation, such as a decoder that trims
at the splice or a writer of cues (decision 2, "This rule is for
packets").

Rejected alternative: treating such lists as unsupported. In the corpus,
every frame of the 6,069-edit audio track straddles a splice, so the whole
track would stay unsupported.

### 4. Raw entries

`parse_elst` keeps every entry of every edit list as stored, in
`Track::edits`: segment duration, media time and rate, for supported and
unsupported lists alike. `coreav probe` prints them in the native output
next to the classification. Each entry costs 24 bytes of
`max_retained_bytes`, so a list larger than the budget is a limit error, as
for other retained tables.

### 5. Duration

The track's duration (what the edit list presents) is the sum
of the media edits' durations, capped by the media header. Empty edits are
not counted, as for the simple model's leading empty edit (KD-EDIT-DURATION
covers `ffprobe` counting them).

The track's span on the movie timeline (`Track::presentation`, from which
the movie's presentation duration follows) runs from the end of the
leading empty edits to the end of the last media edit. The empty edits
between media edits are inside it. Any cap the media header puts on the
duration comes off the end. Rejected alternative: the start plus the
duration, which ends the stream early by every empty edit between media
edits.

### 6. Writers

These all refused `EditList::Multiple` when this note was written, as they
refuse `EditList::Unsupported`, and each takes it on only with its own
oracle check:
- clip;
- split;
- the HLS writers;
- M4A extraction;
- `coreav-text` timing.

`docs/design/edit-list-writers.md` is that step and says where each one
stands today. Remux copies the edit boxes as bytes and is unaffected.

## What `ffprobe` lists (black box, ffprobe 7.0.2)

Measured by comparing the packets `ffprobe -show_packets` lists with a
model computed from the sample tables and the edit list. The fixtures are
files written by `ffmpeg` with the edit list replaced by
`scripts/elst_craft.py`:
- H.264 without B-frames, with an all-zero `ctts` box and without one;
- H.264 with B-frames;
- all-intra H.264;
- AAC at 44.1 and 48 kHz, Opus and MP3.

The model reproduces `ffprobe` packet for packet on:
- every crafted case: splices, gaps, cuts inside a group of pictures, empty
  edits at the start and in the middle, overlapping and backward windows,
  and durations of fractional ticks;
- the multi-edit tracks of six of the seven corpus files (720,024
  `ffprobe` packets; 1ae48e7b, 5de2646e and ceb55bb5 read with signed
  composition offsets).

The seventh, dd6c5629, has one packet: a non-fragmented track with an
empty edit and a media edit of duration 0, which is outside this model.

For a track with samples in decode order (composition time `c`, decode
time `d`, duration, sync flag), media timescale `T` and movie timescale `M`:

1. **Durations.** An entry's duration in media ticks is
   `segment_duration × T / M`, rounded to the nearest tick, halves up.
   Entry k starts at the sum of the rounded durations before it.
2. **Empty edits.** Empty edits before the first media edit only move the
   start. An empty edit after a media edit is listed as a media edit at
   media time −1: `ffprobe` presents media during it.
3. **The edit's window and samples.** Each media edit presents the window
   `[media_time, media_time + duration)`.
   - `F` is the first sample in decode order with `c ≥ media_time`.
   - A sample is presented when `c` lies in the window. In the first media
     edit of an audio track, it is presented when its interval overlaps
     the window instead.
4. **Times in the first media edit.** Times are shifted so that
   `snap = min{c ≥ media_time}` lands on the edit's start. For audio,
   `media_time` itself lands there, with no snap (as KD-EDIT-SNAP
   records). Let `C = d(F) − snap`.
5. **Times in later media edits.** Times are shifted so that `d(F)` lands
   on the edit's start plus `C`. That is, the decode time of the edit's
   first sample, not its media time, is anchored.
6. **Decode times.** When the smallest composition offset is negative,
   `ffprobe` moves every decode time back by it (KD-CTTS).
7. **Preroll.** Each media edit is listed from a preroll sample, not
   presented:
   - for video, the last sync sample at or before `F`;
   - for audio, the sample shown at `media_time − T`, one second earlier,
     or the first sample.
8. **End of an edit's listing.** Let `L` be the edit's last presented
   sample in decode order. The listing goes on to the first sync sample at
   or after `L`, or to the second one when the track has a `ctts` box,
   even an all-zero one. The samples after `L` are listed as not
   presented.
9. **Repeats.** Each media edit's listing is appended in turn, so a sample
   can be listed several times: once at the end of one edit, and again in
   the next edit's preroll or presented samples.

CoreAV differs from this in three ways, and each one is a known divergence
(KD-EDIT-MULTI):
- the anchoring and rounding of rules 1, 4 and 5;
- rule 2's treatment of a middle empty edit;
- the repeats of rules 7 to 9.

The overlay covers a stream only when every reference packet of it equals
the model's listing. It recomputes that listing from:
- CoreAV's raw media times and sync flags;
- the edit list, timescales, handler and `ctts` presence, read from the
  file.

It also requires CoreAV's own packets to equal decision 2's mapping.

## Oracle coverage and pass criteria

These criteria are fixed before the corpus run:
- **Fixtures.** The fixtures (`scripts/gen-fixtures.sh`, `edit_multi_*`)
  agree with `ffprobe` directly or through KD-EDIT-MULTI, at 100%.
- **Corpus.** On the stage 3 corpus (baseline a213539, rechecking the
  uncovered files), the multi-edit streams of 6e67814d, a166d099,
  ea690039, 1ae48e7b, 5de2646e and ceb55bb5 agree at every packet. The
  qt-brand three depend on an earlier change's signed composition offsets.
- **No regressions.** No file that agreed before disagrees after.
- **Speed.** Probe and packet listing keep their bench-guard bars. Finding
  a sample's edit is a binary search over the windows, `O(log n)` per
  sample.

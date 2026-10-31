# Writers for sources with several edits

Status: Proposed.

`docs/design/edit-lists.md` applies edit lists with several media edits
exactly in probe and packet listing (`EditList::Multiple`, decision 1).
Its decision 6 keeps every writer refusing them until each is checked
against its own oracle. This note is that step. It has one section per
writer, in the order the writers are done:
1. clip;
2. split;
3. the HLS writers;
4. M4A extraction;
5. `coreav-text` timing;
6. join.

Each section, with its pass criteria, is committed before that writer's
measurement run. Each writer is one pull request.

## Common decisions

### 1. What stays refused

A writer never writes a file whose timing it cannot state exactly. Each of
these cases fails with an unsupported-feature error (`unsupported: ` and
the message below) and writes nothing; a test per writer checks each case.

| Case | Writers | Message |
|---|---|---|
| An edit list outside the edit model (`EditList::Unsupported`): overlapping or backward windows, a rate other than 1, a dwell, a media time below −1, a zero-length media edit in a movie without fragments | all | `edit list outside the supported model` (`coreav-text`: `an edit list outside the supported model`) |
| Several edits in a movie with fragments (classified `Unsupported` by probe, `docs/design/edit-lists.md` decision 1) | all | `edit list outside the supported model` |
| Several edits in a writer that has not taken them on yet | the HLS writers, which their section leaves refusing | `edit list with several edits (multi-edit), not supported here yet` |
| Several edits whose kept durations need an output movie timescale above 32 bits | clip and split, and any later writer that chooses an output movie timescale; extraction keeps the source's, so it cannot reach this | `edit list whose edit durations need a movie timescale above 32 bits` |
| Several edits with more than 4,096 empty edits in a row (the windows are read from the track's own entries, clip decision 7; a longer run would make one lookup walk without bound) | clip and split, and any later writer that looks a window up per sample; extraction writes the entries once and looks none up | `edit list with a long run of empty edits` |

The writer that refuses is the first to look at the track's edit list, so
the error comes before any byte is written; the test also checks that the
output is empty.

### 2. Budget

The per-iteration list of an edit list's windows (`Span` in
`coreav-isobmff`'s packet listing, about 80 bytes per media edit) is
charged to the listing's budget. Today only the stored entries are charged
(24 bytes each, `max_retained_bytes`); the windows are bounded by them but
not counted. Rejected alternative: leaving the windows uncounted because
the entries are, which lets a list near the retained-bytes limit about
quadruple its real memory use.

### 3. How the oracles judge timing

The writers' oracles compare an output's packets and decoded frames with
the source's, at the source's presentation times minus the output's start.
For a source or output with several edits, `ffprobe` and `ffmpeg` apply
their own edit-list model, which differs from ISO/IEC 14496-12 8.6.6
(`docs/design/edit-lists.md`, "What `ffprobe` lists"). It anchors later
edits on decode times, rounds durations to whole ticks and repeats samples
at edit boundaries. Comparing through that model would need new known
divergences in every writer's oracle, and the set is frozen.

Instead, for any file with several edits, the oracle:
- reads the samples and decoded frames with ffmpeg ignoring edit lists
  (`-ignore_editlist 1`), which lists each sample once at its stored
  decode and composition times (measured: 300 packets and 300 frames for a
  300-sample track whose edits `ffprobe` otherwise lists as 272 packets);
- applies the edit list itself, from the file's own `elst`, `mdhd` and
  `mvhd` boxes, by ISO/IEC 14496-12 8.6.6, to find which frames are
  presented and when.

The black boxes still supply everything the comparison is about: frame
contents, packet hashes, stream parameters and MediaInfo's view. Only the
edit arithmetic is done from the specification. The harness reads `elst`,
`mdhd` and `mvhd` with its own box reader and maps media time to
presentation time from ISO/IEC 14496-12 8.6.6, written in the oracle
script, not imported from CoreAV or its overlay. Files with no edit list or
a simple one keep today's checks unchanged.

**A second view, reported and not gating.** The edit arithmetic in the
gating view is the harness's own. A misreading that CoreAV and the harness
shared would pass it. So each writer's oracle also compares the output
with ffmpeg's normal output, edit lists applied (without
`-ignore_editlist`): the source's and the output's decoded frames (video)
or samples (audio), matched by content and order, with the times
ffmpeg gives them. The report gives the agreement for every file. Where
ffmpeg's applied output differs from the specification's mapping, the
report names the difference, and this note records the reason when it is
one of the measured rules of `docs/design/edit-lists.md` ("What `ffprobe`
lists"):
- times anchored on decode times;
- durations rounded to whole ticks;
- samples repeated at edit boundaries;
- an empty edit between media edits listed as media.

The report tells which apply by predicting each file's edit-applied
frames from its stored samples with those rules (1 to 5 of that section),
and naming the rules in play when both files' views follow the
prediction. One more measured reason sits outside the edit model: when
the decoder emits a frame before frames presented earlier, ffmpeg's
output moves their times up to that frame's ("Non-monotonic DTS ...
changing to"), so their own times are lost. The view takes such frames as
following the prediction when it presents their content no later than
the time they were moved to. (Measured on ea690039d902: in the
edit-applied decode, the frame at 3204 ticks comes before the eleven
presented from 3112 to 3197, and all twelve are written at 3204; decoded
with edit lists ignored, they come in presentation order.)

A difference that none of them explains is investigated before the
writer's pull request merges.

Rejected alternatives:
- **Comparing through ffmpeg's model with new divergence entries.** The
  set is frozen, and an entry per writer would encode ffmpeg's quirks
  rather than check the output.
- **Re-encoding to remove the edits.** No writer re-encodes.

## Clip

### Decisions

1. **The range.** As for the clip writer, the range
   `[start, end)` is on the source's presentation timeline: for several edits, CoreAV's exact
   mapping (`docs/design/edit-lists.md`, decision 2). A sample is
   presented when an edit shows it.
2. **The snapped start.** It is the clip writer's rule, with
   "presented" and "presentation time" from that mapping: the latest video sync sample
   presented at or before `start` such that every sample before it in
   decode order is presented before it. As the clip writer already does for
   a simple edit that trims the start, a sync sample that no edit shows can
   start the clip, which then starts where that sample's window begins. A
   sample in a gap belongs to the next window, as in `docs/design/
   edit-lists.md` decision 2, so a clip can start at the sync sample just
   before a splice and present from the splice on.
3. **The kept run.** As in the clip writer: the decode-order
   run from the
   snapped sync sample to the last sample presented before `end`. Samples
   inside the run that no edit shows, such as a skipped part between two
   windows, are copied so the run decodes, and the output's edits leave
   them unpresented. This is what a decoder of the source does as well: it
   decodes in decode order and presents what the edits show.
4. **The output edit list.**
   - Each track gets one media edit per source media edit that shows part
     of the clip, in source order. Empty edits between them are kept as
     empty edits.
   - Each media edit's media time is where the clip's part of that window
     starts, in the output's media time: the source composition time minus
     the run's first decode time, as the output's decode times start at 0.
   - Each edit's duration is the window's part inside `[snapped start,
     end)`.
   - The first edit starts at the snapped sync sample, as the clip writer's
     one edit does. The last is cut at `end`, rounded up to a whole tick as
     the clip writer rounds it, or where its window ends when that is first,
     which need not be a whole video tick (a 1,000 Hz movie with a
     12,800 Hz video, say). The clip reports where its edits end, exactly;
     the last edit is not rounded up past its window, which would show
     media the source does not.
   - An `end` inside a gap between two windows (an empty edit, or the
     space a skipped part of the media leaves) is moved back to where the
     earlier window ends, and the clip reports that end: an output never
     ends in an empty edit. Rejected: a trailing empty edit, which would
     present nothing for its duration.
   - Where a track's presentation starts inside a gap (audio whose first
     window in the clip starts after the clip's start), its edits begin
     with an empty edit for that gap. Audio that no window shows inside the
     clip is left out (`no_samples_in_clip`), as the clip writer leaves out
     audio with no sample in the clip.
   - A source with a single window inside the clip gives exactly the clip
     writer's one edit.
5. **The movie timescale.**
   - The clip writer sets the movie timescale to the video's media
     timescale, so the video edit is exact. Here a middle edit's duration must also
     be whole movie ticks, or every later frame would move by a fraction
     of a tick.
   - The movie timescale is the smallest multiple of the video timescale
     in which every kept window's duration is whole.
   - Every such duration is a whole multiple of 1 / (source movie
     timescale) seconds, so a multiple exists. When it does not fit in 32
     bits, the clip fails with that reason.
   - Where the audio's timescale also divides it within 32 bits, it is a
     multiple of both; else of the video's alone, the audio edits rounded.
   - Audio edits are converted to it, rounded to the nearest tick where
     the audio timescale makes them inexact, as the clip writer does for
     audio. Each edit ends at its exact end rounded, so that rounding
     does not add up over many edits, and an edit that rounds to 0 ticks
     is left out (a media edit of duration 0 is outside the edit model).
6. **Audio.**
   - An audio track with several edits is clipped the same way: its kept
     run is the decode-order run from the sample presented at the snapped
     start, minus the clip writer's one sample of pre-roll, to the sample
     presented at `end`.
   - Its output edits are its windows inside the clip. The AAC frames that
     straddle a window boundary are decoded continuously, as in the
     source, and the edits present the same samples.
   - An audio track with a single window inside the clip gets the clip
     writer's audio edit.
7. **Memory.** The windows are not copied: the timeline reads them from
   the track's edit list entries, which the parser keeps, with a
   checkpoint every 256 windows and the window of the latest lookup, from
   which a lookup for a nearby time (samples come in decode order) walks a
   few entries. So the clip's memory stays within the clip writer's memory
   bound whatever the source's edit count. Measured: the windows as a table
   (24 bytes per source edit) put a166d09989f9 (6,069 audio edits) 113,673
   bytes over the bound on its whole clip and 141,688 on a 10-second clip;
   without it, every clip of that file is within it. Rejected: the table,
   and releasing it after the edits are written (the peak is the second
   listing, before that). A run of more than 4,096 empty edits in a row is
   refused (common decision 1), so that no lookup walks further.
8. **Everything else** in the clip writer's design is unchanged: codecs,
   track selection, durations, layout, limits, and its refusals other than
   the edit model.

### Oracle

`scripts/clip_check.py` applies common decision 3 to a source or output
with several edits:
- **Packets:** for each output stream, `ffprobe -show_packets
  -show_data_hash -ignore_editlist 1` of the output equals the source's
  kept run, by hash, size, keyframe flag and stored times shifted by the
  run's first decode time.
- **Presented frames:** the frames that the output's edits present, by
  ISO/IEC 14496-12 from its boxes, equal the frames the source's edits
  present inside the clip, at the same times minus the snapped start.
  Both are decoded with `ffmpeg -ignore_editlist 1 -copyts`.
- **Audio:** the decoded audio the output's edits present equals the
  source's inside the clip, within one sample, as today.
- **Stream and MediaInfo checks:** unchanged. Durations are the output's
  edits capped by its media durations.
- **Second view (reported):** ffmpeg's edit-applied frames of the output
  against the source's from the snapped start, by content and time, named
  as in common decision 3; for audio, the edit-applied samples as in the
  existing audio check.

The fixture with audio in several edits (`edit_multi_av.mp4`) is encoded
without perceptual noise substitution. Its noise is not defined by the
bitstream (ISO/IEC 14496-3), so audio decoded from the clip's restart
would be about 1e-5 off from the source's (measured), which the existing
clip oracle records as `KD-C-AUDIO-RESTART` for a single edit. The fixture
bar above admits no known divergence.

### Pass criteria

Fixed here, before the run:
- **Fixtures, 100%, directly.** Clips cut from the edit-list fixtures of
  `scripts/gen-fixtures.sh` (`edit_multi_*`, `edit_trim_audio.mp4`) pass
  every clip oracle above, with no known divergence. Covered ranges: a
  range inside one window, across a splice, across a gap, across an empty
  edit between media edits, and an audio track with several edits. The
  existing clip fixtures keep their results.
- **Corpus, 100%.** Under the clip protocol of `docs/spikes/clip-bench/` (head,
  middle, tail and whole clips; `--audio none` where the audio is not AAC),
  every clip of the corpus files with several edits passes every clip
  oracle:
  - 6e67814d336c, a166d09989f9, ea690039d902, 1ae48e7b157e, 5de2646e4ea0
    and ceb55bb5fb87 become accepted;
  - every other corpus file keeps its clip result.
- **Second view (reported).** The agreement with ffmpeg's edit-applied
  frames and samples, per clip, with each difference named and explained as
  in common decision 3.
- **Refusals.** Each case of common decision 1 that applies to clip refuses
  with its message and writes nothing (a test).
- **Speed: at most 1.00.** The median per-clip ratio, CoreAV over
  libavformat in-process (the clip writer's baseline), must be at most
  1.00:
  - in a class of clips from those six files. Only two of them are held
    out (6e67814d336c and ea690039d902), at most 8 clips, so this class
    uses all six files' clips; the held-out ones are also reported apart;
  - in each existing class, which must also not get worse: its median at
    most the upper end of its 95% interval in `docs/spikes/clip-bench/`'s
    `perf-06` (the writer before several edits);
  - over all held-out clips.

  Two runs on the same cloud shape as `perf-06`, both with this writer:
  every held-out file (the existing classes and the held-out bar), and
  the six files' clips, held out and not (`perf.py --both-splits` on
  `evidence/inputs-multi-edit.json`; class `several edits`, its held-out
  part `several edits, held out`).

  This is a bar, not a claim. A claim that CoreAV is faster than FFmpeg on
  sources with several edits could cite only the held-out clips, and
  needs the upper bound of their confidence interval below 1.00. With two held-out files, at most 8 clips, no such claim is
  expected: the class reports its median and interval, and the speed
  claims stay those of the existing classes.

  The CI benchmark guard gains a clip of `edit_multi_splice.mp4`.
- **Memory:** the clip writer's memory bound,
  checked on the six files' whole clips.

### Results

In `docs/spikes/clip-bench/`, "Sources with several edits". Against
the bars as registered above:
- correctness on the fixtures and the six files: met;
- speed: met in the several-edit class, in every class and over the
  held-out clips;
- "not worse than `perf-06`": **not met** in any existing class. This
  writer against `main` on one machine gives a median ratio of 1.005; the
  difference is in code already on `main` (measured) and
  in `perf-09`'s faster machine;
- memory: met on the six files' whole clips with the windows read from the
  track's entries (decision 7; `perf-me-06`).

## Split

A split's segments partition the source: every sample in exactly one
segment, every frame the source presents presented by exactly one segment
at its source time minus the segment's start, and the segments' intervals
tiling the source's presentation (`docs/design/split.md`). The clip writer
supplies the tables, the file head and the copy pass, so a segment's edits
are built as a clip's are (clip decisions 4 to 6); these decisions are
what a split adds.

### Decisions

1. **When a sample is shown, and which samples order the cuts.** A clean
   cut point is a sync sample that every *presented* sample before it in
   decode order is shown before, and every presented one after it after.
   Two things follow from the window mapping
   (`docs/design/edit-lists.md`, decision 2), which gives every sample a
   time whether or not a window shows it:
   - **Unpresented samples take no part in the ordering**, as they do not
     in a clip's start search. A sample a gap skips is given the time its
     window would show it at, which can lie before samples decoded
     earlier: at a splice that skips a second of media, the first skipped
     sample is placed a second before the last shown one.
   - **A sample is shown from its window's start, not from where the
     mapping places its media.** A sample whose media begins in a gap and
     reaches into the next window is presented, but only its part inside
     the window is, so it appears at the window's start, later than the
     mapping places it. Taking the mapping instead would put the segment
     that begins with such a sample inside the window before it: its
     first edit would ask for media the segment does not hold (a negative
     media time, which fails while the segment's head is written), and
     the segment before it would have its last window cut short. A
     sample's place in the order is therefore the later of the two, which
     is what a clip's start already takes for the sample it snaps to; its
     end stays where its media ends.

   A cut point is a presented sample and is placed at or after its
   window's start, so the segments on both sides of it have an edit that
   reaches it. Rejected: cutting at a sync sample no edit presents, which
   would start a segment where the source shows nothing.

   Without several edits the second rule is not applied at all. The
   first can in principle move a cut either way: a sync sample no edit
   presents was already dropped by the filter for candidates strictly
   inside the video's presentation, but one whose place an earlier,
   unpresented sample's time used to take can now become a cut point.
   Measured: every corpus file the writer took before gives the same
   segments, byte for byte (the corpus bar below).
2. **A segment's edits.** Segment `i` presents `[zero_i, end_i)` of the
   source's presentation timeline and gets one media edit per source
   window that overlaps it, in source order, with the empty edits between
   them kept: clip decision 4's edits, with the clip's range replaced by
   the segment's. Its last edit is **not** rounded up to a whole video
   tick as a clip's is: the segments tile the
   source's presentation, so a segment must not present past where the
   next one starts, and a window boundary need not fall on a whole video
   tick.
3. **A gap at a cut.** Where the cut sample is the first of a window, the
   cut comes after the gap the previous window left. The gap goes to the
   segment that follows: the earlier segment ends where its last window
   does, and the later one starts there, its video after an empty edit for
   the gap, as the first segment's video starts after an empty edit when
   the audio starts earlier. The segments then still tile the source's
   presentation, each starting where the previous one ends, and the
   source's presentation and the segments' durations add up to the same.
   Rejected: a trailing empty edit on the earlier segment (clip decision
   4: an output never ends in an empty edit); and leaving the gap out of
   every segment, which would make the segments shorter than the source
   together and lose the gap on a concatenation.
4. **The last segment's end.** The earliest of the last window's end and
   the end of the last presented sample, moved back to where the earlier
   window ends when it falls in a gap (clip decision 4). With several
   edits the windows' end is where the stream ends, so the media header's
   duration does not cut it short (`docs/design/edit-lists.md`, decision
   5), as a clip to the end already has it.
5. **Audio.** Each audio sample still goes to the segment in which its
   presentation starts, the segments being the ranges decision 3 gives
   them: a sample presented in the gap before a cut belongs to the
   segment the gap belongs to, the one after the cut, which already pads
   its video with an empty edit for that gap. Taking the video's start as
   the boundary instead would leave that audio in the segment before
   while the segment after still counts the gap, and the two together
   would last longer than the source — which happens exactly when the
   audio has no gap of its own there. Each segment's audio edits are its
   windows inside its range, rounded to the movie timescale as clip
   decision 5 rounds them. A sample that straddles a window's end is
   presented past it while the window that follows starts there, so a
   segment's audio ends no later than the next segment's audio starts: no
   audio is presented twice. Without several edits the two meet exactly,
   the samples being contiguous.

   **A sample two windows show.** Where the source pauses with its media
   running on, the sample at the boundary is shown by both windows
   (`docs/design/edit-lists.md`, decision 3; every frame of the
   6,069-edit audio track of the corpus does). A split copies each
   sample into one segment and adds no pre-roll (`docs/design/split.md`
   rejects it), so only the segment that holds the sample can show it,
   and only as far as its own range reaches: a segment's audio runs from
   the first of its samples that is shown to where the first showing of
   the last of them ends, and presents every source window inside that —
   a window that shows one of its samples a second time included, where
   that window falls in the range. What the segments may therefore miss
   of the
   window that resumes is that sample's part in it — shown whole where
   one segment holds both showings, from the next sample on where a cut
   separates them, with a hole inside the window where the showing
   reaches it and the next sample is in another segment, or not at all
   where the whole window lies inside the sample. Which of these falls
   out depends on where the cuts land; in every case it is that one
   sample's part in the window, about 21 ms of AAC, and no more. Every
   sample is still in exactly one segment at its source time, and a join
   of the segments gives the source's samples back, so nothing is lost
   across a split and a join; what a segment on its own does not show is
   part of a sample another segment holds. Rejected: showing both from
   the segment that holds the sample, which would make its audio outlast
   its video by the whole gap and the segments overlap; copying the
   sample into both segments, which `docs/design/split.md` rejects (a
   join would have to find and drop the copies); and refusing such
   sources, which the clip writer takes and which the corpus's longest
   file with several edits is.

6. **The movie timescale.** Per segment, clip decision 5's smallest
   multiple of the video's timescale in which every one of that segment's
   edits is whole; a multiple of the audio's too where it fits. Above 32
   bits the split is refused, and the refusal is made while the source is
   listed, before the first writer is opened, as every other split refusal
   is (`docs/design/split.md`, "Errors"). It depends on the video
   timescale and the windows only, so one pass over the plans decides it.
7. **Memory and work.** One timeline, with the windows read from the
   track's own entries (clip decision 7), serves every segment: a
   segment's plan holds a timeline without them, and the shared one is
   put into the segment being written, so only that one is alive at a
   time. Neither the number of segments nor the source's edit count adds
   to the bound of `docs/design/split.md`, "Memory and work"; a
   checkpoint table per segment would have made their product grow. The
   work is bounded too, though it is not charged to `max_sample_work`,
   which counts the sample listings: each lookup starts from the
   checkpoint before it and walks at most 512 windows (clip decision 7's
   256 per checkpoint, two back), a run of more than 4,096 empty edits is
   refused, the entries are bounded by `max_box_bytes` and the segments by
   `max_table_bytes`.
8. **Everything else** in `docs/design/split.md` is unchanged: where it
   cuts, what each segment holds, the audio without pre-roll, the sync
   flags, the limits and the other refusals.

### Oracle

`scripts/split_check.py` applies common decision 3 to a split whose source
or whose segments have several edits:
- **Packets:** `ffprobe -show_packets -show_data_hash -ignore_editlist 1`
  of the segments, taken in segment order, lists the source's stored
  samples one for one, by hash, size, keyframe flag and stored times
  shifted by the segment's run's first decode time. A source sample no
  segment holds, one two segments hold, or one out of order is a failure,
  as without several edits.
- **Presented frames:** the frames each segment's edits present, by
  ISO/IEC 14496-12 from its own boxes, shifted by the segment's start, are
  compared with the frames the source's edits present: the segments show
  nothing the source does not show at that time, and show every frame the
  source shows *first* before the last segment's end. A frame two windows
  show may be shown once or twice, by decision 5. The last segment's end
  comes from CoreAV's own summary, so the comparison also fails where the
  source's edits first show anything after it. Both are decoded with
  `ffmpeg -ignore_editlist 1 -copyts`.
- **Audio:** per segment, the decoded audio from its second frame on
  equals the source's run (both with edit lists ignored; a segment has no
  pre-roll, so its first frame follows a decoder restart). The restart is
  bit-exact only where the bitstream defines every band:
  `KD-S-AUDIO-RESTART` covers a segment whose audio has one media edit,
  and no entry covers one with several, so `edit_multi_av.mp4` is encoded
  without perceptual noise substitution as the clip fixture is. A corpus
  file with several audio edits and that substitution would report the
  difference; none of the six does (five have no audio or are over the
  decode limit, and the sixth's audio has one edit). Each segment's edits
  present the media a source edit presents at the same time, within one
  tick, and no two segments present overlapping parts of the source's
  timeline. Over the whole split, the segments present nothing the
  source's edits do not, and no span of the source's presentation is
  short by more than decision 5 allows it: the part, in a window that
  starts inside a sample the window before it showed, of that sample —
  and nothing at all at a boundary that runs on in both media and
  presentation, where the sample is shown once without a break. Where
  the missed parts fall depends on the cuts, so the comparison bounds
  them rather than placing them, and bounds them inside the span they
  belong to, so one span's allowance cannot pay for a loss in another.
  It is conservative at a boundary that breaks in the media alone: the
  segment that holds the sample does show the window that resumes
  there, so the allowance it is granted is slack.
- **Durations:** each segment's track header lasts its edits, and the
  video's edits less its leading empty one run from its video start to its
  end. Away from the last segment, the segment's movie lasts its video
  plus at most one audio frame (`docs/design/split.md`), so the segments
  and the source's presentation add up to the same: a gap counted both as
  audio in the segment before it and as the empty edit of the segment it
  belongs to would show here. `ffprobe`'s own duration and start time
  follow its edit model, which common decision 3 keeps out of the gating
  view.
- **Streams and MediaInfo:** unchanged.
- **Second view (reported):** per segment, ffmpeg's edit-applied frames
  against the source's from the segment's start, each difference named as
  in common decision 3; for audio, the edit-applied samples as the
  single-edit check compares them, where the decoder restart of
  `KD-S-AUDIO-RESTART` also shows. Each one decodes a segment again and
  compares it with the whole source, and a split of a long file has
  hundreds of segments, so the view is reported for a sample of them: the
  first, the last and every segment whose own edit list has several
  edits, at most 8 in segment order, with the count reported. A
  misreading of the edit model shared by CoreAV and the harness is the
  same in every segment, so a sample shows it.

No new known divergence is expected: the gating view is the
specification's, and the fixture bar below admits none.

### Pass criteria

Fixed here, before the run:
- **Fixtures, 100%, directly.** Every `edit_multi_*` fixture with a video
  track (`edit_multi_splice.mp4`, `edit_multi_bframes.mp4`,
  `edit_multi_empty.mp4`, `edit_multi_av.mp4`), split every second and at
  0.5 s and 1.3 s, passes the CoreAV round trip of
  `crates/coreav-isobmff-write/tests/split.rs` and every oracle above,
  with no known divergence. Covered: a splice on a cut, a segment across a
  splice, a gap between two windows at a cut and inside a segment, and
  audio with several edits. The four fixtures join the list the CI job
  requires. Every existing fixture keeps its result, and its segments stay
  byte for byte the same.
- **Corpus, 100%.** Under the split protocol of `docs/spikes/split-bench/`
  (`--every 10`, the default limits, allowlist and audio choice; without
  audio where the audio is not AAC), every split of the corpus files with
  several edits passes the round trip and every oracle above:
  - 1ae48e7b157e, 5de2646e4ea0, 6e67814d336c, a166d09989f9, ceb55bb5fb87
    and ea690039d902 become accepted, with every packet of the kept
    streams in exactly one segment;
  - every other corpus file keeps its split result. The instrument, fixed
    here before the run, is byte identity rather than a second oracle
    pass: every corpus file of `docs/spikes/clip-bench/evidence/inputs.json`
    without several edits must give the same segments, byte for byte, from
    this writer and from `main`, and the same refusal where `main` refuses. Equal bytes give the earlier run's
    oracle result by construction, and the comparison covers all 869 files
    rather than the subset an oracle pass could afford.
- **Split then join.** The segments of a source with several edits, joined
  back, give the source's samples byte for byte with their decode and
  composition times, their sync flags and their places on the presentation
  timeline, wherever every segment is joinable: the join takes one edit per
  track (`docs/design/join.md`), so a segment that carries several windows
  is not an input to it, and such a split is reported as not joinable
  rather than failing. Checked on a synthetic spliced source and over the
  fixtures.
- **Refusals.** Each case of common decision 1 that applies to split
  refuses with its message and opens no writer (a test).
- **Speed: at most 1.00.** The median per-file ratio, `coreav split
  --every 10` over `ffmpeg -f segment` (the protocol of
  `docs/design/split.md`, "Speed against FFmpeg"), must be at most 1.00:
  - in the class `several edits`, the six files above
    (`perf.py --all` on `docs/spikes/clip-bench/evidence/inputs-multi-edit.json`;
    only two of the six are held out, so the class uses all six);
  - in its held-out part, `several edits, held out`: 6e67814d336c and
    ea690039d902, which no earlier split run timed (both were refused in
    `perf-01` and `perf-03`), so they have not been used for split.

  The share of the inputs this writer is faster than FFmpeg on is reported
  with each median.
- **Not worse than `main` on sources without several edits.** This writer
  against `main` built from the commit it branches from, both timed in the
  same run on the same machine with the builds alternated, as "Speed
  comparisons for the writers that follow" requires. Subset, fixed here before the run: the
  20 non-held-out inputs of `docs/spikes/clip-bench/evidence/inputs.json` of at
  most 150 MB with the lowest SHA-256 that the split accepts, split
  `--every 10`, one warm-up of each build and then 5 rounds with the build
  order alternated, a build's time the median of its rounds. Bar: the
  median per-file ratio of this writer over `main` at most 1.00, with its
  95% bootstrap interval reported. The existing classes' ratios against
  FFmpeg keep `perf-03`'s result (`docs/design/split.md`): the same
  held-out files, and this comparison is the instrument for a regression
  in them.

  This is a bar, not a claim. A claim that CoreAV is faster than FFmpeg on
  sources with several edits could cite only the held-out files, and
  needs the upper bound of their confidence interval below
  1.00; with two files none is expected.

  The CI benchmark guard gains a split of `edit_multi_splice.mp4`
  (`split-multi-edit`).
- **Memory.** The bound of `docs/design/split.md` (the largest segment's
  `moov` box plus 24 bytes per source sync sample plus 2 MiB above
  `coreav probe`) on each of the six files, and the live heap of
  `split_mp4` reported beside it.

### Results

In `docs/spikes/split-bench/`, "Sources with several edits". Against
the bars as registered above, every one is met:
- fixtures: 8 of 8 multi-edit splits directly, no known divergence; all
  128 fixture splits pass, and every fixture without several edits gives
  the same segments byte for byte;
- corpus: 6 of 6 files accepted, 634,122 of 634,122 packets in exactly
  one segment in order (`check-me-15`); 824 of 824 other files byte for
  byte as `main` writes them, 38 refused by both, over 868 of the 869
  inputs (one object is no longer in the bucket) (`same-me-06`);
- split then join: the segments of a spliced source join back into its
  samples and its two windows;
- speed: class `several edits` 0.180 (95% CI 0.096 to 0.210), its
  held-out part 0.096 (0.035 to 0.158), every file faster than FFmpeg
  (`perf-me-06`);
- not worse than `main`: 0.992, 0.991 and 0.990 over three runs
  (`speed-me-08` to `speed-me-10`), with the segments of all 824 corpus
  files without several edits byte for byte `main`'s;
- memory: 6 of 6 within the bound, the live heap reported beside it
  (`mem-me-06`).

## The HLS writers

A clip is a plain MP4: it carries the source's media edits as its own edit
list, so a sample a window needs but does not present — the frames of a
group of pictures before an edit's start, the AAC frame a splice cuts — is
copied and left unpresented. An HLS rendition is a movie with fragments,
and there a stream copy cannot do the same. The writers keep refusing
sources with several media edits; this section records why, what was
measured, and the design that would lift the refusal.

### What was measured

Results: `docs/spikes/hls-bench/several-edits/`.

**1. A reader of a fragmented rendition sees at most the first media
edit.** The pinned ffmpeg (7.0.2) was given one of this writer's own init
segments with its edit list replaced
(written from ISO/IEC
14496-12 8.6.5 and 8.6.6), followed by its media segments. The source has
300 samples at composition times 0 to 5,980 in a 600 Hz media timescale,
and the init segment's movie timescale is that media timescale
(`docs/design/hls.md`, "Boxes"), so a `segment_duration` below is in
600 Hz ticks too:

| Edit list in the init segment | `ffprobe -show_packets` |
|---|---|
| no entry (an `elst` of count 0) | 300 packets, 0 to 5,980 |
| `3000:0` then `3000:2400` (two media edits) | 300 packets, 0 to 5,980 |
| `3000:600` then `1800:2400` | 300 packets, **−600 to 5,380** |
| `5400:600` (that first edit alone) | 300 packets, −600 to 5,380 — the same |
| `1000:-1`, `3000:0`, `1800:2400` | 300 packets, **1,000 to 6,980** |
| `1000:-1` then `5400:0` (that empty edit and one media edit) | 300 packets, 1,000 to 6,980 — the same |
| `2400:0` (one media edit, shorter than the media) | 300 packets, 0 to 5,980 |

So the reader applies the leading empty edit and the first media edit's
media time, and ignores every `segment_duration` and every entry after the
first. The writer's single media edit works for what it is used for today
— hiding encoder priming, by its media time — and a list of several would
play the material the edits cut out, at the first edit's offset. This is
what the pinned ffmpeg does, not what the format says: ISO/IEC 14496-12
8.6.6 puts no such limit on a movie with fragments, and
`docs/design/edit-lists.md` decision 1 leaves such lists unsupported for
want of exactly this measurement.

**2. With one media edit, a sample between two presented ones has nowhere
to go.** The output's composition time of a presented sample is fixed: it
is the sample's presentation time. One media edit hides only what lies
before it starts or after it ends, so a sample between two presented ones
must be moved to one of those places, by a composition offset as large as
the distance. The spike's placement script
moves one sample of one media segment and reads the rendition back; on the same
300-sample package, with the sample in the second of its two segments:

| Sample's composition offset | `ffprobe` packets | `ffmpeg` decoded frames |
|---|---|---|
| as written | 300, 0 to 5,980 | 300, none moved |
| **−3,500** (before the start) | 300, **3,500 to 9,480** | **180 of 300**; with the oracle's own time base, 120 of 300 moved by the muxer |
| **+3,500** (after the end) | 300, 0 to 5,980 | **181 of 300**; 119 of 300 moved |
| −1 | 300, 1 to 5,981 | 300, none moved |
| +1 | 300, 0 to 5,980 | 300, none moved |

The shift of every packet in the −3,500 row is KD-HLS-1, which
`docs/known-divergences-hls.md` already forgives and whose predicate reads
the rendition's own offsets, so that symptom alone would pass the HLS
oracle. The one that disqualifies both placements is the frames: a third
of them are lost, or moved to the hidden sample's time, as soon as the
offset is large enough to reach outside the edit. Keeping the sample's own
time is not a third option: that time lies inside the previous media
edit's presented interval, where the output would show it.

**3. How many such samples there are.** Per stream, the samples no media
edit presents between the first and the last presented one, split by what
a writer could do with each: left out because a
sync sample follows; placeable because its composition time is already
outside the presented interval; disposable because no picture refers to it
(H.264 `nal_ref_idc` 0); and the rest, which must be hidden.

| Stream | media edits | samples | presented | left out | placeable | disposable | must be hidden |
|---|---|---|---|---|---|---|---|
| `edit_multi_splice`, video | 2 | 300 | 180 | 30 | 0 | 0 | **0** |
| `edit_multi_bframes`, video | 2 | 300 | 180 | 30 | 1 | 1 | **2** |
| `edit_multi_empty`, video | 2 | 300 | 270 | 0 | 0 | 0 | **6** |
| `edit_multi_audio`, audio | 3 | 432 | 412 | 8 | 0 | 0 | **0** |
| `edit_multi_av`, audio | 3 | 432 | 412 | 8 | 0 | 0 | **0** |
| 1ae48e7b157e, video | 2 | 1,161 | 1,158 | 0 | 0 | 1 | **2** |
| 5de2646e4ea0, video | 2 | 1,126 | 1,123 | 0 | 0 | 1 | **2** |
| ceb55bb5fb87, video | 2 | 589 | 586 | 0 | 0 | 1 | **2** |
| ea690039d902, video | 2 | 1,046 | 1,036 | 0 | 1 | 0 | **4** |
| 6e67814d336c, video | 20 | 140,959 | 140,959 | 0 | 0 | 0 | **0** |
| a166d09989f9, video | 3 | 97,022 | 97,009 | 2 | 0 | 0 | **11** |
| a166d09989f9, audio | 6,069 | 189,662 | 189,659 | 2 | 0 | 0 | **0** |

Two limits of the method. "Left out" takes every sync sample as a clean
random-access point, so an open group of pictures whose leading pictures
follow a recovery point would be counted there when a decoder still
needs what came before: lifting that limit can only move samples *into*
the last column. Disposability is read for H.264 only, so a disposable
sample of another codec sits in the last column today: lifting that one
can only move samples *out*. No published number moves either way —
every video row here is H.264 and every row of another codec is 0 — so
the conclusion holds whichever way the two are tightened.

**4. What is left.** A writer that left out and placed every sample it may
and refused the rest would package the five streams with 0 above — of the
six corpus files with several edits, only 6e67814d336c, whose audio has one
media edit; of the five several-edit fixtures, only `edit_multi_splice`.
Two further shapes are representable in the file but not usable:

- **A splice inside a frame.** The source presents a frame of the next
  media edit before the previous edit's last frame starts, which
  `docs/design/edit-lists.md` decision 3 reports as one packet per sample.
  `edit_multi_audio` and `edit_multi_av` carry the same audio edit list and
  the same 528 ticks of it at 44,100 Hz; a166d09989f9's audio, 140 at
  48,000. The output can hold it, each side of the splice in its own
  segment, since a segment carries its own decode time; the pinned ffmpeg
  then drops one frame of the pair (not reproduced here).
- **An empty edit between media edits**, which leaves a hole on the
  presentation timeline that no `EXTINF` can hold without a discontinuity.

### Criteria

What this section had to show for the refusal to stand, fixed before the
measurements above:
- **A reader applies at most one media edit of a fragmented rendition**
  (measurement 1). Had the pinned ffmpeg applied the whole list, the
  writers would carry the source's edit list into the init segment, as the
  clip writer carries it, and the refusal would end there.
- **A sample a presented sample needs, with no place on the output's
  timeline, exists in the corpus** (measurements 2 and 3). Had every such
  sample been droppable, placeable or disposable, the writer of
  measurement 4 would be the change, with the clip writer's bars.
- Otherwise the refusal stands and the design that lifts it is recorded
  with the costs it carries.

### Decision

**The HLS writers keep refusing a source with several media edits**, with
the message of common decision 1, until the design below is built. Support
for the streams measurement 4 leaves — those with nothing to hide and no
splice inside a frame — would add a rewritten decode timeline and rewritten
stored durations to the listing of every sample, for one corpus file of
six, and the design below replaces it rather than building on it.

Rejected alternatives, with the measurement that closes each:
- **Carry the source's edit list into the init segment.** Measurement 1:
  only the first media edit's media time reaches the reader, so the
  material the edits cut out would play.
- **Move the hidden samples out of the way.** Measurement 2: either
  placement loses a third of the frames.
- **Leave out every sample no media edit presents.** Measurement 3: the
  next presented sample then has no references in five of the six corpus
  files.
- **Support the representable streams only.** Measurement 4.

### The design that lifts it

One run of segments per media edit, as HLS expresses an edited timeline:

- segments never straddle a media edit; a run starts at the latest sync
  sample at or before its media edit's start, and ends at its last
  presented sample;
- each run has its own init segment, whose edit list is that one media
  edit — one media edit, whose media time measurement 1 shows a reader
  applies — so the samples before the edit's start are hidden and the
  splice inside a frame is trimmed by a decoder rather than by the writer;
- the runs are separated by `EXT-X-DISCONTINUITY`, and `EXT-X-MAP` names
  each run's init segment.

Nothing is rewritten: decode times, durations and composition offsets stay
the source's, which is what `docs/design/hls.md` ("Timing") promises. The
costs are a new index format (one edit and one init segment per run, a run
per segment entry), a resource name per init segment, playlists that
players follow across discontinuities, one group of pictures repeated per
media edit, and a bound on the number of runs — a166d09989f9's audio would
ask for 6,069. Measurement 1 covers the reader; the player it is for was
not measured, and the HLS spike's player harness (hls.js 1.5 in Chrome) is where that belongs; that work is tracked separately.

Until then the refusal stands, and `coreav clip` is the way to get an
edited MP4 out of such a source.

## M4A extraction

Extraction copies every sample of one AAC track into a plain MP4, in order,
keeping the source's media timescale, sample durations and movie timescale (an HE-AAC track whose media timescale divides its decoded rate is written at that rate, every media duration and media time multiplied alike: "HE-AAC timescale" below). Nothing is cut and no timescale is chosen, so a list
of several media edits needs no mapping: the output carries the source's
entries.

### Decisions

1. **The output's edit list is the source's entries.** Each entry keeps its
   duration in movie ticks and its rate; a media edit's media time moves by
   the source's first decode time, which output decode times leave at 0, as
   the simple model's one media edit already does; an empty edit stays −1.
   The output's movie duration is the entries' durations added up (ISO/IEC
   14496-12 8.6.6: they play one after another), where before it was the
   leading empty edit plus the one media edit — the same value for a simple
   list.
   - Rejected: **flattening the edits into one**, which would present media
     the source hides.
   - Rejected: **cutting the samples the edits do not present**, which
     would restart the AAC decoder at every splice (the clip writer records
     that difference as `KD-C-AUDIO-RESTART`) and gains nothing: the M4A
     extraction copies the whole track.
2. **The refusals of common decision 1 that still apply:** an edit list
   outside the model, and several edits in a movie with fragments — probe
   classifies both as `Unsupported`, so both fail with `edit list outside
   the supported model` before a byte is written. Nothing else is reachable
   for a list of several: the output keeps the source's movie timescale, so
   no duration is inexact and none needs more than 32 bits; the source has
   no fragments, so its first decode time is 0 and no media time has to
   move; and no lookup walks the entries, so a long run of empty edits is
   harmless. The writer keeps its other checks for the simple model (a
   movie timescale of 0, a media edit before the first sample, an edit that
   lasts to the end of the media).
3. **Memory.** The entries are the ones the parser already keeps (24 bytes
   each, `max_retained_bytes`), and the output writes them once.
   a166d09989f9's 6,069 audio entries cost 145,656 bytes of the parser's
   retained bytes and 72,844 in the output's version 0 `elst` (measured).
4. **Everything else** in the M4A extraction's design is unchanged: the
   track choice, the sample entry, the media header duration, the
   `iTunSMPB` amendment and the two passes.

### Oracle

`scripts/extract_check.py` keeps every check it makes today. For a source
with several media edits it adds common decision 3's view, since `ffprobe`
and `ffmpeg` apply their own model to such a list, and both files here
carry the same list, so the existing checks would agree with themselves:

- **Stored samples:** `ffprobe -show_packets -show_data_hash
  -ignore_editlist 1` of the output equals the source's, by hash, size and
  stored times less the source's first decode time.
- **Presented frames:** the frames the output's edits present, by ISO/IEC
  14496-12 from its own `elst`, `mdhd` and `mvhd`, equal the frames the
  source's edits present, at the same times. Both are decoded with
  `-ignore_editlist 1 -copyts`.
- The edit arithmetic is written in the oracle script, from the
  specification, not imported from CoreAV.

One new known divergence, `KD-X-EDIT-MULTI`: `ffprobe` lists more packets
than the track has samples for such a list (a preroll per media edit and
samples repeated at the boundaries, `docs/design/edit-lists.md` rules 7 to
9), for the source and the output alike, while `nb_frames` counts the
sample table. It is tied to values read from the source: the output's
timescales and entries must be the source's with every media time moved by
the source's first stored decode time.

### Pass criteria

Fixed here, before the run:
- **Fixtures, 100%.** Every AAC track of every fixture, `edit_multi_audio`
  and `edit_multi_av` included, passes the Rust round trip and every oracle
  above, directly or through a known divergence; the existing fixtures keep
  their results. The known-divergence mutation audit of an earlier change
  (`tools/oracle/writer_kd_audit.py`) finds nothing covered or absorbed for
  `KD-X-EDIT-MULTI`.
- **Corpus, 100%.** Under the protocol of `docs/spikes/audio-extract-bench/`,
  every AAC track with several edits is extracted and passes every oracle:
  a166d09989f9 (6,069 audio edits) becomes accepted. Every other corpus
  file keeps its result, shown by extracting every AAC track of all 544
  inputs with this writer and with `main` and comparing the outputs byte
  for byte.
- **Refusals.** Each case of decision 2 refuses with its message and writes
  nothing (a test).
- **Speed: at most 1.00 per class**, against the ffmpeg
  command of `docs/spikes/audio-extract-bench/`, on the held-out inputs, with the
  share faster than FFmpeg reported beside every median, and no
  class worse than `main` built from the same commit, measured in the same
  run on the same machine (the method at the end of this note). Extraction
  reads and writes the same bytes as before for every source without
  several edits, so the existing classes are a regression check.

### Results

In `docs/spikes/audio-extract-bench/`, "Sources with several edits".
Against the bars as registered above: fixtures met (44 of 44, two of them
with several edits); corpus met (a166d09989f9, the one `real` file whose
AAC track has several edits — 6,069 entries — is extracted and agrees, and
533 of the other 534 AAC tracks of the 544 inputs are byte for byte what
`main` writes, with no stream newly refused);
refusals met; the mutation audit finds nothing covered or absorbed. No
held-out input has such a track, so the speed result is a development
measurement: 0.0725 of FFmpeg's time on that file, 0.0969 to 0.1455 on
three sources with a simple edit, all four faster than FFmpeg, and
within 0.4 ms of `main` on the three it also accepts, whose outputs are
byte identical.

### HE-AAC timescale

An HE-AAC decoder outputs SBR's rate, not the core's (ISO/IEC 14496-3
1.6.5.2, table 1.22): the extension frequency an explicit signal in the
`AudioSpecificConfig` gives, or, with implicit signalling (SBR data only
in the frames), twice the core frequency. Encoders often store such a
track with the core's rate as its media timescale and sample entry rate
(22,050 for a stream decoded at 44,100), so a tick is two decoded samples.
ISO/IEC 14496-12 12.2.3.3 asks for an audio timescale that matches the
sampling rate or is an integer multiple of it, to enable sample-accurate
timing, and for a sample entry rate that documents the default output.

Decision: when the source's media timescale divides the decoded rate, the
extraction writes the decoded rate as the media timescale and every media
duration and media time (sample durations, the media header's duration,
each edit's media time) multiplied by the same whole number, so every
instant is the source's; the sample entry states the decoded rate whenever
SBR is found. The movie timescale and the edits' durations stay the
source's. SBR is found from the configuration (`Profile::Aac` with `sbr`),
or, for an AAC LC configuration of at most 24 kHz (the rates implicit
signalling can double), from its first frames. One frame ending with an
SBR fill element (`aac_ends_with_sbr`, which moves from the CLI's MPEG-TS
listing to `coreav-isobmff`) is not enough: a plain frame read from its
end shows what looks like SBR data now and then (6 of 96 low-rate AAC LC
files of the pinned `ffmpeg`'s own encoder had such a frame among their
first eight, black box), while an HE-AAC stream carries SBR data in every
frame. So the extraction counts the first 16 frames and takes SBR as
found only when at most two of them disagree (`aac_sbr_found`, the rule
the MPEG-TS listing already used, now shared). A stream whose first one
or two frames carry no SBR data and the rest do is taken as HE-AAC too,
where the pinned `ffmpeg` keeps the core's rate (black box, crafted
streams; from the fourth frame on, neither scales). An `iTunSMPB` item is still
checked against the source's ticks, and the edit it gives is multiplied.
The JSON summary (`coreav.extract-audio/1`) keeps reporting the source's
timescale and durations.

The pinned `ffmpeg` 9.0.2 writes the decoded rate as its output's media
timescale and sample entry rate for every HE-AAC input it was given (black
box: 46 inputs with 16, 22.05 and 24 kHz cores, implicit, backward
compatible and hierarchical signalling, with and without parametric
stereo), and multiplies the sample durations, the media duration and the
edit's media time the same way.

## `coreav-text` timing

A cue or a chapter is a sample shown on the presentation timeline, and
`coreav-text` writes no container: it needs no mapping of samples to an
output, only the intervals each sample is shown over. With several media
edits those are the parts of it that each edit shows, which is where this
section differs from the packet rule.

### Decisions

1. **Every media edit that shows part of a sample shows it.** ISO/IEC
   14496-12 8.6.6: the entries play one after another, each presenting its
   own window of the media. So the timing takes the sample's composition
   interval, intersects it with each media edit's window, and maps each
   overlap through that edit — not through one edit alone.
   - An edit list that cuts the media into pieces and plays them back to
     back presents exactly what no edit list does, so every cue comes out
     whole. That is the common real shape:
     `docs/design/edit-lists.md` ("Why") counts up to 6,069 splices in one
     corpus track.
   - Rejected: **cutting each sample to the one edit that maps it**, which
     `docs/design/edit-lists.md` decision 2 picks for a *packet*, whose one
     time cannot express two presentations. A cue is not a packet: the
     writer emits one per interval and nothing is lost. Taking the packet
     rule here truncated every cue of a list of contiguous edits — measured
     before this note: a text track whose edit list tiles its media in four
     showed its first cue for 250 ms of its 650.
2. **A sample two edits show comes out twice,** at the two places they show
   it, which is what decision 3 of `docs/design/edit-lists.md` says a
   consumer that needs every presentation should do with `Track::edits`.
   Runs of edits with nothing between them are merged first, so the second
   cue appears only where the presentation timeline really jumps.
3. **The edits are walked, not copied.** A text track with a composition
   offset stops the extraction (`docs/design/text.md`, decision 7), so its
   samples come in composition order and one cursor over the track's own
   entries serves them all, moving forward only; the scan for the edits
   that show one sample starts there and leaves the cursor where the next
   sample needs it. No table of windows is built, so a list of thousands of
   edits costs nothing beyond the entries the parser already keeps, and the
   long run of empty edits the clip writer refuses is harmless here: the
   cursor crosses a run once for the whole track, not once per sample.
   The samples tile the media and the windows do not overlap, so the total
   work is one step per sample plus one per edit.
4. **The refusals that still apply:** an edit list outside the model and
   several edits in a movie with fragments (probe classifies both as
   `Unsupported`), and a movie timescale of 0, which leaves the windows
   unconvertible.
5. **Everything else** in `docs/design/text.md` is unchanged: the rounding
   to whole milliseconds for WebVTT, exact ticks for chapters, and the
   counts of samples no edit presents.

### Oracle

`scripts/text_check.py` gates a track with several media edits on its own
boxes instead of on ffmpeg's model of such a list, as common decision 3
asks:
- **Subtitles:** every cue — its start, its end and its text — equals what
  the edit list gives the track's samples by ISO/IEC 14496-12 8.6.6,
  computed in the oracle from the track's `stts`, its fragments' runs, its
  `elst` and its sample texts, rounded as `docs/design/text.md` says CoreAV
  rounds. The arithmetic is written in the oracle from the specification,
  not from the writer's rule: it intersects each sample with every window,
  as decision 1 above does, so a writer that cut a sample to one edit
  would fail it. The comparison is reported as a problem, never skipped
  quietly, when the summary says a sample was dropped for a reason the
  harness cannot account for.
- **Chapters:** the same, in exact ticks.
- **Second view (reported, not gated):** ffmpeg's cues. It anchors later
  edits on decode times, rounds durations to whole ticks and repeats
  samples at the boundaries (`docs/design/edit-lists.md`, "What `ffprobe`
  lists", rules 1 to 9), so it writes cues CoreAV does not: measured, it
  writes 5 where CoreAV writes 3 for a list of four contiguous edits.
  Gating on its cue list, times or texts, would be gating on those rules.

Six inputs, built by the oracle from `text_sized.mp4` and
`chapters_sized.mp4` with their text track's edit list replaced
(`scripts/elst_craft.py`), each for one shape:
- **splice** (both files): two media edits with media between them that
  neither shows, so one cue is cut at the first edit's end, one is shown
  from the middle of the second, and the sample between them is not shown;
- **contiguous** (both files): four media edits that play the media back to
  back, which must present exactly what no edit list does;
- **boundary**: two contiguous media edits whose boundary is a sample
  boundary;
- **split-cue**: a splice inside the first cue with an empty edit over it,
  so that one sample comes out as two cues.

They are built in the oracle's work directory and not by
`scripts/gen-fixtures.sh`, because `docs/design/edit-lists.md` ("What
`ffprobe` lists") models `ffprobe`'s listing of such a list for video and
audio only: a text track follows neither preroll rule, so a fixture
carrying one would leave packets uncovered in the probe oracle. Measuring
that rule and extending the model is an earlier change; the inputs become fixtures
then.

### Pass criteria

Fixed here, before the run:
- **Fixtures, 100%.** Every text and chapter track of every fixture, and
  the six several-edit inputs above, passes `scripts/text_check.py`; the
  existing fixtures keep their results, and the Rust tests pin the shapes
  above. The fixture run passes `--require-several-edits`, so an input the
  oracle cannot build fails it and the criterion cannot go vacuous; a
  corpus run, whose directory holds none of the sources, builds none and
  judges the corpus alone. The probe oracle keeps its coverage: no fixture gains a
  text track with several media edits.
- **Corpus, 100%.** Under the protocol of the stage 3 text run, every text
  or chapter track with several media edits passes, and every other file
  keeps its result — shown by running the gate over the whole subset with
  this writer and with `main` and comparing every item, since the two
  mistakes this change made on the way were both invisible to the fixture
  run.
- **Refusals.** An edit list outside the model is still refused before
  anything is written (a test).
- **Speed.** `coreav extract-subtitles` and `coreav chapters` walk the same
  samples as before, with one cursor over the edit list entries added, and
  neither has a speed bar of its own today. A development
  measurement against `ffmpeg -c:s webvtt` is reported.

### Results

In `docs/measurements/text-several-edits.md`.

## Join

`join_mp4` (`docs/design/join.md`) took one edit per input and refused an
input with several. Every input it otherwise takes is now joined as it
presents itself, several edits included, as `coreav clip` and `coreav
split` write them; the segments of a split whose cuts fall inside a run
of edits join back.

### Decisions

1. **One output edit per media edit of each input.** Each media edit of
   an input becomes an output edit at the input's offset, cut to the span
   the input presents (its start and end as `docs/design/join.md`
   computes them); the empty edits between them stay empty edits. Edits
   that meet in both presentation and media time merge, as before, so the
   segments of a split give back the source's edits. Rejected: rewriting
   decode times to close the gaps an input's edits leave (no writer
   rewrites them, `docs/design/hls.md`, "Timing").
2. **An input's end between two of its edits** is where the earlier edit
   ends (`Timeline::presented_until`): nothing is presented after it, and
   holding the last frame across the gap would show frames the input's
   edits cut out. The stream duration that bounds the end of an input
   with one edit (the edit's own length) does not bound one with several,
   whose edits bound it. Where an input presents its samples is taken
   from every edit that shows part of a sample (`Scaled::presented`), not
   only from the edit that owns the sample's time: two edits can show
   parts of one AAC frame or video frame, and the later one is kept (a
   test, `audio_shown_twice_from_one_frame_keeps_both_passages`).
3. **The hold before a later video start** (`docs/design/join.md`,
   "Timing") is kept only where no frame of the input is composed after
   its last edit. Otherwise the running edit would show that frame (cut
   out by an edit, or kept by a clip only for decoding), and the gap stays
   an empty edit, as the input's own empty edit was. The empty edit after
   a media edit is what `docs/design/join.md` rejects for a gap between
   inputs (ffmpeg plays frames of the next edit during it); here it
   reproduces the input, which ffmpeg plays the same way.
4. **Placement is unchanged.** Each input still starts at its earliest
   presented sample. For the segment of a split that starts in an empty
   edit of its source, that is its audio, which starts after the cut by
   less than an AAC frame (`docs/design/split.md`): such a join gives back
   the source's samples and edits with that gap shorter by the audio's
   lead, the following times earlier by as much (a test,
   `segments_across_an_empty_edit_join_back`). Rejected: starting every
   input at its own time 0, which would turn the leading empty edits of
   clips and remuxes into gaps between inputs.
5. **Refusals.** The cases of common decision 1 that apply to any writer
   still refuse (an edit list outside the model, several edits in a movie
   with fragments); the join no longer refuses several edits.

### Oracle

`scripts/join_check.py`, as for every join, with common decision 3 for
joins where an input, or the source of joined segments, has several
edits: samples and frames read with edit lists ignored, and the edit
arithmetic the harness's own (`iso_windows`, `iso_presented`):

- the output's stored samples are the parts' (the inputs, or the source)
  one after another: data hash, size, sync flag, composition offset, each
  part's times moved by one constant;
- the frames the output's edits present are the frames each input's edits
  present, at its time plus its offset (for segments, the source's frames
  from the first segment's start to the joined video's end), and the
  inputs follow one another as for any join;
- each output audio edit presents, at its time, the media its input's
  edits present there (within one tick), and the output presents what
  each input presents, up to where the next one starts;
- streams, ends and MediaInfo as for any join; ffmpeg's edit-applied
  video is reported (the second view), not gating.

Each check fails on a bug-shaped change of the output (an edit's media
time moved by a frame, a sample's data, an empty edit closed up, an audio
edit's media time moved by an AAC frame), and
`tools/oracle/tests/test_join_kd.py` holds the audio check's cases.

Every join of the fixtures is also compared with `ffmpeg -f concat -safe
0 -i LIST -c copy` on its inputs (`--ffmpeg-concat`,
`docs/design/join.md`, "Verification"), the join of the 0.1.0 command
set (`ffmpeg -f concat [-safe 0] -i LIST -c copy OUTPUT`).

### Pass criteria

Fixed with this section, before the fixture run below:

- **Fixtures, 100%.** Every fixture join of the `fixtures_round_trip`
  test passes the round trip and `join_check.py` with `--ffmpeg-concat`:
  the segments and clips of `edit_multi_splice.mp4`,
  `edit_multi_bframes.mp4`, `edit_multi_empty.mp4` and
  `edit_multi_av.mp4` (no longer skipped), every pair, and every join
  that passed before.
- **Tests.** `crates/coreav-isobmff-write/tests/join.rs`: inputs with
  several edits in either place, an empty edit inside an input, audio with
  several edits, and segments cut at an empty edit.
- **Corpus.** Not in this round: the six corpus files with several edits
  are the next measurement, under the split protocol, split then joined.

### Results

On the fixtures, with the pinned FFmpeg 7.0.2 and MediaInfo: every fixture
joins (no join refused for several edits), 230 of 230 joins pass every
oracle, the nine with several edits directly; and every packet of the
joins `ffmpeg -f concat` takes agrees with ffmpeg's output.

## Speed comparisons for the writers that follow

A bar that a change must "not make worse" compares the change with `main`
built from the same commit it branches from, both measured in the same
run on the same machine, engines interleaved, never with an earlier run:
`perf-06` and `perf-09` ran on different machines (AWS at 3.1 GHz, Azure at
3.7 GHz), which moved `lav_avio` by 0.70 and CoreAV's clip by 0.79, more
than the change being judged. The bar is the median per-clip ratio of the
change over `main`, with its 95% interval; the absolute bars against
FFmpeg stay as they are (at most 1.00).

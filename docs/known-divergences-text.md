# Known divergences: subtitle and chapter extraction versus the oracles

Every intentional difference between what `coreav extract-subtitles` and
`coreav chapters` produce and what the black-box oracles report is
documented, with a machine-readable twin (`docs/design/text.md`). The twin is
[`known-divergences-text.json`](known-divergences-text.json);
`scripts/text_check.py` applies it, and `tools/oracle/tests/test_text_kd.py`
keeps this page, the twin and the predicates in sync.

Cue entries are rewrites: each turns one side's cue list into what the other
side would show if the documented difference were absent (for example,
ffmpeg's cues without text are dropped), and applies only when its
conditions hold, rechecked from the file's own box headers and the oracles'
output. A stream counts as agreement only if the two lists are equal after
the rewrites; chapter and stream entries are plain predicates. Covered items
are reported apart from direct agreement.

The oracles are `ffprobe` and `ffmpeg` 7.0.2 and MediaInfo 26.10. Predicates
never use file names. Counts are from the corpus v1 run of
[`docs/measurements/text-corpus-v1.md`](measurements/text-corpus-v1.md).

## KD-T-MI-HOURS

- Field: MediaInfo `Events_Total`, `Duration_End`.
- When: a cue ends at 100 hours or later. WebVTT timestamps have two or more
  hour digits (WebVTT 4.5); MediaInfo skips cues with three or more.
- Coverage requires: MediaInfo's event count, first start and last end equal
  those of the cues that end before 100 hours, and at least one cue does.
- Corpus: 1 stream (a last sample whose `stts` duration is 4,294,957,296
  ms, which the cue keeps).

## KD-T-ESCAPE

- Field: cue text.
- When: the text holds `&`, `<` or `>`. CoreAV writes `&amp;`, `&lt;` and
  `&gt;` (WebVTT 4.2.2: cue text spans contain no `&` or `<`, and a payload
  never contains `-->`); `ffmpeg` writes them as they are.
- Rewrite: CoreAV's text with the three references undone.
- Corpus: 3 streams.

## KD-T-STYLE

- Field: cue text.
- When: the stream has styling boxes (`styl`, `hlit`, `hclr`, `krok`,
  `blnk`), which CoreAV drops and counts (design choice 9);
  `ffmpeg` writes them as `<b>`, `<i>`, `<u>` and `<font>` tags.
- Rewrite: `ffmpeg`'s text without those tags.
- Corpus: 4 streams.

## KD-T-LEAD-SPACE

- Field: cue text.
- When: a sample's text starts with spaces. `ffmpeg` drops them, from 3GPP
  timed text (`tx3g`) and QuickTime text (`text`) alike, with or without
  style records (a `tx3g` and a `text` track written by `ffmpeg`, one
  cue's first letter made a space; both lose it). CoreAV keeps the text as
  stored (design choices 8 and 9); WebVTT cue text may start with spaces.
- Rewrite: CoreAV's text without those leading spaces, where that is the
  only difference.
- Coverage requires: equal cue counts, and each such cue, spaces included,
  one of the source's own samples: its exact times from the sample tables
  and edit list, rounded as CoreAV rounds, and its text written as CoreAV
  writes cue text (line breaks as LF, empty lines removed, `&`, `<`, `>`
  escaped).
- Corpus: 3 streams.

## KD-T-EMPTY

- Field: cue count.
- When: samples hold no text, or only white space and line breaks. CoreAV
  writes no cue for them (design choice 8); `ffmpeg` writes cues with an
  empty payload.
- Rewrite: `ffmpeg`'s list without those cues.
- Corpus: 5 streams.

## KD-T-INVALID

- Field: cue count.
- When: sample text is not valid UTF-8 or UTF-16, typically old QuickTime
  text in a Mac OS encoding, which the QuickTime File Format does not
  specify. CoreAV writes the text with U+FFFD in place of the invalid
  sequences and counts the samples (`invalid_text`); `ffmpeg` drops them.
- Rewrite: CoreAV's list without cues that hold U+FFFD; requires a non-zero
  `invalid_text` count, and each cue dropped to be one of the file's own
  invalid samples, read from its sample tables: the sample's text decoded
  with replacement and written as CoreAV writes cue text (line breaks as
  LF, empty lines removed, `&`, `<`, `>` escaped), at the sample's exact
  times from `stts` and the edit list, rounded as CoreAV rounds; as many
  cues as such samples. The text is compared as CoreAV writes it, or with
  its escapes undone where KD-T-ESCAPE has undone CoreAV's. Samples in
  movie fragments are not read, so the entry does not apply to fragmented
  sources.
- Corpus: 3 streams.

## KD-T-CR

- Field: cue text.
- When: QuickTime text uses CR as its line break. TS 26.245 5.11 says CR
  should start a new line, and CoreAV writes one; `ffmpeg` drops the CR.
- Rewrite: CoreAV's text without its line breaks, where the texts are equal
  without them; QuickTime `text` tracks whose samples hold CR only.
- Corpus: 5 streams.

## KD-T-ZERO-END

- Field: cue end.
- When: `ffmpeg` has no duration for a packet (`ffprobe` lists the source
  packet without one: fragmented files, or the last sample of some
  QuickTime files) and writes a cue that ends where it starts, which WebVTT
  4.1 does not allow. CoreAV ends the cue at the end of the sample, from the
  sample tables or the fragment's run.
- Rewrite: `ffmpeg`'s end replaced by CoreAV's, for at most as many cues as
  `ffprobe` lists packets without a duration, where CoreAV's start and end
  are a sample's exact times from the source's sample tables and edit,
  rounded as CoreAV rounds them (`ffmpeg` gives no end to compare with).
- Corpus: 5 streams.

## KD-T-EDIT-ALIGN

- Fields: cue start and end.
- When: the track's only media edit starts at a media time M that is not a
  sample boundary. ISO/IEC 14496-12 8.6.6 presents media time M at the
  edit's start, and so does CoreAV; `ffmpeg` presents there the first sample
  that starts at or after M, so all its cues are earlier by the gap between
  M and that sample.
- Rewrite: `ffmpeg`'s times replaced by CoreAV's, where every one differs by
  that gap, computed from the source's `stts` (to 1.5 ms, as each side
  rounds its own times).
- Corpus: 16 streams, 15 of them in one file.

## KD-T-LAST-END

- Field: the last cue's end.
- When: `ffmpeg` extends the last cue to the end of the file (the format
  duration `ffprobe` reports); CoreAV ends it with the last sample, whose
  duration is in the sample tables.
- Rewrite: `ffmpeg`'s last end replaced by CoreAV's, when it is the format
  duration to 1 ms and CoreAV's is the end of the track's last presented
  sample, read from its `stts` and edit list and rounded as CoreAV rounds.
- Corpus: 6 streams.

## KD-T-ROUND

- Fields: cue start and end.
- When: the track's timescale does not divide 1,000, so times in
  milliseconds are not exact. CoreAV rounds each exact time to the nearest
  millisecond; `ffmpeg` rounds the start and the duration separately.
- Rewrite: `ffmpeg`'s times replaced by CoreAV's, where they are at most
  1 ms apart, the text is equal, and CoreAV's times are a sample's exact
  times, read from the track's `stts` and edit list (ISO/IEC 14496-12
  8.6.1.2, 8.6.6), rounded to the nearest millisecond, halves away from
  zero (`docs/design/text.md`).
- Corpus: 1 stream.

## KD-T-EDIT-RATE

- Field: `coreav extract-subtitles` exit code.
- When: the track's edit list has a media edit at a rate other than 1.
  Presentation times then fall outside CoreAV's edit model, which takes
  media edits at rate 1 only, and
  CoreAV refuses the track (exit 1, `unsupported_feature`); `ffmpeg` ignores
  the rate.
- Coverage requires: the refusal names the edit list, and the source's
  `elst` holds a media edit whose rate is not 1.
- Corpus: 1 stream.

## KD-T-CHPL

- Field: chapters.
- When: the file's only chapters are in a Nero `chpl` box, which no cited
  specification defines (design choice 6). CoreAV reports no chapter track;
  `ffprobe` lists the `chpl` chapters.
- Coverage requires: no chapters from CoreAV, chapters from `ffprobe`, a
  `chpl` box in `moov/udta`, and no text track named by a `chap` reference of
  an enabled track.
- Corpus: 10 files.

## KD-T-HMMT

- Field: chapters.
- When: a GoPro file has HiLight tags in an `HMMT` box, which no cited
  specification defines; `ffprobe` lists them as untitled chapters.
- Coverage requires: no chapters from CoreAV, untitled chapters from
  `ffprobe`, an `HMMT` box in `moov/udta`, and no text track named by a
  `chap` reference of an enabled track.
- Corpus: 1 file.

## KD-T-CHAP-ORDER

- Field: chapters.
- When: `chap` references name several text tracks: alternate languages,
  or the title and URL tracks of an enhanced podcast. QuickTime uses the
  first chapter list it finds, from an enabled track (QuickTime File Format,
  "Chapter lists"), and so does CoreAV; `ffprobe` lists another one, or a
  mix, also following references of disabled tracks.
- Coverage requires: two or more text tracks named by `chap` references,
  CoreAV's chapters from the first one an enabled track names and equal to
  that track's samples, read independently from its sample tables (titles,
  and times from `stts` when no edit moves them), and every
  `ffprobe` title equal to the text of a sample of one of those tracks, read
  from their sample tables.
- Corpus: 7 files.

## KD-T-QT-STYLE (retired)

Replaced by `KD-T-LEAD-SPACE`. It named QuickTime `styl` atoms as
the cause of `ffmpeg` dropping a cue's leading spaces and required them;
`ffmpeg` drops leading spaces without any style record as well, so a
QuickTime text stream without `styl` atoms (5f0f40c9396c) fell outside it.

# Subtitles to WebVTT and chapters to JSON (`coreav-text`)

Status: Accepted. Scope: the stage 3 stream-copy primitive "extracting
`tx3g` subtitles to WebVTT and chapters to JSON", use case 7, and the subtitles row of the roadmap table.

## What it does

- `extract_subtitles(source, limits, text_limits, choice, out)` writes one
  text track of an MP4/MOV file as a WebVTT file, front to back to any
  `std::io::Write`, and returns a summary: cues written, and what the
  output does not carry (styling and other modifier boxes by type, empty,
  unpresented, malformed and sub-millisecond samples, text that was not
  valid Unicode).
- `read_chapters(source, limits, text_limits)` returns the QuickTime chapter
  list of a file: one chapter per sample of its chapter track, with start,
  end and title, or `None` when the file has no chapter track.
- `coreav extract-subtitles [--stream N] [--force] INPUT OUTPUT` and
  `coreav chapters FILE` wrap them (`docs/schema/extract-subtitles-v1.md`,
  `docs/schema/chapters-v1.md`), with the conventions of `coreav clip` and
  `coreav extract-audio` (`docs/design/extract-audio-cli.md`): exit codes 0,
  1, 2, 3 and 141 (closed pipe), `coreav.error/1` on standard error, `-` for standard output, a
  temporary file renamed into place, `--force`.
- `coreav-isobmff` gains what both need: `Movie::chapter_tracks`, the
  `track_ID`s named by `chap` track references, and `Codec::QuickTimeText`
  for the `text` sample entry. `Movie::chapter_tracks` follows QuickTime
  and keeps only the references of enabled tracks; `Movie::chapter_references`
  keeps every track's, for the `ffprobe`-compatible output, since `ffprobe`
  lists a timed text track that any `chap` reference names as a `bin_data`
  data stream without a size (measured with a reference from a disabled
  track).

## Specifications

Clean room, from public specifications only:

- 3GPP TS 26.245 (ETSI TS 126 245 V19.0.0): the text sample (5.17: a 16-bit
  byte length, the text, then modifier boxes to the end of the sample), text
  encoding (5.1: UTF-8, or UTF-16 after a byte order mark), line breaks
  (5.11), and the modifier boxes (5.17.1: `styl`, `hlit`, `hclr`, `krok`,
  `dlay`, `href`, `tbox`, `blnk`, `twrp`; `disp` from the stereoscopic
  amendment).
- W3C WebVTT (Candidate Recommendation): file header, timestamps, cue
  timings and cue text, with its escapes.
- Apple QuickTime File Format documentation: chapter lists (a `chap` track
  reference from an enabled track to a text track; one sample per chapter,
  its display time the chapter's start; QuickTime uses the first chapter
  list it finds), the track reference atom (32-bit track IDs, 0 for an
  unused entry), the `text` sample description and text sample data (the
  same 16-bit length and text, then atoms), and the subtitle sample data
  (UTF-16 with a byte order mark in either byte order).
- ISO/IEC 14496-12: the track reference box (8.3.3), edit lists (8.6.6).

`ffmpeg` and `ffprobe` are black boxes for the oracle checks only.

## Choices

1. **A new crate, `coreav-text`.** It reads through `coreav-isobmff` and
   writes text, not ISO base media, so it belongs neither in the parser
   (read-only) nor in `coreav-isobmff-write` (MP4 writing).
   Rejected: a module in `coreav-isobmff-write`, which would give text
   extraction a dependency on MP4 box writing it never uses.
2. **Which tracks are subtitles.** 3GPP timed text (`tx3g`, `mov_text` in
   `ffprobe`) and QuickTime text (`text`), whose sample data has the same
   layout; `ffprobe` also calls a QuickTime text track that is not a chapter
   track `mov_text` (black-box observation). The default is the first text
   track that is not a chapter track; `--stream N` names any text track,
   chapter tracks included. Rejected: `tx3g` only (the issue's wording),
   which would refuse tracks `ffmpeg` converts the same way.
3. **Only text tracks are parsed.** Both entry points probe with an
   allowlist of the two text codecs, so the sample tables of video and audio
   tracks are never decoded: less work, and less untrusted input reaches
   table code. Consequently the commands take no `--allow` or `--strict`:
   the codec list is fixed by what they read. Rejected: the caller's
   allowlist plus the text codecs, which parses every allowed track's tables
   for nothing.
4. **QuickTime text is not on the default allowlist.** Adding it would turn
   every chapter track into a stream of `coreav probe`, a change to probe
   output (and to the `ffprobe` comparison, where `ffprobe` calls chapter
   tracks `bin_data` data streams) outside this issue. Chapter tracks stay
   dropped streams in probe (`KD-QT-TEXT` in `docs/known-divergences.md`)
   and are read on request by `read_chapters`.
5. **Chapter references.** `coreav-isobmff` reads the `chap` entries of
   every `tref`, also in tracks the allowlist drops (the chapter track's
   referrer is usually video or audio), keeps those of enabled tracks
   (QuickTime File Format) in `Movie::chapter_tracks` and those of every
   track in `Movie::chapter_references` (what `ffprobe` follows), and lists the IDs in file order without repeats,
   at most 64, a fixed bound not charged to `max_retained_bytes`, so the
   room other boxes need is unchanged. A `tref` over 4 KiB or with a
   malformed entry is ignored, not an error, and so is a defect after the
   media box of a dropped track: chapters are optional, and probe must not
   start rejecting files it accepts today. A read the source fails is not a
   defect of the file: it fails the probe there as anywhere else.
   The chapter track is the first listed ID that names a kept text track.
6. **Nero `chpl` is not read.** It is in neither Apple's QuickTime File
   Format, ISO/IEC 14496-12 nor the MP4RA registry (the issue asks for it
   only if a cited specification defines it). `ffprobe` reads it, so a file
   whose only chapters are in `chpl` is a known divergence (`KD-T-CHPL`).
7. **Timing.** A cue or chapter starts at its sample's presentation time
   (the composition time through the edit list, as `coreav-isobmff`
   computes it) and ends at that time plus the sample duration, both cut to
   the track's edit: with several media edits, to the one that maps the
   sample (`docs/design/edit-list-writers.md`, "`coreav-text` timing"),
   so a cue that runs past a splice ends there and one the next edit shows
   from its middle starts where that edit does. Samples outside the edit
   are skipped and counted. A track whose edit list is outside the
   supported model (`docs/design/edit-lists.md`, decision 1)
   is refused before anything is written, and a sample with a composition
   offset (in `ctts` or a fragment's run; zero offsets are none) stops the
   extraction, as in audio extraction: presentation order or times are then
   not known exactly. WebVTT times are
   whole milliseconds, rounded to the nearest; a cue that rounds to no
   duration is skipped and counted (WebVTT requires its end after its
   start). Chapters keep exact ticks.
8. **Text.** Decoded as TS 26.245 says (UTF-8, or UTF-16 after a byte order
   mark, both byte orders as Apple allows); invalid sequences become U+FFFD
   and are counted. QuickTime's documentation does not give the encoding of
   `text` samples (old ones are often in a Mac OS encoding), so they are
   decoded the same way. The text ends at its first NUL: TS 26.245 needs no
   terminator, and some writers (DJI drones in the corpus) store a
   NUL-terminated string in a fixed-size field followed by stale bytes. In
   WebVTT, `&`, `<` and `>` are escaped as `&amp;`, `&lt;`, `&gt;` (the
   three references every WebVTT version accepts; `>` too, so `-->` can
   never appear), the TS 26.245 line breaks (LF, U+2028, U+2029, CR, U+0085,
   CR LF) become LF, and lines that are empty or only white space are
   removed (an empty line ends a cue). A sample with no text left is a gap
   (TS 26.245 does not define empty samples; writers use them to clear the
   screen) and gives no cue. Chapter titles keep their text as decoded;
   empty titles are kept.
9. **Styling is dropped and reported.** Modifier boxes are counted by type
   (the TS 26.245 types, then one `other` count, which includes QuickTime's
   own atoms) and never interpreted. The sample entry's defaults (font,
   colours, justification, text box) are not read. Rejected: mapping styles
   to WebVTT tags as `ffmpeg` does; the issue asks for text and timing, and
   tags are a second rendering model to keep correct.
10. **Malformed samples.** A sample shorter than its declared text length is
    skipped and counted rather than failing the whole track; bytes after the
    text that are not whole boxes are ignored and counted. Both are rare and
    visible in the summary.
11. **Output layout.** `WEBVTT`, a blank line, then one cue per sample:
    timings with hours always written, the text, a blank line. No cue
    identifiers, no BOM, LF line ends. A track without text gives a file
    with the header only.

## Limits and memory

Reading uses the parser's `Limits` (box, table, packet and fragment limits).
`TextLimits` adds: `max_sample_bytes` (1 MiB; TS 26.245 asks for at most
2,048 bytes of text), `max_chapters` (10,000) and `max_title_bytes` (4 MiB
of titles in total). Going past one of them fails with
`TextError::LimitExceeded`, whose `TextLimitKind` names the field; the
parser's limits fail with its own error inside `TextError::Source`. Subtitles are written one sample at a time through one
reused buffer, so memory is the text track's sample tables plus one sample,
whatever the file size; the chapter list holds the titles, bounded by the
limits above.

## Verification

- Unit and integration tests in `coreav-text` on synthetic movies: UTF-8 and
  both UTF-16 byte orders, invalid text, each line break, escapes, empty and
  malformed samples, modifier box counts, edit lists (shifted, cut), chapter
  references from enabled, disabled and dropped tracks, limits.
- Fuzz targets (`crates/coreav-text/fuzz`): `subtitles` runs arbitrary input
  through `extract_subtitles` and `read_chapters` and checks that every
  output is WebVTT as promised (header, timestamp syntax, end after start,
  starts in order, escaped text, no empty line in a cue); `sample` runs
  `parse_sample` and `cue_text` on arbitrary bytes. `coreav-isobmff`'s
  `probe` target covers the new `tref` reading.
- Oracles, `scripts/text_check.py`, with differences that follow from this
  note listed in `docs/known-divergences-text.md` and its machine-readable
  twin:
  - subtitles: the output passes a strict WebVTT check; `ffprobe` reads it
    as WebVTT with one packet per cue (start, duration, payload hash);
    MediaInfo reads it as WebVTT with as many events as cues, starting at
    the first and ending at the last; and the cues equal those of
    `ffmpeg -c:s webvtt` on the source stream (times to the millisecond,
    text);
  - chapters: count, start, end (exact fractions of a second) and titles
    equal `ffprobe -show_chapters`.
- Pass bars, fixed here before the corpus run:
  - fixtures (CI, `fixtures` workflow): 100% of the `mov_text` streams and
    of the files with chapters;
  - corpus v1, every `real` and `edge` ISO base media file tagged with a
    subtitle track or chapters (112 files): at least 98% of the streams
    `ffprobe` reads as `mov_text`, and at least 98% of the files where
    `ffprobe` or CoreAV finds chapters, among files `coreav probe` accepts,
    pass every oracle directly or through a known divergence; 100% of the
    WebVTT outputs pass the strict check; no crash on any of the 112 files.
    Every failure is investigated and recorded. Files `coreav
    probe` rejects are reported apart (probe has its own oracle).
- No performance bar: text tracks are small, and no claim against FFmpeg's
  speed is made for them.

## Speed bars (fixed before the run)

- **Operations:** `extract_subtitles` of each `mov_text` stream to a WebVTT
  file, and `read_chapters`, with the library defaults on a buffered file
  source.
- **Baselines (gate):** libavformat and libavcodec in-process:
  - subtitles: open the input with every other stream discarded, read the
    stream's packets, decode them (`mov_text`), encode them (`webvtt`) and
    write a WebVTT file, the work of `ffmpeg -map 0:N -c:s webvtt`;
  - chapters: `avformat_open_input`, after which libavformat has read the
    chapter list (`AVFormatContext.chapters`), the work behind `ffprobe
    -show_chapters`.

  **Reported:** the `ffmpeg` and `ffprobe` command lines.
- **Inputs:** the held-out corpus v1 `real` files with a `mov_text` stream
  (subtitles; every such stream) or with chapters CoreAV reads (chapters).
  These held-out sets are small (3 files with subtitles, at most 6 with
  chapters), so each bar must also hold on every corpus v1 `real` file of
  its kind (34 files with subtitles or chapters), reported separately; both
  must pass. One class each.
- **Method** (the method of `docs/spikes/remux-bench/`, in `docs/spikes/parity-bench/`):
  one file at a time on the development machine, sources in the page cache,
  outputs on a tmpfs; per file, one warm-up run of every engine, then 5
  rounds with the engines' order rotated; a file's time per engine is the
  median of its 5 rounds; ratios are per file, ours over the baseline.
  CoreAV and the baselines are timed inside their processes, around the
  operation only. The baselines are libavformat 7.0.2 in-process (the pinned
  LGPL package of Spike C, written from the public API
  documentation), with 64 KiB custom I/O like CoreAV's buffered source.
- **Bar:** the median of the per-input ratios is at most **1.00**, on the
  held-out inputs and on all `real` inputs.

**Result: met.** Subtitles: **0.030** on the 4 held-out streams, **0.040**
(95% CI 0.034 to 0.073) on all 12 `real` streams. Chapters: **0.006** on
the 4 held-out files, **0.005** (95% CI 0.003 to 0.016) on all 14 `real`
files where CoreAV reads chapters. libavformat demuxes every interleaved
packet to reach one subtitle stream and reads the whole movie box before
it lists chapters; CoreAV reads the one track's samples and the chapter
boxes. Evidence: `docs/spikes/parity-bench/` (`subs-02`, `chapters-02`).

## Results

Corpus v1 (`docs/measurements/text-corpus-v1.md`): 59 of 59 subtitle
streams and 35 of 35 chapter files pass every oracle (25 and 17 directly,
the rest through the known divergences), every WebVTT output passes the
strict check, and no file crashes either command. The first run passed 25
and 27; the fixes it led to are choice 8's NUL and white-space rules, and
the other differences are the known divergences of
`docs/known-divergences-text.md`.

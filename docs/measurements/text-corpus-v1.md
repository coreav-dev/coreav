# Subtitles and chapters against `ffmpeg`, `ffprobe` and MediaInfo

Results of the corpus run whose pass bars `docs/design/text.md` fixed before
the run (commit `56eefe6`, first pushed as `0ca73b4`
before sign-off lines were added). Counts and manifest metadata only; the reports here hold file hashes, outcomes and
known-divergence ids, no media content.

## Method

- Binaries: `ffmpeg` and `ffprobe` 7.0.2 and MediaInfo 26.10 (the pinned
  oracles), and `coreav` built from the commit that adds this file.
- Inputs: every corpus v1 file tagged `c:isobmff` and `subtitle-track` or
  `chapters` (112 files, 22.6 GB: 34 `real`, 78 `edge`), downloaded from R2
  with `scripts/corpus_subset.py OUT --all c:isobmff --any subtitle-track
  chapters`.
- `scripts/text_check.py --coreav target/release/coreav --media OUT
  --mediainfo mediainfo` runs every check of the design note and applies
  `docs/known-divergences-text.md`. A crash check ran `coreav chapters` and
  `coreav extract-subtitles` (default track) on all 112 files
  ([`text-corpus-v1.smoke.txt`](text-corpus-v1.smoke.txt): file, exit
  codes).
- No held-out split: no performance claim is made, and the text tracks of
  the selected files are all there are in corpus v1.

## Results

As of the recheck for an earlier change (below; the run of in
brackets where it differs):

| Check | Items | Pass (direct + via KD) | Bar |
|---|---|---|---|
| Subtitle streams `ffprobe` reads as `mov_text` | 61 [59] | 61/61, 100% (25 + 36) [59/59 (25 + 34)] | at least 98% |
| WebVTT outputs passing the strict check (and `ffprobe`, MediaInfo re-probe) | 60 [58] | 60/60, 100% | 100% |
| Files where `ffprobe` or CoreAV finds chapters | 35 | 35/35, 100% (17 + 18) | at least 98% |
| Crashes, panics or timeouts, 112 files × 2 commands | 224 | 0 | 0 |
| Fixtures (CI) | 7 streams, 5 files | 100% | 100% |

One stream (an edit at rate 1.67) is refused (`KD-T-EDIT-RATE`), so it has
no output. `coreav probe` rejects 6 [8] of the 112 files with subtitles or
chapters (5 truncated, 1 [3] invalid structure; probe's own oracle covers
them), so they are outside the denominators, as the design note fixes.

Known divergences used (`docs/known-divergences-text.md`): subtitles
`KD-T-EDIT-ALIGN` 16 (15 in one file), `KD-T-LAST-END` 6, `KD-T-EMPTY` 6
[5], `KD-T-CR` 6 [5], `KD-T-ZERO-END` 5, `KD-T-STYLE` 4, `KD-T-ESCAPE` 4
[3], `KD-T-INVALID` 4 [3], `KD-T-LEAD-SPACE` 3 [`KD-T-QT-STYLE` 2],
`KD-T-ROUND` 1, `KD-T-EDIT-RATE` 1, `KD-T-MI-HOURS` 1; chapters `KD-T-CHPL`
10, `KD-T-CHAP-ORDER` 7, `KD-T-HMMT` 1.

## Changes after the first run

The first run ([`text-corpus-v1.run-01.json.gz`](text-corpus-v1.run-01.json.gz))
passed 25 of 59 streams and 27 of 35 chapter files. Every failure was
investigated; the bars did not change:

- Code: text now ends at its first NUL (DJI drone files store a
  NUL-terminated string in a fixed 254-byte field with stale bytes after
  it), and a sample whose text is only white space gives no cue
  (`docs/design/text.md` choice 8).
- The other differences follow from the specifications or the design note
  and are documented with predicates: `ffmpeg` aligns an edit to a sample
  boundary, extends the last cue to the end of the file, writes cues of no
  duration or no text, drops CR line breaks and samples it cannot decode,
  turns styling into tags; `ffprobe` reads Nero `chpl` and GoPro `HMMT`
  boxes and picks another of several chapter tracks.

The independent review of the pull request then made two of the checks
stricter: `KD-T-CHAP-ORDER` also requires CoreAV's chapters to equal its
track's samples, read independently from the sample tables, and `KD-T-CR`
requires a CR in the source samples. It also restored probe's handling of a
defect after the media box of a dropped track (the track is dropped, as
before, rather than the file rejected). The final run
([`text-corpus-v1.final.json.gz`](text-corpus-v1.final.json.gz)) re-checked
all 112 files with the final binary and the stricter checks, not only the
affected ones (the whole run takes about 12 s); the counts are unchanged.

## Recheck for an earlier change

Re-running the check on the same 112 files gave 59 of 61 streams: two more
were judged, and both failed. `coreav probe` now reads QuickTime compressed
movie boxes (`cmov`, `docs/design/compressed-movie.md`), so two
files it had rejected, 1660aab92b19 and 5f0f40c9396c, entered the
denominators with their QuickTime text tracks. The cause was the oracle,
not the writer:

- **`text_check.py` read the compressed movie as empty.** Its own box
  reader saw only the `cmov` box, so every predicate that reads the
  source's tracks found nothing and no entry applied. It now inflates the
  movie (`scripts/track_boxes.py`, `movie_payload`). With the tracks read,
  1660aab92b19's line breaks are `KD-T-CR` (its samples hold CR), and
  5f0f40c9396c's three Mac OS Roman cues are `KD-T-INVALID` (with
  `KD-T-EMPTY`, the cue counts become equal).
- **A real `ffmpeg` difference, misattributed.** 5f0f40c9396c's last two
  differences are cues whose text starts with a space, which `ffmpeg`
  drops. `KD-T-QT-STYLE` named QuickTime `styl` atoms as the cause and
  required them; this track has none. A `tx3g` and a `text` track written
  by `ffmpeg` with one cue's first letter made a space lose it as well, so
  the cause is the leading space, whatever the track. `KD-T-LEAD-SPACE`
  replaces it: the same rewrite, covering a cue only when it is, spaces
  and times included, one of the source's own samples, read from its
  sample tables and edit list. It covers this stream and the two
  `KD-T-QT-STYLE` covered.

The recheck ([`text-corpus-v1.recheck-334.json.gz`](text-corpus-v1.recheck-334.json.gz),
crash check [`text-corpus-v1.recheck-334.smoke.txt`](text-corpus-v1.recheck-334.smoke.txt))
used `ffmpeg`, `ffprobe` 7.0.2 and MediaInfo 26.10 with `coreav` built from
`main`. With `main`'s oracle on the same files and binary, 59 of 61 streams
pass, the two above failing; with this change's, 61 of 61. The chapter
results are the same with both. The crash check's exit codes differ from
the first run's only for those two files, which both commands now read (0
and 0, where they exited 1). The text mutation audit on the three streams
that reach `KD-T-LEAD-SPACE` catches all 26 changes and covers none.

## Informational: time and memory

Not gated and not a claim (no bar, warm page cache, one machine). Median wall
time per subtitle stream, the 59 streams above: `ffmpeg -map 0:N -c:s
webvtt` 10.6 ms, `coreav extract-subtitles` 1.1 ms. Peak resident memory on
the four largest files with subtitles (0.9 to 3.3 GB): `coreav` 2.9 to 3.1
MB, `ffmpeg` 29 to 37 MB; CoreAV reads only the text track's tables and
samples.

## Spend

0 USD: the files came from R2 (no egress charge; 112 reads) to the
development machine; no cloud host or other paid compute.

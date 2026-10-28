# Results, 2026-10-15

Informational only: nothing here gates, and no oracle agreement
rate or threshold changes. Binaries: MediaInfo CLI 26.10, `ffprobe` 7.0.2, the
`coreav-isobmff` adapter of `main` at `dd853a6` (its SHA-256 is in each
report's provenance line). Run locally, at no cost.

| File | Inputs | Files |
|---|---|---|
| `fixtures-2026-10-15.{md,json}` | generated fixtures, names and values shown | 38 |
| `corpus-v1-development-2026-10-15.md` | corpus v1 `edge` files and non-holdout `real` files (the split the tables were developed on) | 1,619 edge + 945 real |
| `corpus-v1-holdout-2026-10-15.md` | corpus v1 holdout `real` files, run once after the tables were frozen at `21d52b4` | 407 |

Corpus reports hold counts and generic enumerated values only, never file
names. Their JSON twins stay with the corpus, which
is not redistributed.

Provenance notes:

- All three reports were regenerated from the same captures and the same
  frozen tables after a review fix to the report classes (`mediainfo-split`,
  commit after `a56c128`); no item moved, the reports only gained an empty
  column.

- The development captures were started before provenance recording existed,
  with the code of `3017ff8`; the provenance file was written after the run
  and says so. Normalized captures do not depend on the later fixes, which
  changed only the comparison and report code, and the mapping tables are
  applied at report time.
- The holdout provenance reads `21d52b4…-dirty`. The only modified tracked
  file was a Python bytecode cache committed by mistake (removed in the next
  commit); the recorded SHA-256 of `mediainfo_oracle.py` and `corpus.py`
  equals those files at `21d52b4`.

## What the holdout report says (as measured)

- **Probe success.** CoreAV fails 107 of 407 files; 104 of them are WebM,
  Matroska, MP3 or Ogg, outside an MP4/MOV parser's scope; 3 are ISO base
  media files. MediaInfo reads all 107, like `ffprobe`; it flags truncation on
  1 and conformance errors on 2.
- **Colour.** On the 32 to 48 colour disagreements per field where MediaInfo
  agrees with `ffprobe` (holdout), the dominant pattern (also in the development split) is CoreAV
  reporting `null` where both oracles read a value, mostly `bt709`, `tv` or
  `smpte170m`. These are the first CoreAV-alone candidates to triage; a
  likely cause, not yet checked, is colour signalled only inside the video
  bitstream rather than in a `colr` box. On 5 to 14 per field MediaInfo sides with CoreAV
  (for example `ffprobe` `smpte170m` transfer where CoreAV and MediaInfo read
  `bt709`).
- **Format duration.** Of 171 CoreAV and `ffprobe` disagreements, MediaInfo
  agrees with CoreAV on 67, with `ffprobe` on 4, and on 99 the two differ by
  less than MediaInfo's 1 ms resolution, so MediaInfo cannot decide.
- **Stream count.** Of 58 disagreements, MediaInfo agrees with `ffprobe` on 54
  (CoreAV drops streams with codecs outside its allowlist) and
  with CoreAV on 2.
- **Width and height.** All 3 and 5 disagreements: MediaInfo agrees with
  CoreAV.
- **Language** (informational). CoreAV reports `null` where `ffprobe` and
  MediaInfo agree on a language in about a quarter of streams (also the most
  frequent development-split pattern, 278 streams of real files).

## Limitations

- MediaInfo prints durations in milliseconds; differences below 1 ms are
  `mediainfo-between`, not evidence for either side.
- MediaInfo prints a pixel aspect ratio of 1.000 where `ffprobe` reports none,
  so its aspect-ratio agreement is low by convention, not by error; both are
  informational fields.
- MediaInfo's stream `Duration` is a presentation duration and can include an
  empty edit (`edit_list.mp4`); its `Source_Duration` is reported as a second
  reading. Stream-duration verdicts against MediaInfo are therefore weaker
  evidence than the other fields.
- MediaInfo names outside the tables (timecode tracks, metadata tracks, rare
  codecs) count as disagreement and are listed in each report.
- MediaInfo crashed with a segmentation fault on 8 development `edge` files
  (failure label `exit--11` in that report, `signal-11` in the current code)
  and did not recognize 7.
- Timescale and handler name are not observable from MediaInfo, and there is
  no packet-level second oracle.

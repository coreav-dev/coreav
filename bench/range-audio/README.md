# Audio extraction over range reads

The measurement of `docs/design/range-extraction.md`. The bars and the
method are in that note ("Bars"), fixed before the held-out run. This
directory holds the drivers and the evidence.

| File | What |
|---|---|
| `sets.py` | Draws the held-out set (60 objects with an AAC track, from corpus v1's held-out objects, new salt) and the tuning pool from corpus v1's manifest |
| `run.py` | Local: `coreav extract-audio URL` (`--engines cli`, the default) and FFmpeg on a presigned R2 URL, each output checked against the native build's (`tools/edge-native`) |
| `summarize.py` | The bars' statistics |
| `gaps.py` | The gaps between the regions extraction advises, from ffprobe's packet list (black box) |

The helpers they share with the other benchmarks are in `bench/shared`.

The lists of the objects measured and the runs' per-object records name
each object's place in corpus v1, which is not redistributed, so they are
kept with the corpus.
The summaries here name each object by its SHA-256. `run.py` and `gaps.py`
take a list of your own objects, `{"files": [{"sha256", "key", "size"}]}`,
with `key` an object key in your bucket; `summarize.py` takes
`{"held_out": [...]}`, the same entries each with its `stratum`
(`audio-only`, `video-small` or `video-large`), and `run.py --held-out`
reads that list too.

Credentials come from the environment (`R2_ACCESS_KEY_ID`,
`R2_SECRET_ACCESS_KEY`, `R2_ENDPOINT`).
Presigned URLs are never written to the evidence.

## Tuning

The gaps between the advised regions (`gaps.py`; regions are the copy
window's reads, so a gap is what the window does not reach):

| File | Regions | Their share of the object | Gap p10 / p50 / p90 |
|---|---:|---:|---|
| MP4, H.264, 373 MiB | 30 | 0.5% | 10.8 / 12.1 / 13.6 MiB |
| MP4, H.264, 1,645 MiB | 1,573 | 6.0% | 0.46 / 1.0 / 1.39 MiB |
| MP4, H.264, 2,491 MiB | 21,242 | 53% | 0.02 / 0.04 / 0.07 MiB |
| MP4, H.264, 113 MiB | 54 | 3.0% | 1.93 / 2.0 / 2.09 MiB |
| MP4, H.264, 1,239 MiB | 1,408 | 7.1% | 0.29 / 0.79 / 1.35 MiB |
| MP4, H.264, 45 MiB | 22 | 3.0% | 1.88 / 1.89 / 1.89 MiB |
| MOV, HEVC, 28 MiB | 6 | 1.3% | 2.68 / 3.17 / 3.38 MiB |
| MP4, H.264, 18 MiB | 256 | 88% | 0.00 / 0.01 / 0.02 MiB |
| M4B, audio only, 235 MiB | 1 | 98% | (none) |
| M4A, audio only, 2 MiB | 1 | 100% | (none) |

What this decided:
- **The gap.** A fixed 2 MiB loses where the gaps are 2 MiB. At this host's 275 ms round trip, gaps pay from
  about 4 MB. The gap is now set per session from the first
  response's wait (design note, "The gap threshold").
- **Where the gain is.** Files whose audio chunks lie far apart, as in the
  373 MiB file (gaps of 12 MiB), skip all but half a
  percent of the object. Files whose audio chunks lie a megabyte or less
  apart (the 1,239, 1,645 and 2,491 MiB files) are
  read through at this round trip, as before; skipping them would take
  more requests than it saves.

## Held-out runs

| Run | Engines | Commit | Result |
|---|---|---|---|
| `cli-01` | CLI, FFmpeg | `2fa4878` (`measured/range-audio-cli`) | E-1 and E-3 met: 0.725 (0.612 to 0.806), every class at most 1.00 |
| `final-01` | CLI, FFmpeg | `c070360` (rebased onto `main`) | E-1 and E-3 met: CLI 0.777 (0.618 to 0.829), every class at most 1.00; decides the bars |

Summaries: `evidence/<run>/summary.json`, the CLI's part of
`summarize.py`'s output for the run; each run's
`meta.json` is beside it.

## Spend

R2 Class B requests within the free tier (`cli-01`: 4,752 requests of
the readers, and FFmpeg's; `final-01`: 10,840). Total: 0 USD.

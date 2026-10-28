# Corpus v1, holdout split (real, holdout): CoreAV, ffprobe, MediaInfo (2026-10-15, tables frozen at 21d52b4 before this run)

Selected manifest entries: 407; captured: 407; missing: 0.

Capture provenance: `{"binaries": {"coreav_oracle_sha256": "f91534ef70fe41da96bb91dcef6da4c8a5d317b660866b254632cf707d333767", "ffprobe": "ffprobe version 7.0.2-static https://johnvansickle.com/ffmpeg/  Copyright (c) 2007-2024 the FFmpeg developers", "isobmff_adapter_sha256": "688041002db3771b58a42b68192a37e45c5b8681a813f86a8e3ae0ead3f73e05", "mediainfo": "MediaInfoLib - v26.10"}, "tool": {"corpus_py_sha256": "8f52d3a50164610c3cfa43e6086c230207a49b80be0c43c98c408be1f83a97a4", "git": "21d52b4b43dc-dirty", "mediainfo_oracle_py_sha256": "5084aff7afc6789df1d9e4c2e173f9a546fbb16013a28a2df28276d3f9e3cec0"}}`.

## Split `holdout`, all categories

Files: 407.

### Probe outcomes

| CoreAV, ffprobe, MediaInfo | Files |
|---|---|
| coreav fail, ffprobe ok, mediainfo ok | 107 |
| coreav ok, ffprobe ok, mediainfo ok | 300 |

CoreAV rejects, ffprobe reads: 107 files; MediaInfo flags truncation on 1, conformance errors on 2, fails on 0.

### CoreAV and ffprobe disagreements on gating fields, by MediaInfo reading

| Field | MediaInfo agrees with CoreAV | MediaInfo agrees with ffprobe | MediaInfo within tolerance of both | MediaInfo differs from both | MediaInfo has no value |
|---|---|---|---|---|---|
| `probe_ok` | 0 | 107 | 0 | 0 | 0 |
| `stream_count` | 2 | 54 | 0 | 2 | 0 |
| `format_duration` | 67 | 4 | 99 | 1 | 0 |
| `width` | 3 | 0 | 0 | 0 | 0 |
| `height` | 5 | 0 | 0 | 0 | 0 |
| `duration` | 4 | 5 | 8 | 3 | 0 |
| `color_primaries` | 5 | 32 | 0 | 0 | 0 |
| `color_transfer` | 14 | 34 | 0 | 0 | 0 |
| `color_matrix` | 5 | 34 | 1 | 0 | 0 |
| `color_range` | 7 | 48 | 0 | 0 | 0 |

### Pairwise agreement per field

| Field | Gating | CoreAV = ffprobe | MediaInfo = CoreAV | MediaInfo = ffprobe |
|---|---|---|---|---|
| `probe_ok` | yes | 300/407 (73.7%) | 300/407 (73.7%) | 407/407 (100.0%) |
| `stream_count` | yes | 242/300 (80.7%) | 244/300 (81.3%) | 368/407 (90.4%) |
| `format_duration` | yes | 129/300 (43.0%) | 295/300 (98.3%) | 321/407 (78.9%) |
| `kind` | yes | 477/477 (100.0%) | 477/477 (100.0%) | 684/688 (99.4%) |
| `codec_fourcc` | yes | 477/477 (100.0%) | 449/477 (94.1%) | 650/688 (94.5%) |
| `codec_name` | yes | 477/477 (100.0%) | 449/477 (94.1%) | 616/688 (89.5%) |
| `width` | yes | 474/477 (99.4%) | 477/477 (100.0%) | 661/688 (96.1%) |
| `height` | yes | 472/477 (99.0%) | 477/477 (100.0%) | 659/688 (95.8%) |
| `rotation` | yes | 477/477 (100.0%) | 477/477 (100.0%) | 688/688 (100.0%) |
| `duration` | yes | 457/477 (95.8%) | 402/477 (84.3%) | 469/688 (68.2%) |
| `color_primaries` | yes | 440/477 (92.2%) | 444/477 (93.1%) | 681/688 (99.0%) |
| `color_transfer` | yes | 429/477 (89.9%) | 442/477 (92.7%) | 672/688 (97.7%) |
| `color_matrix` | yes | 437/477 (91.6%) | 442/477 (92.7%) | 678/688 (98.5%) |
| `color_range` | yes | 422/477 (88.5%) | 429/477 (89.9%) | 673/688 (97.8%) |
| `sample_rate` | no | 473/477 (99.2%) | 473/477 (99.2%) | 688/688 (100.0%) |
| `channels` | no | 464/477 (97.3%) | 464/477 (97.3%) | 688/688 (100.0%) |
| `language` | no | 356/477 (74.6%) | 356/477 (74.6%) | 685/688 (99.6%) |
| `sample_aspect_ratio` | no | 420/477 (88.1%) | 328/477 (68.8%) | 586/688 (85.2%) |
| `display_aspect_ratio` | no | 419/477 (87.8%) | 310/477 (65.0%) | 566/688 (82.3%) |

### Three-way classes per field

| Field | all-agree | coreav-alone | ffprobe-alone | mediainfo-alone | mediainfo-between | mediainfo-split | all-differ | not-comparable |
|---|---|---|---|---|---|---|---|---|
| `probe_ok` | 300 | 107 | 0 | 0 | 0 | 0 | 0 | 0 |
| `stream_count` | 242 | 54 | 2 | 0 | 0 | 0 | 2 | 107 |
| `format_duration` | 129 | 4 | 67 | 0 | 99 | 0 | 1 | 107 |
| `kind` | 477 | 0 | 0 | 0 | 0 | 0 | 0 | 275 |
| `codec_fourcc` | 449 | 0 | 0 | 28 | 0 | 0 | 0 | 275 |
| `codec_name` | 449 | 0 | 0 | 28 | 0 | 0 | 0 | 275 |
| `width` | 474 | 0 | 3 | 0 | 0 | 0 | 0 | 275 |
| `height` | 472 | 0 | 5 | 0 | 0 | 0 | 0 | 275 |
| `rotation` | 477 | 0 | 0 | 0 | 0 | 0 | 0 | 275 |
| `duration` | 390 | 5 | 4 | 67 | 8 | 0 | 3 | 275 |
| `color_primaries` | 439 | 32 | 5 | 1 | 0 | 0 | 0 | 275 |
| `color_transfer` | 428 | 34 | 14 | 1 | 0 | 0 | 0 | 275 |
| `color_matrix` | 436 | 34 | 5 | 1 | 1 | 0 | 0 | 275 |
| `color_range` | 422 | 48 | 7 | 0 | 0 | 0 | 0 | 275 |
| `sample_rate` | 473 | 4 | 0 | 0 | 0 | 0 | 0 | 275 |
| `channels` | 464 | 13 | 0 | 0 | 0 | 0 | 0 | 275 |
| `language` | 356 | 121 | 0 | 0 | 0 | 0 | 0 | 275 |
| `sample_aspect_ratio` | 328 | 57 | 0 | 92 | 0 | 0 | 0 | 275 |
| `display_aspect_ratio` | 309 | 56 | 1 | 110 | 0 | 0 | 1 | 275 |

### Most frequent disagreement patterns (enumerated fields)

- `probe_ok`
  - 107 × coreav=False ffprobe=True mediainfo=True
- `language`
  - 119 × coreav=None ffprobe=eng mediainfo=eng
  - 2 × coreav=\u007f\u007f\u007f ffprobe=None mediainfo=None
- `color_primaries`
  - 29 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 4 × coreav=bt709 ffprobe=bt470bg mediainfo=bt709
  - 2 × coreav=None ffprobe=smpte170m mediainfo=smpte170m
  - 1 × coreav=None ffprobe=bt470bg mediainfo=bt470bg
  - 1 × coreav=None ffprobe=None mediainfo=bt709
  - 1 × coreav=None ffprobe=bt470bg mediainfo=None
- `color_transfer`
  - 32 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 13 × coreav=bt709 ffprobe=smpte170m mediainfo=bt709
  - 1 × coreav=None ffprobe=bt470m mediainfo=bt470m
  - 1 × coreav=None ffprobe=None mediainfo=bt709
  - 1 × coreav=None ffprobe=smpte170m mediainfo=None
  - 1 × coreav=gamma22 ffprobe=bt470m mediainfo=bt470m
- `color_matrix`
  - 29 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 4 × coreav=bt709 ffprobe=bt470bg mediainfo=bt709
  - 3 × coreav=None ffprobe=smpte170m mediainfo=bt601
  - 2 × coreav=None ffprobe=bt470bg mediainfo=bt601
  - 1 × coreav=None ffprobe=None mediainfo=bt709
  - 1 × coreav=None ffprobe=bt470bg mediainfo=None
  - 1 × coreav=smpte170m ffprobe=bt470bg mediainfo=bt601
- `color_range`
  - 35 × coreav=None ffprobe=tv mediainfo=tv
  - 13 × coreav=None ffprobe=pc mediainfo=pc
  - 4 × coreav=None ffprobe=tv mediainfo=None
  - 3 × coreav=tv ffprobe=pc mediainfo=tv
- `codec_fourcc`
  - 28 × coreav=tmcd ffprobe=tmcd mediainfo=None
- `codec_name`
  - 28 × coreav=None ffprobe=None mediainfo=mi:QuickTime TC

Stream duration disagreements where MediaInfo also printed `Source_Duration` (class: sides matching that second reading):

- all-differ:coreav+ffprobe: 2
- all-differ:ffprobe: 1
- coreav-alone:coreav: 2
- coreav-alone:none: 2
- ffprobe-alone:ffprobe: 2
- mediainfo-alone:coreav+ffprobe: 7
- mediainfo-alone:none: 1

### MediaInfo values outside the mapping tables

- codec_name=mi:QuickTime TC: 28
- codec_name=mi:gpmd: 9
- codec_name=mi:Timed Metadata Sample: 6
- codec_name=mi:Sony Real Time Metadata: 5
- codec_name=mi:mett-application/octet-stream: 5
- kind=mi:Menu: 4
- codec_name=mi:RTP: 4
- codec_name=mi:fdsc: 3
- codec_name=mi:ADPCM: 2
- codec_name=mi:rtsp: 1
- codec_name=mi:WMA: 1
- codec_name=mi:RLE: 1
- codec_name=mi:MPEG Video: 1
- codec_name=mi:System Core: 1
- codec_name=mi:System: 1

Not observable from MediaInfo: timescale, handler_name.

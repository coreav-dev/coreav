# Corpus v1, development split (edge + non-holdout real): CoreAV, ffprobe, MediaInfo (2026-10-15, tables of 21d52b4)

Selected manifest entries: 2564; captured: 2564; missing: 0.

Capture provenance: `{"binaries": {"coreav_oracle_sha256": "f91534ef70fe41da96bb91dcef6da4c8a5d317b660866b254632cf707d333767", "ffprobe": "ffprobe version 7.0.2-static https://johnvansickle.com/ffmpeg/  Copyright (c) 2007-2024 the FFmpeg developers", "isobmff_adapter_sha256": "688041002db3771b58a42b68192a37e45c5b8681a813f86a8e3ae0ead3f73e05", "mediainfo": "MediaInfoLib - v26.10"}, "tool": {"git": "3017ff8", "note": "recorded after the run: this run started before provenance recording, with the code of commit 3017ff8 except two edits that do not change captures other than one failure label (exit--11 instead of signal-11)"}}`.

## Split `tuning`, all categories

Files: 945.

### Probe outcomes

| CoreAV, ffprobe, MediaInfo | Files |
|---|---|
| coreav fail, ffprobe ok, mediainfo ok | 204 |
| coreav ok, ffprobe ok, mediainfo ok | 741 |

CoreAV rejects, ffprobe reads: 204 files; MediaInfo flags truncation on 4, conformance errors on 6, fails on 0.

### CoreAV and ffprobe disagreements on gating fields, by MediaInfo reading

| Field | MediaInfo agrees with CoreAV | MediaInfo agrees with ffprobe | MediaInfo within tolerance of both | MediaInfo differs from both | MediaInfo has no value |
|---|---|---|---|---|---|
| `probe_ok` | 0 | 204 | 0 | 0 | 0 |
| `stream_count` | 5 | 166 | 0 | 7 | 0 |
| `format_duration` | 125 | 8 | 244 | 2 | 0 |
| `kind` | 2 | 0 | 0 | 0 | 0 |
| `codec_name` | 2 | 0 | 0 | 0 | 0 |
| `width` | 3 | 0 | 0 | 0 | 0 |
| `height` | 5 | 0 | 0 | 0 | 0 |
| `duration` | 23 | 8 | 20 | 10 | 0 |
| `color_primaries` | 12 | 82 | 0 | 0 | 0 |
| `color_transfer` | 32 | 82 | 0 | 0 | 0 |
| `color_matrix` | 11 | 83 | 2 | 0 | 0 |
| `color_range` | 31 | 129 | 0 | 0 | 0 |

### Pairwise agreement per field

| Field | Gating | CoreAV = ffprobe | MediaInfo = CoreAV | MediaInfo = ffprobe |
|---|---|---|---|---|
| `probe_ok` | yes | 741/945 (78.4%) | 741/945 (78.4%) | 945/945 (100.0%) |
| `stream_count` | yes | 563/741 (76.0%) | 568/741 (76.7%) | 875/945 (92.6%) |
| `format_duration` | yes | 362/741 (48.9%) | 731/741 (98.7%) | 785/945 (83.1%) |
| `kind` | yes | 1142/1144 (99.8%) | 1144/1144 (100.0%) | 1646/1660 (99.2%) |
| `codec_fourcc` | yes | 1144/1144 (100.0%) | 1059/1144 (92.6%) | 1545/1660 (93.1%) |
| `codec_name` | yes | 1142/1144 (99.8%) | 1060/1144 (92.7%) | 1421/1660 (85.6%) |
| `width` | yes | 1141/1144 (99.7%) | 1144/1144 (100.0%) | 1651/1660 (99.5%) |
| `height` | yes | 1139/1144 (99.6%) | 1144/1144 (100.0%) | 1650/1660 (99.4%) |
| `rotation` | yes | 1144/1144 (100.0%) | 1144/1144 (100.0%) | 1660/1660 (100.0%) |
| `duration` | yes | 1083/1144 (94.7%) | 1010/1144 (88.3%) | 1209/1660 (72.8%) |
| `color_primaries` | yes | 1050/1144 (91.8%) | 1058/1144 (92.5%) | 1643/1660 (99.0%) |
| `color_transfer` | yes | 1030/1144 (90.0%) | 1058/1144 (92.5%) | 1623/1660 (97.8%) |
| `color_matrix` | yes | 1048/1144 (91.6%) | 1057/1144 (92.4%) | 1640/1660 (98.8%) |
| `color_range` | yes | 984/1144 (86.0%) | 1015/1144 (88.7%) | 1606/1660 (96.7%) |
| `sample_rate` | no | 1137/1144 (99.4%) | 1137/1144 (99.4%) | 1660/1660 (100.0%) |
| `channels` | no | 1109/1144 (96.9%) | 1109/1144 (96.9%) | 1657/1660 (99.8%) |
| `language` | no | 862/1144 (75.3%) | 862/1144 (75.3%) | 1651/1660 (99.5%) |
| `sample_aspect_ratio` | no | 981/1144 (85.8%) | 757/1144 (66.2%) | 1418/1660 (85.4%) |
| `display_aspect_ratio` | no | 979/1144 (85.6%) | 722/1144 (63.1%) | 1378/1660 (83.0%) |

### Three-way classes per field

| Field | all-agree | coreav-alone | ffprobe-alone | mediainfo-alone | mediainfo-between | mediainfo-split | all-differ | not-comparable |
|---|---|---|---|---|---|---|---|---|
| `probe_ok` | 741 | 204 | 0 | 0 | 0 | 0 | 0 | 0 |
| `stream_count` | 563 | 166 | 5 | 0 | 0 | 0 | 7 | 204 |
| `format_duration` | 362 | 8 | 125 | 0 | 244 | 0 | 2 | 204 |
| `kind` | 1142 | 0 | 2 | 0 | 0 | 0 | 0 | 612 |
| `codec_fourcc` | 1059 | 0 | 0 | 85 | 0 | 0 | 0 | 612 |
| `codec_name` | 1058 | 0 | 2 | 84 | 0 | 0 | 0 | 612 |
| `width` | 1141 | 0 | 3 | 0 | 0 | 0 | 0 | 612 |
| `height` | 1139 | 0 | 5 | 0 | 0 | 0 | 0 | 612 |
| `rotation` | 1144 | 0 | 0 | 0 | 0 | 0 | 0 | 612 |
| `duration` | 967 | 8 | 23 | 116 | 20 | 0 | 10 | 612 |
| `color_primaries` | 1046 | 82 | 12 | 4 | 0 | 0 | 0 | 612 |
| `color_transfer` | 1026 | 82 | 32 | 4 | 0 | 0 | 0 | 612 |
| `color_matrix` | 1044 | 83 | 11 | 4 | 2 | 0 | 0 | 612 |
| `color_range` | 984 | 129 | 31 | 0 | 0 | 0 | 0 | 612 |
| `sample_rate` | 1137 | 7 | 0 | 0 | 0 | 0 | 0 | 612 |
| `channels` | 1109 | 35 | 0 | 0 | 0 | 0 | 0 | 612 |
| `language` | 862 | 282 | 0 | 0 | 0 | 0 | 0 | 612 |
| `sample_aspect_ratio` | 757 | 163 | 0 | 224 | 0 | 0 | 0 | 612 |
| `display_aspect_ratio` | 720 | 162 | 1 | 259 | 1 | 0 | 1 | 612 |

### Most frequent disagreement patterns (enumerated fields)

- `probe_ok`
  - 204 × coreav=False ffprobe=True mediainfo=True
- `language`
  - 278 × coreav=None ffprobe=eng mediainfo=eng
  - 4 × coreav=\u007f\u007f\u007f ffprobe=None mediainfo=None
- `kind`
  - 2 × coreav=subtitle ffprobe=data mediainfo=subtitle
- `codec_fourcc`
  - 83 × coreav=tmcd ffprobe=tmcd mediainfo=None
  - 2 × coreav=tx3g ffprobe=tx3g mediainfo=None
- `codec_name`
  - 83 × coreav=None ffprobe=None mediainfo=mi:QuickTime TC
  - 2 × coreav=mov_text ffprobe=bin_data mediainfo=mov_text
  - 1 × coreav=mp3 ffprobe=mp3 mediainfo=mp2
- `color_range`
  - 104 × coreav=None ffprobe=tv mediainfo=tv
  - 25 × coreav=None ffprobe=pc mediainfo=pc
  - 17 × coreav=None ffprobe=tv mediainfo=None
  - 14 × coreav=tv ffprobe=pc mediainfo=tv
- `color_primaries`
  - 74 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 10 × coreav=bt709 ffprobe=bt470bg mediainfo=bt709
  - 7 × coreav=None ffprobe=smpte170m mediainfo=smpte170m
  - 4 × coreav=None ffprobe=None mediainfo=bt709
  - 1 × coreav=smpte170m ffprobe=bt470bg mediainfo=smpte170m
  - 1 × coreav=None ffprobe=bt470bg mediainfo=bt470bg
  - 1 × coreav=bt709 ffprobe=smpte170m mediainfo=bt709
- `color_transfer`
  - 77 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 32 × coreav=bt709 ffprobe=smpte170m mediainfo=bt709
  - 4 × coreav=None ffprobe=None mediainfo=bt709
  - 3 × coreav=None ffprobe=iec61966-2-4 mediainfo=iec61966-2-4
  - 1 × coreav=None ffprobe=smpte170m mediainfo=smpte170m
  - 1 × coreav=None ffprobe=iec61966-2-1 mediainfo=iec61966-2-1
- `color_matrix`
  - 73 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 9 × coreav=bt709 ffprobe=bt470bg mediainfo=bt709
  - 7 × coreav=None ffprobe=smpte170m mediainfo=bt601
  - 4 × coreav=None ffprobe=None mediainfo=bt709
  - 3 × coreav=None ffprobe=bt470bg mediainfo=bt601
  - 2 × coreav=smpte170m ffprobe=bt470bg mediainfo=bt601
  - 2 × coreav=bt709 ffprobe=smpte170m mediainfo=bt709

Stream duration disagreements where MediaInfo also printed `Source_Duration` (class: sides matching that second reading):

- all-differ:coreav: 1
- all-differ:coreav+ffprobe: 6
- all-differ:ffprobe: 3
- coreav-alone:coreav: 4
- coreav-alone:none: 3
- ffprobe-alone:ffprobe: 5
- mediainfo-alone:coreav+ffprobe: 16
- mediainfo-alone:none: 4
- mediainfo-between:coreav+ffprobe: 2

### MediaInfo values outside the mapping tables

- codec_name=mi:QuickTime TC: 83
- codec_name=mi:gpmd: 29
- codec_name=mi:Timed Metadata Sample: 29
- codec_name=mi:fdsc: 12
- codec_name=mi:Sony Real Time Metadata: 11
- codec_name=mi:RTP: 9
- kind=mi:Menu: 9
- codec_name=mi:System: 7
- codec_name=mi:System Core: 7
- codec_name=mi:cndm: 6
- codec_name=mi:AMR: 6
- codec_name=mi:mett-application/octet-stream: 4
- codec_name=mi:wvtt: 3
- codec_name=mi:ADPCM: 3
- codec_name=mi:priv: 2
- codec_name=mi:Ogg: 2
- codec_name=mi:EIA-608: 2
- codec_name=mi:rtsp: 1
- codec_name=mi:djmd: 1
- codec_name=mi:dbgi: 1
- codec_name=mi:RGB: 1
- codec_name=mi:MPEG Video: 1
- codec_name=mi:moov: 1
- codec_name=mi:MPEG Audio None: 1
- codec_name=mi:RLE: 1
- codec_name=mi:mett-application/microvideo-image-meta: 1
- codec_name=mi:D_WEBVTT/SUBTITLES: 1

Not observable from MediaInfo: timescale, handler_name.

## Split `edge`, all categories

Files: 1619.

### Probe outcomes

| CoreAV, ffprobe, MediaInfo | Files |
|---|---|
| coreav fail, ffprobe fail, mediainfo fail | 3 |
| coreav fail, ffprobe fail, mediainfo ok | 32 |
| coreav fail, ffprobe ok, mediainfo fail | 4 |
| coreav fail, ffprobe ok, mediainfo ok | 393 |
| coreav ok, ffprobe fail, mediainfo ok | 35 |
| coreav ok, ffprobe ok, mediainfo fail | 8 |
| coreav ok, ffprobe ok, mediainfo ok | 1144 |

CoreAV rejects, ffprobe reads: 397 files; MediaInfo flags truncation on 136, conformance errors on 148, fails on 4.

### CoreAV and ffprobe disagreements on gating fields, by MediaInfo reading

| Field | MediaInfo agrees with CoreAV | MediaInfo agrees with ffprobe | MediaInfo within tolerance of both | MediaInfo differs from both | MediaInfo has no value |
|---|---|---|---|---|---|
| `probe_ok` | 39 | 393 | 0 | 0 | 0 |
| `stream_count` | 9 | 688 | 0 | 13 | 8 |
| `format_duration` | 113 | 109 | 236 | 9 | 4 |
| `kind` | 7 | 17 | 0 | 2 | 3 |
| `codec_fourcc` | 2 | 0 | 0 | 0 | 0 |
| `codec_name` | 11 | 2 | 3 | 9 | 0 |
| `width` | 41 | 19 | 0 | 0 | 3 |
| `height` | 45 | 18 | 0 | 0 | 3 |
| `rotation` | 2 | 20 | 0 | 3 | 3 |
| `duration` | 41 | 13 | 3 | 10 | 3 |
| `color_primaries` | 2 | 55 | 0 | 0 | 2 |
| `color_transfer` | 5 | 54 | 0 | 0 | 2 |
| `color_matrix` | 2 | 54 | 1 | 2 | 2 |
| `color_range` | 46 | 93 | 0 | 4 | 3 |

### Pairwise agreement per field

| Field | Gating | CoreAV = ffprobe | MediaInfo = CoreAV | MediaInfo = ffprobe |
|---|---|---|---|---|
| `probe_ok` | yes | 1187/1619 (73.3%) | 1186/1619 (73.3%) | 1540/1619 (95.1%) |
| `stream_count` | yes | 434/1152 (37.7%) | 459/1179 (38.9%) | 1435/1537 (93.4%) |
| `format_duration` | yes | 681/1152 (59.1%) | 1046/1179 (88.7%) | 1352/1537 (88.0%) |
| `kind` | yes | 1041/1070 (97.3%) | 1070/1089 (98.3%) | 3086/3116 (99.0%) |
| `codec_fourcc` | yes | 1068/1070 (99.8%) | 1010/1089 (92.7%) | 2820/3116 (90.5%) |
| `codec_name` | yes | 1045/1070 (97.7%) | 980/1089 (90.0%) | 2231/3116 (71.6%) |
| `width` | yes | 1007/1070 (94.1%) | 1070/1089 (98.3%) | 2969/3116 (95.3%) |
| `height` | yes | 1004/1070 (93.8%) | 1071/1089 (98.3%) | 2958/3116 (94.9%) |
| `rotation` | yes | 1042/1070 (97.4%) | 1066/1089 (97.9%) | 3108/3116 (99.7%) |
| `duration` | yes | 1000/1070 (93.5%) | 976/1089 (89.6%) | 2201/3116 (70.6%) |
| `color_primaries` | yes | 1011/1070 (94.5%) | 1026/1089 (94.2%) | 3084/3116 (99.0%) |
| `color_transfer` | yes | 1009/1070 (94.3%) | 1026/1089 (94.2%) | 3079/3116 (98.8%) |
| `color_matrix` | yes | 1009/1070 (94.3%) | 1018/1089 (93.5%) | 2992/3116 (96.0%) |
| `color_range` | yes | 924/1070 (86.4%) | 979/1089 (89.9%) | 2826/3116 (90.7%) |
| `sample_rate` | no | 1046/1070 (97.8%) | 1057/1089 (97.1%) | 3099/3116 (99.5%) |
| `channels` | no | 1010/1070 (94.4%) | 1025/1089 (94.1%) | 3071/3116 (98.6%) |
| `language` | no | 637/1070 (59.5%) | 665/1089 (61.1%) | 3077/3116 (98.7%) |
| `sample_aspect_ratio` | no | 965/1070 (90.2%) | 772/1089 (70.9%) | 2502/3116 (80.3%) |
| `display_aspect_ratio` | no | 961/1070 (89.8%) | 764/1089 (70.2%) | 2486/3116 (79.8%) |

### Three-way classes per field

| Field | all-agree | coreav-alone | ffprobe-alone | mediainfo-alone | mediainfo-between | mediainfo-split | all-differ | not-comparable |
|---|---|---|---|---|---|---|---|---|
| `probe_ok` | 1147 | 393 | 39 | 40 | 0 | 0 | 0 | 0 |
| `stream_count` | 434 | 688 | 9 | 0 | 0 | 0 | 13 | 475 |
| `format_duration` | 667 | 109 | 113 | 10 | 236 | 0 | 9 | 475 |
| `kind` | 1037 | 17 | 7 | 0 | 0 | 0 | 2 | 2433 |
| `codec_fourcc` | 982 | 0 | 2 | 79 | 0 | 0 | 0 | 2433 |
| `codec_name` | 957 | 2 | 11 | 81 | 3 | 0 | 9 | 2433 |
| `width` | 1003 | 19 | 41 | 0 | 0 | 0 | 0 | 2433 |
| `height` | 1000 | 18 | 45 | 0 | 0 | 0 | 0 | 2433 |
| `rotation` | 1038 | 20 | 2 | 0 | 0 | 0 | 3 | 2433 |
| `duration` | 907 | 13 | 41 | 89 | 3 | 0 | 10 | 2433 |
| `color_primaries` | 999 | 55 | 2 | 7 | 0 | 0 | 0 | 2433 |
| `color_transfer` | 996 | 54 | 5 | 8 | 0 | 0 | 0 | 2433 |
| `color_matrix` | 990 | 54 | 2 | 14 | 1 | 0 | 2 | 2433 |
| `color_range` | 910 | 93 | 46 | 10 | 0 | 0 | 4 | 2433 |
| `sample_rate` | 1031 | 22 | 2 | 8 | 0 | 0 | 0 | 2433 |
| `channels` | 1001 | 53 | 7 | 2 | 0 | 0 | 0 | 2433 |
| `language` | 631 | 422 | 0 | 2 | 8 | 0 | 0 | 2433 |
| `sample_aspect_ratio` | 746 | 95 | 2 | 215 | 1 | 0 | 4 | 2433 |
| `display_aspect_ratio` | 735 | 96 | 6 | 222 | 0 | 0 | 4 | 2433 |

### Most frequent disagreement patterns (enumerated fields)

- `probe_ok`
  - 393 × coreav=False ffprobe=True mediainfo=True
  - 35 × coreav=True ffprobe=False mediainfo=True
  - 32 × coreav=False ffprobe=False mediainfo=True
  - 8 × coreav=True ffprobe=True mediainfo=False
  - 4 × coreav=False ffprobe=True mediainfo=False
- `language`
  - 405 × coreav=None ffprobe=eng mediainfo=eng
  - 8 × coreav=\u007f\u007f\u007f ffprobe=None mediainfo=None
  - 8 × coreav=und ffprobe=None mediainfo=None
  - 3 × coreav=None ffprobe=fra mediainfo=fra
  - 3 × coreav=None ffprobe=spa mediainfo=spa
  - 2 × coreav=None ffprobe=amh mediainfo=amh
  - 1 × coreav=und ffprobe=und mediainfo=eng
  - 1 × coreav=None ffprobe=ger mediainfo=deu
  - 1 × coreav=None ffprobe=None mediainfo=eng
- `kind`
  - 17 × coreav=data ffprobe=video mediainfo=video
  - 5 × coreav=subtitle ffprobe=data mediainfo=subtitle
  - 2 × coreav=subtitle ffprobe=data mediainfo=mi:Menu
  - 2 × coreav=video ffprobe=audio mediainfo=video
- `codec_fourcc`
  - 75 × coreav=tmcd ffprobe=tmcd mediainfo=None
  - 3 × coreav=tx3g ffprobe=tx3g mediainfo=None
  - 2 × coreav=avc1 ffprobe=None mediainfo=avc1
  - 1 × coreav=lpcm ffprobe=lpcm mediainfo=fl32
- `codec_name`
  - 75 × coreav=None ffprobe=None mediainfo=mi:QuickTime TC
  - 7 × coreav=mov_text ffprobe=bin_data mediainfo=mov_text
  - 6 × coreav=None ffprobe=rawvideo mediainfo=mi:RGB
  - 4 × coreav=mp3 ffprobe=mp3 mediainfo=mp2
  - 3 × coreav=aac ffprobe=mp3on4 mediainfo=mi:mp4a-40-29
  - 2 × coreav=aac ffprobe=mp4als mediainfo=mp4als
  - 2 × coreav=pcm_u8 ffprobe=pcm_s16be mediainfo=pcm
  - 1 × coreav=aac ffprobe=mp3 mediainfo=aac
  - 1 × coreav=mov_text ffprobe=None mediainfo=mov_text
  - 1 × coreav=aac ffprobe=aac mediainfo=mi:ER HVXC
  - 1 × coreav=h264 ffprobe=flac mediainfo=h264
  - 1 × coreav=aac ffprobe=aac mediainfo=mi:mp4a-40-95
  - 1 × coreav=h264 ffprobe=opus mediainfo=h264
  - 1 × coreav=pcm_f32le ffprobe=pcm_f32be mediainfo=pcm
- `color_range`
  - 80 × coreav=None ffprobe=tv mediainfo=tv
  - 42 × coreav=None ffprobe=tv mediainfo=None
  - 13 × coreav=None ffprobe=pc mediainfo=pc
  - 5 × coreav=None ffprobe=None mediainfo=tv
  - 4 × coreav=None ffprobe=tv mediainfo=pc
  - 3 × coreav=tv ffprobe=pc mediainfo=tv
  - 3 × coreav=tv ffprobe=tv mediainfo=pc
  - 1 × coreav=pc ffprobe=pc mediainfo=None
  - 1 × coreav=None ffprobe=pc mediainfo=None
  - 1 × coreav=tv ffprobe=tv mediainfo=None
- `color_primaries`
  - 26 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 22 × coreav=None ffprobe=smpte170m mediainfo=smpte170m
  - 5 × coreav=None ffprobe=None mediainfo=bt709
  - 3 × coreav=None ffprobe=bt2020 mediainfo=bt2020
  - 2 × coreav=None ffprobe=bt470bg mediainfo=bt470bg
  - 2 × coreav=None ffprobe=bt709 mediainfo=None
  - 1 × coreav=None ffprobe=None mediainfo=bt2020
  - 1 × coreav=None ffprobe=None mediainfo=smpte170m
  - 1 × coreav=jedec-p22 ffprobe=ebu3213 mediainfo=ebu3213
  - 1 × coreav=bt2020 ffprobe=bt709 mediainfo=bt709
- `color_transfer`
  - 37 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 9 × coreav=None ffprobe=iec61966-2-1 mediainfo=iec61966-2-1
  - 6 × coreav=None ffprobe=None mediainfo=bt709
  - 3 × coreav=None ffprobe=smpte2084 mediainfo=smpte2084
  - 2 × coreav=None ffprobe=bt709 mediainfo=None
  - 2 × coreav=None ffprobe=smpte170m mediainfo=smpte170m
  - 2 × coreav=bt709 ffprobe=smpte170m mediainfo=bt709
  - 1 × coreav=None ffprobe=bt470bg mediainfo=bt470bg
  - 1 × coreav=None ffprobe=None mediainfo=smpte2084
  - 1 × coreav=bt2020-10 ffprobe=bt2020-10 mediainfo=arib-std-b67
  - 1 × coreav=None ffprobe=bt470m mediainfo=None
  - 1 × coreav=bt2020-10 ffprobe=arib-std-b67 mediainfo=arib-std-b67
  - 1 × coreav=bt2020-10 ffprobe=bt709 mediainfo=bt709
- `color_matrix`
  - 24 × coreav=None ffprobe=smpte170m mediainfo=bt601
  - 23 × coreav=None ffprobe=bt709 mediainfo=bt709
  - 10 × coreav=None ffprobe=None mediainfo=bt709
  - 3 × coreav=None ffprobe=bt470bg mediainfo=bt601
  - 3 × coreav=None ffprobe=None mediainfo=bt601
  - 3 × coreav=None ffprobe=bt2020nc mediainfo=bt2020nc
  - 2 × coreav=None ffprobe=bt709 mediainfo=None
  - 2 × coreav=None ffprobe=bt709 mediainfo=gbr
  - 1 × coreav=None ffprobe=None mediainfo=mi:IPT-PQ-C2
  - 1 × coreav=smpte170m ffprobe=bt470bg mediainfo=bt601
  - 1 × coreav=bt2020nc ffprobe=bt709 mediainfo=bt709
- `rotation`
  - 20 × coreav=None ffprobe=0 mediainfo=0
  - 3 × coreav=None ffprobe=0 mediainfo=270
  - 2 × coreav=0 ffprobe=None mediainfo=0

Stream duration disagreements where MediaInfo also printed `Source_Duration` (class: sides matching that second reading):

- all-differ:coreav: 1
- all-differ:coreav+ffprobe: 2
- all-differ:ffprobe: 6
- all-differ:none: 1
- coreav-alone:coreav: 12
- ffprobe-alone:ffprobe: 13
- mediainfo-alone:coreav+ffprobe: 37
- mediainfo-alone:none: 1

### MediaInfo values outside the mapping tables

- codec_name=mi:QuickTime TC: 85
- codec_name=mi:System: 80
- codec_name=mi:RTP: 68
- codec_name=mi:System Core: 43
- codec_name=mi:ADPCM: 40
- codec_name=mi:hvt1: 36
- codec_name=mi:vvs1: 32
- codec_name=mi:AMR: 30
- codec_name=mi:RLE: 23
- codec_name=mi:MPEG Video: 22
- codec_name=mi:MPEG Audio None: 20
- codec_name=mi:Road Pizza: 18
- codec_name=mi:smc : 18
- codec_name=mi:YUV: 16
- kind=mi:Menu: 14
- codec_name=mi:EIA-608: 14
- codec_name=mi:RGB: 12
- codec_name=mi:Timed Metadata Sample: 10
- color_matrix=mi:IPT-PQ-C2: 9
- codec_name=mi:AAC scalable: 8
- codec_name=mi:WMA: 8
- codec_name=mi:USAC: 7
- codec_name=mi:DXV: 6
- codec_name=mi:SheerVideo: 6
- codec_name=mi:hvc2: 6
- codec_name=mi:it35: 6
- codec_name=mi:stpp: 5
- codec_name=mi:MPEG-H 3D Audio: 5
- codec_name=mi:CineForm: 5
- codec_name=mi:RealVideo 4: 5
- codec_name=mi:wvtt: 4
- codec_name=mi:UCOD: 4
- codec_name=mi:3IV1: 4
- codec_name=mi:IAMF: 4
- codec_name=mi:IV32: 4
- codec_name=mi:pxlt: 4
- codec_name=mi:m1s : 4
- codec_name=mi:mp4s-FF: 4
- codec_name=mi:agsm: 3
- codec_name=mi:mp4a-40-29: 3
- codec_name=mi:tiff: 3
- codec_name=mi:ZyGo: 3
- codec_name=mi:Bitmap: 3
- codec_name=mi:Nellymoser: 3
- codec_name=mi:VP31: 3
- codec_name=mi:AIC: 3
- codec_name=mi:tga : 3
- codec_name=mi:nclc: 3
- codec_name=mi:stxt: 2
- codec_name=mi:R10k: 2
- codec_name=mi:AVj2: 2
- codec_name=mi:DVB Subtitle: 2
- codec_name=mi:rle1: 2
- codec_name=mi:Hap Q: 2
- codec_name=mi:spex: 2
- codec_name=mi:SPXN: 2
- codec_name=mi:V_QUICKTIME: 2
- codec_name=mi:A_QUICKTIME/QDM2: 2
- codec_name=mi:Digital Voodoo: 2
- codec_name=mi:rrtp: 2
- codec_name=mi:rtcp: 2
- codec_name=mi:EVRC: 2
- codec_name=mi:D_WEBVTT/SUBTITLES: 2
- codec_name=mi:Sony Real Time Metadata: 2
- codec_name=mi:Cooker: 2
- codec_name=mi:Hap Alpha: 2
- codec_name=mi:DXDI: 2
- codec_name=mi:qtvr: 2
- codec_name=mi:IV41: 2
- codec_name=mi:3IV2: 2
- codec_name=mi:mvc1: 2
- codec_name=mi:R10g: 2
- codec_name=mi:MYUV: 2
- codec_name=mi:S_TEXT/ASCII: 2
- codec_name=mi:dtPA: 2
- codec_name=mi:fdsc: 2
- codec_name=mi:lvc1: 2
- codec_name=mi:DTS-UHD: 1
- codec_name=mi:sawp: 1
- codec_name=mi:OggV: 1
- codec_name=mi:DTS LBR: 1
- codec_name=mi:evte: 1
- codec_name=mi:SLS: 1
- codec_name=mi:m1a : 1
- codec_name=mi:APAC: 1
- codec_name=mi:metx: 1
- codec_name=mi:ER HVXC: 1
- codec_name=mi:qtz : 1
- codec_name=mi:.mp1: 1
- codec_name=mi:TTML: 1
- codec_name=mi:mp4a: 1
- codec_name=mi:C5: 1
- codec_name=mi:V_MJPEG: 1
- codec_name=mi:dtnt: 1
- codec_name=mi:HapA: 1
- codec_name=mi:v410: 1
- codec_name=mi:HV60: 1
- codec_name=mi:(Encrypted): 1
- codec_name=mi:cvar: 1
- codec_name=mi:VP6A: 1
- codec_name=mi:Shr6: 1
- codec_name=mi:Shr5: 1
- codec_name=mi:dmb1: 1
- codec_name=mi:Mczm: 1
- color_primaries=mi:BT.2020 / BT.709: 1
- color_transfer=mi:PQ / BT.709: 1
- color_matrix=mi:IPT-PQ-C2 / BT.709: 1
- codec_name=mi:ACTL: 1
- codec_name=mi:MJPG: 1
- codec_name=mi:H264: 1
- codec_name=mi:qdrw: 1
- codec_name=mi:gpmd: 1
- codec_name=mi:PSIV: 1
- codec_name=mi:M-GIF: 1
- codec_name=mi:R411: 1
- codec_name=mi:fire: 1
- codec_name=mi:sqcp: 1
- codec_name=mi:DIV3: 1
- codec_name=mi:mp4a-40-95: 1
- codec_name=mi:PGVV: 1
- codec_name=mi:FFV1: 1
- codec_name=mi:WMV2: 1
- codec_name=mi:divx: 1
- codec_name=mi:yuv4: 1
- codec_name=mi:v308: 1
- codec_name=mi:HapM: 1
- codec_name=mi:RealVideo 2: 1
- codec_name=mi:vcmM: 1
- codec_name=mi:NO16: 1
- codec_name=mi:v408: 1
- codec_name=mi:ilbc: 1
- codec_name=mi:SHQ2: 1

Not observable from MediaInfo: timescale, handler_name.

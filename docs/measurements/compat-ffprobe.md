# `coreav ffprobe`: the command-coverage set, agreement and speed

The measurements of `docs/design/compat-cli.md` ("The measurements, fixed
before the runs"), on the bars of that note ("The bars, pre-registered"),
for `coreav ffprobe` (roadmap 8.7 step 1). `coreav ffmpeg` is not
measured here: it refuses every command for now.

The bars and methods were committed in `0f3237db` before any run.

## Round 1

### B1, B2 and the writers (B3a on the fixtures)

`tools/compat-corpus/run.py commands`, every `ffprobe` command of
`commands-v1` on every generated fixture, binary of `72fb2f24`:

| Fixtures | B1 `ffprobe` refusals | B1 `ffmpeg` refusals | B2 runs | B3a identical output |
|---|---|---|---|---|
| ISO base media (96 files) | 48 / 48 | 897 / 897 | 8,272 / 8,831 | 7,284 / 8,272 (88.1%) |
| Matroska and WebM (38) | 48 / 48 | 897 / 897 | 3,344 / 3,497 | 3,344 / 3,344 (100%) |
| MPEG-TS (45) | 48 / 48 | 897 / 897 | 4,001 / 4,181 | 3,286 / 4,001 (82.1%) |

**B1 met.** Every out-of-scope command of the set exits 4 with a message
naming the option and prints nothing; `coreav ffmpeg` writes no file.
**B2 missed:** the four commands that ask for no section (`ffprobe INPUT`,
`INPUT -hide_banner`, `-i INPUT -v debug`, `INPUT -print_format json`) are
refused by design (`ffprobe`'s summary on standard error is not
reproduced), and the fixtures `coreav` rejects (`truncated.mp4`,
`nested_10000.mp4`) fail every command. **B3a on the fixtures missed** for
ISO base media and MPEG-TS: every byte of every native command's output is
compared, and one differing value fails the whole output. In MPEG-TS that
value is mostly the AAC `bit_rate`, which CoreAV leaves unknown.

### B3a on the corpus

`tools/compat-corpus/` (the keys run), binary of `72fb2f24`, one remote
container per file: every corpus v1 `real` file whose container `coreav
ffprobe` reads and every `real` transport stream of corpus v1-ts-audio,
holdout included. Per key, the share of the files it appears in on which
CoreAV printed every occurrence as `ffprobe` did (a value CoreAV leaves
unknown counts as different). Tag keys found in fewer than five files are
counted together under "other tags", so that no key names a single private
file.

1315 files (1049 ISO base media, 209 Matroska and WebM, 57 MPEG-TS; 389 of them held out); 12 that `ffprobe` itself rejects are not compared, and 8 that `coreav ffprobe` rejects count as disagreeing on every key.

**B3a missed** for the keys below, in each container; every other key is at or above 98%. Full tables, tags included, are in `compat-ffprobe/b3a-round1.json`.

**ISO base media**: 101 keys found in at least five files; 17 at or above 98%, 84 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `streams.divx_packed` | 40 | 0 | 0.00% |
| `streams.quarter_sample` | 40 | 0 | 0.00% |
| `streams.side_data_list.side_data_type` | 53 | 14 | 26.42% |
| `packets.duration` | 1049 | 683 | 65.11% |
| `packets.duration_time` | 1049 | 683 | 65.11% |
| `streams.bit_rate` | 1044 | 690 | 66.09% |
| `streams.side_data_list.displaymatrix` | 18 | 14 | 77.78% |
| `streams.side_data_list.rotation` | 18 | 14 | 77.78% |
| `packets.side_data_list.side_data_type` | 243 | 197 | 81.07% |
| `streams.r_frame_rate` | 1049 | 883 | 84.18% |
| `packets.flags` | 1049 | 907 | 86.46% |
| `streams.duration` | 1049 | 909 | 86.65% |
| `streams.duration_ts` | 1049 | 909 | 86.65% |
| `chapters.end` | 15 | 13 | 86.67% |
| `chapters.end_time` | 15 | 13 | 86.67% |
| `chapters.id` | 15 | 13 | 86.67% |
| `chapters.start` | 15 | 13 | 86.67% |
| `chapters.start_time` | 15 | 13 | 86.67% |
| `chapters.time_base` | 15 | 13 | 86.67% |
| `streams.avg_frame_rate` | 1049 | 914 | 87.13% |
| `streams.display_aspect_ratio` | 631 | 553 | 87.64% |
| `streams.has_b_frames` | 973 | 853 | 87.67% |
| `packets.pts` | 1049 | 921 | 87.80% |
| `packets.pts_time` | 1049 | 921 | 87.80% |
| `packets.dts` | 1049 | 922 | 87.89% |
| `packets.dts_time` | 1049 | 922 | 87.89% |
| `packets.codec_type` | 1049 | 925 | 88.18% |
| `streams.pix_fmt` | 973 | 858 | 88.18% |
| `packets.size` | 1049 | 927 | 88.37% |
| `streams.sample_aspect_ratio` | 631 | 558 | 88.43% |
| `packets.pos` | 1049 | 929 | 88.56% |
| `packets.stream_index` | 1049 | 930 | 88.66% |
| `packets.side_data_list.skip_samples` | 218 | 194 | 88.99% |
| `streams.chroma_location` | 853 | 760 | 89.10% |
| `streams.profile` | 1012 | 921 | 91.01% |
| `streams.bits_per_raw_sample` | 746 | 680 | 91.15% |
| `streams.color_range` | 551 | 506 | 91.83% |
| `streams.level` | 973 | 894 | 91.88% |
| `streams.codec_long_name` | 1043 | 959 | 91.95% |
| `streams.codec_name` | 1043 | 959 | 91.95% |
| `streams.color_space` | 471 | 438 | 92.99% |
| `packets.side_data_list.discard_padding` | 218 | 203 | 93.12% |
| `packets.side_data_list.discard_reason` | 218 | 203 | 93.12% |
| `packets.side_data_list.skip_reason` | 218 | 203 | 93.12% |
| `format.duration` | 1049 | 988 | 94.18% |
| `format.bit_rate` | 1048 | 989 | 94.37% |
| `streams.codec_tag` | 1049 | 996 | 94.95% |
| `streams.codec_tag_string` | 1049 | 996 | 94.95% |
| `streams.sample_fmt` | 649 | 621 | 95.69% |
| `streams.color_transfer` | 456 | 440 | 96.49% |
| `streams.height` | 973 | 943 | 96.92% |
| `streams.coded_height` | 973 | 944 | 97.02% |
| `streams.field_order` | 744 | 722 | 97.04% |
| `streams.channel_layout` | 585 | 568 | 97.09% |
| `streams.start_pts` | 1046 | 1017 | 97.23% |
| `streams.start_time` | 1046 | 1017 | 97.23% |
| `streams.width` | 973 | 946 | 97.23% |
| `streams.codec_type` | 1049 | 1021 | 97.33% |
| `streams.coded_width` | 973 | 947 | 97.33% |
| `streams.film_grain` | 973 | 949 | 97.53% |
| `streams.closed_captions` | 973 | 951 | 97.74% |
| `streams.refs` | 973 | 951 | 97.74% |
| `format.nb_streams` | 1049 | 1027 | 97.90% |
| `streams.disposition.attached_pic` | 1049 | 1027 | 97.90% |
| `streams.disposition.captions` | 1049 | 1027 | 97.90% |
| `streams.disposition.clean_effects` | 1049 | 1027 | 97.90% |
| `streams.disposition.comment` | 1049 | 1027 | 97.90% |
| `streams.disposition.default` | 1049 | 1027 | 97.90% |
| `streams.disposition.dependent` | 1049 | 1027 | 97.90% |
| `streams.disposition.descriptions` | 1049 | 1027 | 97.90% |
| `streams.disposition.dub` | 1049 | 1027 | 97.90% |
| `streams.disposition.forced` | 1049 | 1027 | 97.90% |
| `streams.disposition.hearing_impaired` | 1049 | 1027 | 97.90% |
| `streams.disposition.karaoke` | 1049 | 1027 | 97.90% |
| `streams.disposition.lyrics` | 1049 | 1027 | 97.90% |
| `streams.disposition.metadata` | 1049 | 1027 | 97.90% |
| `streams.disposition.non_diegetic` | 1049 | 1027 | 97.90% |
| `streams.disposition.original` | 1049 | 1027 | 97.90% |
| `streams.disposition.still_image` | 1049 | 1027 | 97.90% |
| `streams.disposition.timed_thumbnails` | 1049 | 1027 | 97.90% |
| `streams.disposition.visual_impaired` | 1049 | 1027 | 97.90% |
| `streams.id` | 1049 | 1027 | 97.90% |
| `streams.index` | 1049 | 1027 | 97.90% |
| `streams.time_base` | 1049 | 1027 | 97.90% |

**Matroska and WebM**: 101 keys found in at least five files; 92 at or above 98%, 9 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `chapters.end` | 5 | 3 | 60.00% |
| `chapters.end_time` | 5 | 3 | 60.00% |
| `streams.duration` | 10 | 9 | 90.00% |
| `streams.duration_ts` | 10 | 9 | 90.00% |
| `packets.duration` | 209 | 200 | 95.69% |
| `packets.duration_time` | 209 | 200 | 95.69% |
| `packets.dts` | 209 | 204 | 97.61% |
| `packets.dts_time` | 209 | 204 | 97.61% |
| `streams.chroma_location` | 91 | 89 | 97.80% |

**MPEG-TS**: 157 keys found in at least five files; 90 at or above 98%, 67 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `packets.duration` | 45 | 15 | 33.33% |
| `packets.duration_time` | 45 | 15 | 33.33% |
| `packets.dts` | 45 | 17 | 37.78% |
| `packets.dts_time` | 45 | 17 | 37.78% |
| `packets.pts` | 45 | 19 | 42.22% |
| `packets.pts_time` | 45 | 19 | 42.22% |
| `streams.bit_rate` | 39 | 19 | 48.72% |
| `packets.codec_type` | 45 | 23 | 51.11% |
| `packets.flags` | 45 | 23 | 51.11% |
| `packets.pos` | 45 | 23 | 51.11% |
| `packets.size` | 45 | 23 | 51.11% |
| `packets.stream_index` | 45 | 23 | 51.11% |
| `programs.streams.bit_rate` | 39 | 20 | 51.28% |
| `streams.duration` | 44 | 23 | 52.27% |
| `streams.duration_ts` | 44 | 23 | 52.27% |
| `streams.has_b_frames` | 40 | 21 | 52.50% |
| `packets.side_data_list.id` | 44 | 24 | 54.55% |
| `programs.streams.duration` | 44 | 24 | 54.55% |
| `programs.streams.duration_ts` | 44 | 24 | 54.55% |
| `programs.streams.has_b_frames` | 40 | 22 | 55.00% |
| `streams.avg_frame_rate` | 45 | 26 | 57.78% |
| `programs.streams.avg_frame_rate` | 45 | 27 | 60.00% |
| `packets.side_data_list.side_data_type` | 44 | 28 | 63.64% |
| `format.bit_rate` | 44 | 38 | 86.36% |
| `format.duration` | 44 | 38 | 86.36% |
| `streams.sample_rate` | 42 | 37 | 88.10% |
| `programs.streams.closed_captions` | 40 | 36 | 90.00% |
| `streams.channel_layout` | 40 | 36 | 90.00% |
| `streams.closed_captions` | 40 | 36 | 90.00% |
| `streams.color_primaries` | 10 | 9 | 90.00% |
| `streams.color_space` | 10 | 9 | 90.00% |
| `streams.color_transfer` | 10 | 9 | 90.00% |
| `programs.streams.sample_rate` | 42 | 38 | 90.48% |
| `streams.channels` | 42 | 38 | 90.48% |
| `programs.streams.channel_layout` | 40 | 37 | 92.50% |
| `programs.streams.channels` | 42 | 39 | 92.86% |
| `streams.profile` | 42 | 39 | 92.86% |
| `streams.color_range` | 15 | 14 | 93.33% |
| `streams.r_frame_rate` | 45 | 42 | 93.33% |
| `programs.streams.profile` | 42 | 40 | 95.24% |
| `programs.streams.r_frame_rate` | 45 | 43 | 95.56% |
| `streams.display_aspect_ratio` | 33 | 32 | 96.97% |
| `streams.sample_aspect_ratio` | 33 | 32 | 96.97% |
| `streams.field_order` | 39 | 38 | 97.44% |
| `streams.chroma_location` | 40 | 39 | 97.50% |
| `streams.coded_height` | 40 | 39 | 97.50% |
| `streams.coded_width` | 40 | 39 | 97.50% |
| `streams.extradata_size` | 40 | 39 | 97.50% |
| `streams.film_grain` | 40 | 39 | 97.50% |
| `streams.height` | 40 | 39 | 97.50% |
| `streams.level` | 40 | 39 | 97.50% |
| `streams.pix_fmt` | 40 | 39 | 97.50% |
| `streams.refs` | 40 | 39 | 97.50% |
| `streams.width` | 40 | 39 | 97.50% |
| `streams.bits_per_sample` | 42 | 41 | 97.62% |
| `streams.initial_padding` | 42 | 41 | 97.62% |
| `streams.sample_fmt` | 42 | 41 | 97.62% |
| `streams.start_pts` | 44 | 43 | 97.73% |
| `streams.start_time` | 44 | 43 | 97.73% |
| `programs.pcr_pid` | 45 | 44 | 97.78% |
| `programs.streams.index` | 45 | 44 | 97.78% |
| `streams.codec_long_name` | 45 | 44 | 97.78% |
| `streams.codec_name` | 45 | 44 | 97.78% |
| `streams.codec_tag` | 45 | 44 | 97.78% |
| `streams.codec_tag_string` | 45 | 44 | 97.78% |
| `streams.codec_type` | 45 | 44 | 97.78% |
| `streams.id` | 45 | 44 | 97.78% |

### B4 speed

`tools/compat-corpus/speed.py`, the shipped (`dist`) binary of
`e6ba1571`, on the development VM. 57 held-out files (20 ISO base media, 20
Matroska and WebM, 17 transport streams: every held-out transport stream of
at most 256 MB there is), every native command, 5 rounds alternating with
the pinned `ffprobe`, wall time of the process, page cache warm.

| | Median ratio, `coreav ffprobe` over `ffprobe` |
|---|---|
| All (5,073 pairs) | 0.288 (95% CI 0.280 to 0.296) |
| ISO base media | 0.215 (95% CI 0.211 to 0.219) |
| Matroska and WebM | 0.391 (95% CI 0.381 to 0.401) |
| MPEG-TS | 0.364 (95% CI 0.353 to 0.372) |
| Slowest command | 0.332 (`-v error -show_entries stream=codec_name INPUT`) |
| Slowest command in one container | 0.522 |

**B4 met**, for every command, in each container and overall; every
interval's upper bound is below 1.00, so `coreav ffprobe` is faster than
`ffprobe` on these commands and files. The four commands refused by design
have no ratio. Host: 8 vCPUs, Intel Xeon @ 2.90 GHz (the development VM);
the remote client of the B3a run was running at the same time (load average
about 2), which a ratio this far below 1.00 does not hinge on.
Pairs and summaries: `compat-ffprobe/b4-round1.json`.

## Round 2

### B3a on the corpus

Binary of `5a5ad036`, same files and method as round 1 (1315 files; 12
that `ffprobe` rejects are not compared, 8 that `coreav ffprobe` rejects
count as disagreeing on every key). Keys found in at least five files and
below 98%: ISO base media 76 of 101 (84 in round 1), Matroska and WebM 2
of 101 (9), MPEG-TS 65 of 157 (67). **B3a missed.** Full tables:
`compat-ffprobe/b3a-round2.json`.

### B4 speed

Not reported: the run of the `dist` binary of `5a5ad036` overlapped a
release build and a development loop on the same host during its last
files, a host fault; B4 was measured again on the round 3 binary.

## Round 3

The code of round 2 with the ISO stream durations, bit rates, timecode
rates and protection side data of `docs/design/compat-cli.md` ("How the
values are derived"). B3a ran on the binary of `1ec3a6b6`; B4 on the
`dist` binary of `c794eec2`, which adds the review fixes: on the fixtures
the only output they change is the coverage-set command with
`-read_intervals 999999`, now refused (it needs a seek).

### B1, B2 and the writers (B3a on the fixtures)

Binary of `1ec3a6b6`, as in round 1:

| Fixtures | B1 `ffprobe` refusals | B1 `ffmpeg` refusals | B2 runs | B3a identical output |
|---|---|---|---|---|
| ISO base media (96 files) | 48 / 48 | 897 / 897 | 8,272 / 8,831 | 7,573 / 8,272 (91.5%) |
| Matroska and WebM (38) | 48 / 48 | 897 / 897 | 3,344 / 3,497 | 3,344 / 3,344 (100%) |
| MPEG-TS (45) | 48 / 48 | 897 / 897 | 4,001 / 4,181 | 3,286 / 4,001 (82.1%) |

**B1 met; B2 missed** as in round 1 (the commands without a section, the
rejected fixtures; with `c794eec2`, also the `999999` command). **B3a on
the fixtures missed** for ISO base media and MPEG-TS.

### B3a on the corpus

Binary of `1ec3a6b6`, same files and method as round 1: 1315 files (1049 ISO base media, 209 Matroska and WebM, 57 MPEG-TS; 389 held out); 12 that `ffprobe` rejects are not compared, and 8 that `coreav ffprobe` rejects count as disagreeing on every key. Full tables: `compat-ffprobe/b3a-round3.json`.

**B3a missed** for the keys below; every other key is at or above 98%. Against round 1, the keys below 98% went from 84 to 37 in ISO base media (stream `bit_rate` 66.09% to 89.85%, `avg_frame_rate` 87.13% to 99.05%, `duration` 86.65% to 96.38%), from 9 to 2 in Matroska and WebM, and from 67 to 65 in MPEG-TS.

**ISO base media**: 101 keys found in at least five files; 64 at or above 98%, 37 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `streams.side_data_list.displaymatrix` | 18 | 14 | 77.78% |
| `streams.side_data_list.rotation` | 18 | 14 | 77.78% |
| `streams.side_data_list.side_data_type` | 53 | 43 | 81.13% |
| `chapters.end` | 15 | 13 | 86.67% |
| `chapters.end_time` | 15 | 13 | 86.67% |
| `chapters.id` | 15 | 13 | 86.67% |
| `chapters.start` | 15 | 13 | 86.67% |
| `chapters.start_time` | 15 | 13 | 86.67% |
| `chapters.time_base` | 15 | 13 | 86.67% |
| `streams.bit_rate` | 1044 | 938 | 89.85% |
| `streams.bits_per_raw_sample` | 746 | 699 | 93.70% |
| `streams.r_frame_rate` | 1049 | 990 | 94.38% |
| `streams.color_range` | 551 | 522 | 94.74% |
| `streams.has_b_frames` | 973 | 925 | 95.07% |
| `streams.pix_fmt` | 973 | 930 | 95.58% |
| `packets.duration` | 1049 | 1005 | 95.81% |
| `packets.duration_time` | 1049 | 1005 | 95.81% |
| `streams.color_space` | 471 | 452 | 95.97% |
| `streams.chroma_location` | 853 | 820 | 96.13% |
| `streams.sample_fmt` | 649 | 624 | 96.15% |
| `streams.display_aspect_ratio` | 631 | 607 | 96.20% |
| `streams.duration` | 1049 | 1011 | 96.38% |
| `streams.duration_ts` | 1049 | 1011 | 96.38% |
| `format.duration` | 1049 | 1012 | 96.47% |
| `format.bit_rate` | 1048 | 1012 | 96.56% |
| `streams.color_transfer` | 456 | 441 | 96.71% |
| `streams.sample_aspect_ratio` | 631 | 612 | 96.99% |
| `packets.codec_type` | 1049 | 1018 | 97.04% |
| `streams.channel_layout` | 585 | 568 | 97.09% |
| `packets.dts` | 1049 | 1019 | 97.14% |
| `packets.dts_time` | 1049 | 1019 | 97.14% |
| `packets.pts` | 1049 | 1019 | 97.14% |
| `packets.pts_time` | 1049 | 1019 | 97.14% |
| `packets.size` | 1049 | 1019 | 97.14% |
| `packets.pos` | 1049 | 1020 | 97.24% |
| `packets.stream_index` | 1049 | 1021 | 97.33% |
| `packets.flags` | 1049 | 1026 | 97.81% |

**Matroska and WebM**: 101 keys found in at least five files; 99 at or above 98%, 2 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `packets.duration` | 209 | 203 | 97.13% |
| `packets.duration_time` | 209 | 203 | 97.13% |

**MPEG-TS**: 157 keys found in at least five files; 92 at or above 98%, 65 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `streams.bit_rate` | 39 | 24 | 61.54% |
| `programs.streams.bit_rate` | 39 | 25 | 64.10% |
| `streams.color_primaries` | 10 | 9 | 90.00% |
| `streams.color_space` | 10 | 9 | 90.00% |
| `streams.color_transfer` | 10 | 9 | 90.00% |
| `streams.profile` | 42 | 39 | 92.86% |
| `streams.sample_rate` | 42 | 39 | 92.86% |
| `packets.codec_type` | 45 | 42 | 93.33% |
| `packets.dts` | 45 | 42 | 93.33% |
| `packets.dts_time` | 45 | 42 | 93.33% |
| `packets.duration` | 45 | 42 | 93.33% |
| `packets.duration_time` | 45 | 42 | 93.33% |
| `packets.flags` | 45 | 42 | 93.33% |
| `packets.pos` | 45 | 42 | 93.33% |
| `packets.pts` | 45 | 42 | 93.33% |
| `packets.pts_time` | 45 | 42 | 93.33% |
| `packets.size` | 45 | 42 | 93.33% |
| `packets.stream_index` | 45 | 42 | 93.33% |
| `streams.color_range` | 15 | 14 | 93.33% |
| `streams.channel_layout` | 40 | 38 | 95.00% |
| `programs.streams.profile` | 42 | 40 | 95.24% |
| `programs.streams.sample_rate` | 42 | 40 | 95.24% |
| `streams.channels` | 42 | 40 | 95.24% |
| `packets.side_data_list.id` | 44 | 42 | 95.45% |
| `packets.side_data_list.side_data_type` | 44 | 42 | 95.45% |
| `streams.duration` | 44 | 42 | 95.45% |
| `streams.duration_ts` | 44 | 42 | 95.45% |
| `streams.avg_frame_rate` | 45 | 43 | 95.56% |
| `streams.r_frame_rate` | 45 | 43 | 95.56% |
| `streams.display_aspect_ratio` | 33 | 32 | 96.97% |
| `streams.sample_aspect_ratio` | 33 | 32 | 96.97% |
| `streams.field_order` | 39 | 38 | 97.44% |
| `programs.streams.channel_layout` | 40 | 39 | 97.50% |
| `programs.streams.closed_captions` | 40 | 39 | 97.50% |
| `streams.chroma_location` | 40 | 39 | 97.50% |
| `streams.closed_captions` | 40 | 39 | 97.50% |
| `streams.coded_height` | 40 | 39 | 97.50% |
| `streams.coded_width` | 40 | 39 | 97.50% |
| `streams.extradata_size` | 40 | 39 | 97.50% |
| `streams.film_grain` | 40 | 39 | 97.50% |
| `streams.has_b_frames` | 40 | 39 | 97.50% |
| `streams.height` | 40 | 39 | 97.50% |
| `streams.level` | 40 | 39 | 97.50% |
| `streams.pix_fmt` | 40 | 39 | 97.50% |
| `streams.refs` | 40 | 39 | 97.50% |
| `streams.width` | 40 | 39 | 97.50% |
| `programs.streams.channels` | 42 | 41 | 97.62% |
| `streams.bits_per_sample` | 42 | 41 | 97.62% |
| `streams.initial_padding` | 42 | 41 | 97.62% |
| `streams.sample_fmt` | 42 | 41 | 97.62% |
| `format.bit_rate` | 44 | 43 | 97.73% |
| `format.duration` | 44 | 43 | 97.73% |
| `programs.streams.duration` | 44 | 43 | 97.73% |
| `programs.streams.duration_ts` | 44 | 43 | 97.73% |
| `streams.start_pts` | 44 | 43 | 97.73% |
| `streams.start_time` | 44 | 43 | 97.73% |
| `programs.streams.avg_frame_rate` | 45 | 44 | 97.78% |
| `programs.streams.index` | 45 | 44 | 97.78% |
| `programs.streams.r_frame_rate` | 45 | 44 | 97.78% |
| `streams.codec_long_name` | 45 | 44 | 97.78% |
| `streams.codec_name` | 45 | 44 | 97.78% |
| `streams.codec_tag` | 45 | 44 | 97.78% |
| `streams.codec_tag_string` | 45 | 44 | 97.78% |
| `streams.codec_type` | 45 | 44 | 97.78% |
| `streams.id` | 45 | 44 | 97.78% |

### B4 speed

The `dist` binary of `c794eec2`, same files and method as round 1, on the development VM with nothing else running (load average about 1 before and after).

| | Median ratio, `coreav ffprobe` over `ffprobe` |
|---|---|
| All (5,016 pairs) | 0.244 (95% CI 0.240 to 0.251) |
| ISO base media | 0.167 (95% CI 0.163 to 0.173) |
| Matroska and WebM | 0.363 (95% CI 0.360 to 0.367) |
| MPEG-TS | 0.283 (95% CI 0.279 to 0.289) |
| Slowest command | 0.282 (`-v error -select_streams v:0 -count_packets -show_entries stream=nb_read_packets -of csv=p=0 INPUT`) |
| Slowest command in one container | 0.495 |

**B4 met**: no command, container or overall median is over 1.00, and every interval's upper bound is below 1.00, so `coreav ffprobe` is faster than `ffprobe` on these commands and files. Most of these files were later used for development (round 4, below; `docs/design/compat-cli.md`). The five refused commands (the four without a section and `-read_intervals 999999`) have no ratio. Pairs and summaries: `compat-ffprobe/b4-round3.json`.

## Round 4

The code of round 3 with the rules that bring every fixture to
byte-identical output or to a recorded divergence (`docs/design/compat-cli.md`, "How the values are
derived": the MPEG-TS ADTS bit rate over the analysis window, AAC layouts
from the channel configuration or a program config element, the H.264 and
ProRes coded sizes, field order, multi-edit durations and the others listed
there). B3a, on the fixtures and on the corpus, ran on the binary of
`501e685a`. B4 was last measured in round 3 (`c794eec2`, met) and was not
run again on this code.

### B1, B2 and the writers (B3a on the fixtures)

Binary of `501e685a`, as in round 1:

| Fixtures | B1 `ffprobe` refusals | B1 `ffmpeg` refusals | B2 runs | B3a identical output |
|---|---|---|---|---|
| ISO base media (96 files) | 48 / 48 | 897 / 897 | 8,178 / 8,831 | 8,036 / 8,036 (100%), 142 set aside |
| Matroska and WebM (38) | 48 / 48 | 897 / 897 | 3,306 / 3,497 | 3,306 / 3,306 (100%) |
| MPEG-TS (45) | 48 / 48 | 897 / 897 | 3,960 / 4,181 | 3,933 / 3,933 (100%), 27 set aside |

The set-aside runs are the ones a recorded known divergence explains:
`KD-DECLARED-SIZE` (`docs/known-divergences.md`, the four fixtures that
declare a size their samples contradict) and `KD-TS-DUPLICATE`
(`docs/known-divergences-ts.md`, the fixture with duplicated packets).
`tools/compat-corpus/run.py` lists them and sets a run aside only when
every key the two tools print differently over the whole file is one the
record names. B2 runs fall against round 3 (by 94, 38 and 41) mainly
because `-read_intervals 999999` is now refused where it needs a seek (on
95, 38 and 41 fixtures; one of the 95 already failed in round 3).
**B1 met; B2 missed** as in round 1. **B3a on the fixtures met** for all
three containers.

### B3a on the corpus

Binary of `501e685a`, same files and method as round 1: 1315 files (1049 ISO base media, 209 Matroska and WebM, 57 MPEG-TS; 389 held out); 12 that `ffprobe` rejects are not compared, and 8 that `coreav ffprobe` rejects count as disagreeing on every key. Full tables: `compat-ffprobe/b3a-round4.json`.

**B3a missed** for the keys below; every other key outside the tags is at or above 98% (tags: `compat-ffprobe/b3a-round4.json`; the tag keys below 98% are those of round 3). The counts of keys below 98% are those of round 3 (ISO base media 37 of 101, Matroska and WebM 2 of 101, MPEG-TS 65 of 157), though values moved: MPEG-TS stream `bit_rate` 61.54% to 84.62% (in programs 64.10% to 87.18%), ISO base media `bits_per_raw_sample` 93.70% to 97.05%, `has_b_frames` 95.07% to 95.48%, stream `bit_rate` 89.85% to 90.04%. Four ISO base media keys fell: `channel_layout` 97.09% to 96.07% (six files), `format.duration` 96.47% to 96.28% (two files), and stream `duration` and `duration_ts` 96.38% to 96.28% (one file).

The held-out files of ISO base media and Matroska were not held out in rounds 2 to 4: by a selection error, the development files used for the fixes were 224 of the 303 held-out ISO base media files and all 69 held-out Matroska and WebM files. The 17 held-out MPEG-TS files were not used for development. From round 5, B3a is also reported for development and held-out files apart (`docs/design/compat-cli.md`, "The measurements, fixed before the runs").

**ISO base media**: 101 keys found in at least five files; 64 at or above 98%, 37 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `streams.side_data_list.displaymatrix` | 18 | 14 | 77.78% |
| `streams.side_data_list.rotation` | 18 | 14 | 77.78% |
| `streams.side_data_list.side_data_type` | 53 | 43 | 81.13% |
| `chapters.end` | 15 | 13 | 86.67% |
| `chapters.end_time` | 15 | 13 | 86.67% |
| `chapters.id` | 15 | 13 | 86.67% |
| `chapters.start` | 15 | 13 | 86.67% |
| `chapters.start_time` | 15 | 13 | 86.67% |
| `chapters.time_base` | 15 | 13 | 86.67% |
| `streams.bit_rate` | 1044 | 940 | 90.04% |
| `streams.r_frame_rate` | 1049 | 990 | 94.38% |
| `streams.color_range` | 551 | 522 | 94.74% |
| `streams.has_b_frames` | 973 | 929 | 95.48% |
| `streams.pix_fmt` | 973 | 930 | 95.58% |
| `packets.duration` | 1049 | 1005 | 95.81% |
| `packets.duration_time` | 1049 | 1005 | 95.81% |
| `streams.color_space` | 471 | 452 | 95.97% |
| `streams.channel_layout` | 585 | 562 | 96.07% |
| `streams.chroma_location` | 853 | 820 | 96.13% |
| `streams.sample_fmt` | 649 | 624 | 96.15% |
| `streams.display_aspect_ratio` | 631 | 607 | 96.20% |
| `format.duration` | 1049 | 1010 | 96.28% |
| `streams.duration` | 1049 | 1010 | 96.28% |
| `streams.duration_ts` | 1049 | 1010 | 96.28% |
| `format.bit_rate` | 1048 | 1012 | 96.56% |
| `streams.color_transfer` | 456 | 441 | 96.71% |
| `streams.sample_aspect_ratio` | 631 | 612 | 96.99% |
| `packets.codec_type` | 1049 | 1018 | 97.04% |
| `streams.bits_per_raw_sample` | 746 | 724 | 97.05% |
| `packets.dts` | 1049 | 1019 | 97.14% |
| `packets.dts_time` | 1049 | 1019 | 97.14% |
| `packets.pts` | 1049 | 1019 | 97.14% |
| `packets.pts_time` | 1049 | 1019 | 97.14% |
| `packets.size` | 1049 | 1019 | 97.14% |
| `packets.pos` | 1049 | 1020 | 97.24% |
| `packets.stream_index` | 1049 | 1021 | 97.33% |
| `packets.flags` | 1049 | 1026 | 97.81% |

**Matroska and WebM**: 101 keys found in at least five files; 99 at or above 98%, 2 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `packets.duration` | 209 | 203 | 97.13% |
| `packets.duration_time` | 209 | 203 | 97.13% |

**MPEG-TS**: 157 keys found in at least five files; 92 at or above 98%, 65 below:

| Key | Files | Agree | Rate |
|---|---|---|---|
| `streams.bit_rate` | 39 | 33 | 84.62% |
| `programs.streams.bit_rate` | 39 | 34 | 87.18% |
| `streams.color_primaries` | 10 | 9 | 90.00% |
| `streams.color_space` | 10 | 9 | 90.00% |
| `streams.color_transfer` | 10 | 9 | 90.00% |
| `streams.profile` | 42 | 39 | 92.86% |
| `streams.sample_rate` | 42 | 39 | 92.86% |
| `packets.codec_type` | 45 | 42 | 93.33% |
| `packets.dts` | 45 | 42 | 93.33% |
| `packets.dts_time` | 45 | 42 | 93.33% |
| `packets.duration` | 45 | 42 | 93.33% |
| `packets.duration_time` | 45 | 42 | 93.33% |
| `packets.flags` | 45 | 42 | 93.33% |
| `packets.pos` | 45 | 42 | 93.33% |
| `packets.pts` | 45 | 42 | 93.33% |
| `packets.pts_time` | 45 | 42 | 93.33% |
| `packets.size` | 45 | 42 | 93.33% |
| `packets.stream_index` | 45 | 42 | 93.33% |
| `streams.color_range` | 15 | 14 | 93.33% |
| `streams.channel_layout` | 40 | 38 | 95.00% |
| `programs.streams.profile` | 42 | 40 | 95.24% |
| `programs.streams.sample_rate` | 42 | 40 | 95.24% |
| `streams.channels` | 42 | 40 | 95.24% |
| `packets.side_data_list.id` | 44 | 42 | 95.45% |
| `packets.side_data_list.side_data_type` | 44 | 42 | 95.45% |
| `streams.duration` | 44 | 42 | 95.45% |
| `streams.duration_ts` | 44 | 42 | 95.45% |
| `streams.avg_frame_rate` | 45 | 43 | 95.56% |
| `streams.r_frame_rate` | 45 | 43 | 95.56% |
| `streams.display_aspect_ratio` | 33 | 32 | 96.97% |
| `streams.sample_aspect_ratio` | 33 | 32 | 96.97% |
| `streams.field_order` | 39 | 38 | 97.44% |
| `programs.streams.channel_layout` | 40 | 39 | 97.50% |
| `programs.streams.closed_captions` | 40 | 39 | 97.50% |
| `streams.chroma_location` | 40 | 39 | 97.50% |
| `streams.closed_captions` | 40 | 39 | 97.50% |
| `streams.coded_height` | 40 | 39 | 97.50% |
| `streams.coded_width` | 40 | 39 | 97.50% |
| `streams.extradata_size` | 40 | 39 | 97.50% |
| `streams.film_grain` | 40 | 39 | 97.50% |
| `streams.has_b_frames` | 40 | 39 | 97.50% |
| `streams.height` | 40 | 39 | 97.50% |
| `streams.level` | 40 | 39 | 97.50% |
| `streams.pix_fmt` | 40 | 39 | 97.50% |
| `streams.refs` | 40 | 39 | 97.50% |
| `streams.width` | 40 | 39 | 97.50% |
| `programs.streams.channels` | 42 | 41 | 97.62% |
| `streams.bits_per_sample` | 42 | 41 | 97.62% |
| `streams.initial_padding` | 42 | 41 | 97.62% |
| `streams.sample_fmt` | 42 | 41 | 97.62% |
| `format.bit_rate` | 44 | 43 | 97.73% |
| `format.duration` | 44 | 43 | 97.73% |
| `programs.streams.duration` | 44 | 43 | 97.73% |
| `programs.streams.duration_ts` | 44 | 43 | 97.73% |
| `streams.start_pts` | 44 | 43 | 97.73% |
| `streams.start_time` | 44 | 43 | 97.73% |
| `programs.streams.avg_frame_rate` | 45 | 44 | 97.78% |
| `programs.streams.index` | 45 | 44 | 97.78% |
| `programs.streams.r_frame_rate` | 45 | 44 | 97.78% |
| `streams.codec_long_name` | 45 | 44 | 97.78% |
| `streams.codec_name` | 45 | 44 | 97.78% |
| `streams.codec_tag` | 45 | 44 | 97.78% |
| `streams.codec_tag_string` | 45 | 44 | 97.78% |
| `streams.codec_type` | 45 | 44 | 97.78% |
| `streams.id` | 45 | 44 | 97.78% |

## B6

`scripts/bench_guard.py` has six `coreav ffprobe` cases (`ffprobe-mp4`,
`ffprobe-mkv`, `ffprobe-ts` and their `-packets` twins); a case without a
baseline entry is reported, not judged, until a guard run on `main` records
it. The coverage set runs in the three fixture workflows on every change
against the baselines in `tools/compat-corpus/baseline-*.json`.

## Hosts

B3a ran as 1,315 one-file containers of 1 CPU and 4 GiB each per round, on a cloud host. B4 and the fixtures ran on the development VM.

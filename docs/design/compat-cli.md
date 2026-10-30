# The compatibility command: `coreav ffprobe` and `coreav ffmpeg`

Status: Proposed

Roadmap step 1 and an earlier change: a command that accepts the common `ffmpeg`
and `ffprobe` invocations of the jobs CoreAV does natively, and refuses
every option it does not implement with a clear error, so that an
unsupported command never runs with different behaviour.

This note is written in the pull request that builds the **command-coverage
set**, before any of the command exists. It records the set, the
vocabulary step 1 implements and the bars, so that none of them can be
chosen after a result is seen.

## The command-coverage set

[`commands-v1`](../../tools/compat-corpus/commands-v1/README.md): 1,147 real
invocations from 894 Stack Overflow answers, with their sources, the rules
that chose them, and the licence of each answer. Its README has the
composition; this note has what the set is used for.

Three labels, from `tools/compat-corpus/classify.py`: `native` (step 1
must run it), `out-of-scope` (step 1 must refuse it) and `unusable` (the
line was cut short, or a value was never expanded; outside the bars).

## What step 1 is

The jobs of roadmap step 1: `ffprobe` stream and format output, and
`ffmpeg -c copy` with seeking, trimming, concatenation, segmenting, HLS
and stream selection. Every one of them has a CoreAV command behind it:
`probe`, `clip`, `split`, `join`, `remux`, `sanitize`,
`extract-audio` and `hls`, each with the fixture oracle B3b names. So
`native` means a job that exists, and B2 and B3b promise nothing that has
to be built first — what step 1 builds is the compatibility surface over
them.

## Step 1's vocabulary

What `native` means, option by option. Anything not here is refused.

**`coreav ffprobe`**

| | |
|---|---|
| With a value | `-i`, `-v`, `-loglevel`, `-of`, `-print_format`, `-output_format`, `-show_entries`, `-select_streams`, `-read_intervals`, `-f` |
| Flags | `-show_format`, `-show_streams`, `-show_chapters`, `-show_programs`, `-show_packets`, `-show_error`, `-count_packets`, `-hide_banner`, `-pretty`, `-sexagesimal`, `-nostdin` |
| Writers (`-of`) | `json`, `default`, `compact`, `csv`, `flat` |
| `-show_entries` sections | `format`, `stream`, `packet`, `chapter`, `program`, `format_tags`, `stream_tags`, `packet_tags`, `chapter_tags`, `program_tags`, `stream_side_data` |
| Demuxers (`-f`) | `mov,mp4,m4a,3gp,3g2,mj2`, `matroska,webm`, `mpegts` |

**`coreav ffmpeg`**, stream copy only

| | |
|---|---|
| Codec options | `-c`, `-codec`, `-vcodec`, `-acodec`, `-scodec`, with any stream specifier — the value must be `copy` |
| With a value | `-i`, `-ss`, `-sseof`, `-t`, `-to`, `-map`, `-map_metadata`, `-map_chapters`, `-movflags`, `-f`, `-loglevel`, `-v`, `-segment_time`, `-segment_format`, `-segment_list`, `-hls_time`, `-hls_list_size`, `-hls_segment_filename`, `-hls_playlist_type`, `-safe`, `-fflags`, `-avoid_negative_ts`, `-reset_timestamps`, `-max_muxing_queue_size`, `-threads` |
| Flags | `-y`, `-n`, `-an`, `-vn`, `-sn`, `-dn`, `-hide_banner`, `-nostdin`, `-copyts`, `-start_at_zero`, `-ignore_unknown`, `-shortest` |
| Demuxers (`-f` before the first `-i`) | `concat`, `mov,mp4,m4a,3gp,3g2,mj2`, `matroska,webm`, and the aliases FFmpeg matches against those names: `mp4`, `mov`, `m4a`, `3gp`, `matroska`, `webm`. **Not** `mpegts`: `coreav probe` reads transport streams, no writer of step 1 does |
| Muxers (`-f` after it) | `mp4`, `ipod`, `hls`, `segment` — CoreAV reads more containers than it writes, so `-f matroska` on the way out is refused although Matroska on the way in is read |
| One input, one output | several inputs means muxing streams together, which is not a step 1 job |
| Containers written | MP4, M4A, HLS (`.m3u8`), and numbered segments |
| `-f concat` | the input is a list file, and the job is `coreav join` (`docs/design/join.md`), which takes files sharing a codec configuration |

**Refused by design,** although common in the set, each because the job
itself is not native yet: `-show_frames` and `-count_frames` and any
`-show_entries frame=…` or `nb_read_frames` (per-frame fields that need a
decoder), `-vf`, `-af`, `-filter_complex` and `-filter:*` (filtering),
every encoder value of a codec option, `-f lavfi`, `-f image2`,
`-f rawvideo`, `-f null`, `-re`, `-loop`, `-ignore_loop`, `-bsf:*`
(bitstream filters), `-r`, `-frames:v`, `-s`, `-pix_fmt`, `-ar`, `-ac`,
`-b:*`, `-ab`, `-crf`, `-preset`, `-profile:*`, `-vframes`, `-vsync`,
`-async`, `-metadata`, `-strict`, `-flags`, `-probesize`,
`-analyzeduration`, `-protocol_whitelist`, `-bufsize`, `-maxrate`, `-g`,
`-pass`, `-vpre`, `-q:*`, `-qscale`, `-framerate`, `-video_size`,
`-pattern_type`, `-metadata:*`, `-level`, `-id3v2_version`,
`-stream_loop`, `-aspect`, and the informational queries (`-h`, `-formats`,
`-encoders`, `-decoders`, `-filters`, `-muxers`, `-pix_fmts`,
`-sample_fmts`, `-layouts`, `-sections`, `-version`).

One encode is taken since: in a build with a decoder backend, a command
line that encodes the video with `libx264`, with the few options
[`transcode.md`](transcode.md) lists under "Through `coreav ffmpeg`", runs
through `coreav transcode`'s pipeline, and its bar is B9 there. The
vocabulary above and the labels of the command-coverage set stay step 1's,
so those command lines remain `out-of-scope` in the set, and B1 holds for
the default build, which has no decoder backend and refuses them. B1 on a
build with a decoder backend needs those lines relabelled first, a change
to `classify.py` this step does not make.

Every one of those is refused by **naming it**: the error says which
option or property is not supported, and the exit status is non-zero. A
refused command never runs differently: in strict-safe mode it is refused,
and otherwise it runs FFmpeg itself and says so ("Fallback to FFmpeg").

Three option meanings the set makes unavoidable, pre-registered here so
the implementation does not get to choose later:

- **`-ss` before `-i` is input seeking, after it output seeking.** Both
  are in the vocabulary, and with `-c copy` the two produce different
  files. FFmpeg's semantics are the bar in both positions for the range a
  command names (where each time counts from and what it bounds), not an
  opinion of ours. What is written for that range is `coreav clip`'s,
  judged by the clip oracle ("Times" under `coreav ffmpeg`).
- **`-map` takes FFmpeg's selector grammar**: `FILE[:TYPE[:INDEX]]`,
  `FILE:m:KEY[:VALUE]` for a metadata tag, and a leading `-` to remove
  from what earlier `-map` options selected. Order matters, and a negative
  selector is a subtraction from the set selected so far. The set uses all
  three forms.
- **`-c copy` is only legal when the output container can carry the
  stream's codec.** The classifier reads extensions, not codecs, so a
  native command may still be one FFmpeg itself refuses for the fixture it
  is given (VP8 into MP4). The runner therefore picks, per command, a
  fixture that command can take — the container pair its `-f` and
  extensions name, and the streams its `-map` selectors need, which for
  `-map 0:a:3` means four audio streams and for
  `-map 0:m:language:eng` a language tag — and reports any command for
  which no such fixture exists rather than counting it against a bar.

## The bars, pre-registered

Measured over `commands-v1`. Every command's placeholders are
filled by the runner with a fixture or corpus file, which is why the set
holds the option sequence and not the names an answer happened to use.

| | Bar |
|---|---|
| **B1 Refusal** | **100%** of `out-of-scope` commands exit non-zero, name the option or property that is not supported, and write no output file. One command that runs instead of refusing fails the bar |
| **B2 Acceptance** | **100%** of `native` commands run to success on a substituted fixture the command's container pair can carry |
| **B3a ffprobe agreement** | for each key a `native` `ffprobe` command prints, CoreAV's value equals the pinned `ffprobe`'s, after the documented known divergences, on **at least 98% of the files** that key appears in (the probe command's denominator), and on 100% of the generated fixtures |
| **B3b ffmpeg output** | the output file of a `native` `ffmpeg` command passes the existing oracle of the job it is — clip, split, join, remux, sanitize, extract-audio, HLS — at those oracles' own bars: 100% of the fixtures |
| **B4 Speed** | per command, the median per-input ratio against the same FFmpeg command is **at most 1.00**, overall and per class (`ffprobe`, copy-write), with the confidence interval reported. A claim that CoreAV is faster needs the interval's upper bound below 1.00 |
| **B5 Vocabulary** | every option appearing in **10 or more** commands of the set is either in the tables above or in the refused-by-design list above. None may be absent from both, and none may be silently ignored. A test checks it against the committed set, in both directions: the note may not name an option the classifier does not know either |
| **B6 Guard** | the set runs in CI on the generated fixtures on every change, and the speed bar has a regression guard |

**Where the bars stand.** In the last round
(`docs/measurements/compat-ffprobe.md`, round 7) B1 is met, B2 and B3a
are missed, and B4 was last measured in round 3 (met) and not run again on
this code. B3a now misses on the
corpus only: the fixtures print byte-identical output for every command
that runs, in all three containers (100%, with the runs a recorded known
divergence explains set aside), but on the corpus 9 of 101 ISO base media
keys and 35 of 157 MPEG-TS keys agree on fewer than 98% of files
(Matroska and WebM: 0 of 101). On the development files alone, 9 of 101
ISO base media keys and 6 of 157 MPEG-TS keys are below 98%; the MPEG-TS
work measured in round 6 generalized, taking the held-out count from 62
to 35 while the development count stayed at 6.
B2 misses on refusals by design (the
commands without a section, intervals that need a seek) and on the
fixtures CoreAV rejects, not on wrong values, so it does not gate the
label. B3b is checked on the fixtures for the `commands-0.1.0` commands:
R3 on the fixtures is met and gated in CI for every `ffmpeg` shape of the
set, each run natively on at least one fixture of each family it takes,
the clips of audio alone (C15, C19) included ("`coreav ffmpeg`" below).
R3 on the held-out draw, and B3b over `commands-v1`, are not yet measured.
The bars stay as registered. Until
B3a is met, `coreav ffprobe` is labelled experimental where users see it
(its help and `coreav --help`), with these numbers; the label goes when a
later round, run on new code, meets B3a.

**What the set does not reach.** The vocabulary is wider than the set
exercises: seventeen of its options appear in no native command of
`commands-v1`, among them every `-hls_*` option, `-sseof`, `-copyts`,
`-shortest` and `-segment_list`, and only a few commands use
`-f segment`. B2 and B3b therefore measure part of the
vocabulary, and the rest is covered by the generated fixtures of the
existing oracles, as `coreav hls` and `coreav split` already are. The set
measures reach, not completeness.

**The composition is a fact, not a bar.** 66.0% of the usable `ffprobe`
commands and 8.1% of the usable `ffmpeg` commands are native, which
follows from the committed set and the vocabulary above and is known
today. The two floors below only guard against the vocabulary narrowing
later: the native share of `commands-v1` may not fall below **60%**
for `ffprobe` or **7%** for `ffmpeg` without a reason in this note. Both
floors sit a few records below today's share, so that one command
relabelled by a correction is not a failure while a vocabulary narrowed
by several is.

Widening the vocabulary relabels commands in the committed set.
`tools/compat-corpus/tests/` recomputes every label from `classify.py` and
fails when the file disagrees, so a widening shows as a diff of
`commands.jsonl` in the pull request that makes it, and the reach numbers
above move with it in this note.

## Decisions

- **Answers, not questions.** A question's command is often the broken one
  the asker is asking about. An answer scoring 1 or more is one the site's
  readers endorsed, which is what a compatibility command must accept.
- **The whole `ffprobe` tag, the top 500 of `ffmpeg`.** The `ffprobe` tag
  is small enough to take entirely, so that stratum has no selection at
  all. The `ffmpeg` tag is not, and taking its highest-voted questions is
  the one ordering that does not require a judgement about what matters.
  Rejected: filtering the `ffmpeg` stratum to questions about copying,
  which would have made the reach number meaningless.
- **File names are replaced by placeholders.** A coverage set is about
  which options are combined; the names are examples and every run
  substitutes a file anyway. It also keeps the committed set clear of
  whatever a name in a post held, and merges two records that differ only
  in a file name. Rejected: keeping the names, which made a Windows path
  mangled by POSIX word-splitting look like a real invocation.
- **One classifier, in code, committed with the set.** Rejected: labelling
  by hand, which cannot be rechecked when the vocabulary widens.
- **`unusable` is a line that cannot be run**: cut short by the
  extraction, or holding a value no shell expanded. An invocation that
  real FFmpeg itself refuses — no output file, an informational query — is
  `out-of-scope`, because refusing it clearly is exactly what the
  compatibility command has to do.
- **The set is committed under the repository's own terms,** because what
  it holds is option sequences rather than anything an answer's author
  wrote: see `tools/compat-corpus/commands-v1/README.md`, which states the reasoning
  and what reversing it costs.
- **A snapshot is a version.** Scores change and answers are edited, so
  the set is frozen as `commands-v1`; a later snapshot is `commands-v2`,
  in the pull request that measures against it.

## The 0.1.0 command set

Release 0.1.0 does not wait for B2, B3a and B3b over the whole vocabulary.
It ships the commands that upload and preprocessing pipelines run most, the
`ffprobe` listings and the `ffmpeg -c copy` jobs, as **`commands-0.1.0`**, and
holds them to the bars below. This section is written before any run of a
0.1.0 command, and the bars above stay as registered for everything the
set leaves out.

### The set

[`commands-0.1.0/commands.json`](../../tools/compat-corpus/commands-0.1.0/commands.json), derived
from the native commands of `commands-v1` by
[`release_set.py`](../../tools/compat-corpus/release_set.py). `tools/compat-corpus/tests/`
derives it again and fails when the committed file, or a number or row of
this section, differs from what the script derives.

- **Shape.** Each native command is reduced to what it asks the tool to do.
  For `ffprobe`: the sections it asks for (`-show_format`, `-show_streams`,
  `-show_chapters`, `-show_packets`, and each section named by
  `-show_entries`), the writer, and whether it selects streams. For
  `ffmpeg`: the job (a copy, `-f concat`, `-f segment`, `-f hls`), the seek
  and whether it comes before or after `-i`, an end (`-t` or `-to`), the
  streams kept (all, audio only, video only), and `-map`, `-movflags`, the
  timestamp options and the metadata options as one feature each. Left
  out: what does not change the output (the log level, `-y`, `-hide_banner`,
  `-threads`, the order of the options, `-i INPUT` against a bare input), and
  the details within a feature (the times, the stream indexes, a writer's
  settings, the `-show_entries` keys), which the entry's commands still
  carry. The streams kept are those the command selects (FFmpeg's automatic
  selection, `-map`, `-vn`, `-an`, `-sn`) and copies, so `-c copy` and per-type
  copies of what it selects are one shape. A command that selects a stream
  type it does not copy (`-i INPUT -acodec copy OUTPUT` selects video too)
  would encode that stream, so it is a copy only on an input without that
  type: the file records the types per command (`input_without`), and a run
  gives such a command only inputs that have none of them.
- **Eligible.** A native command the command refuses by design is not
  eligible: the four that ask for no section and print only the summary on
  standard error, and `-read_intervals 999999`. They hold 17 of the 253
  native command-and-answer pairs.
- **The cut.** Shapes are ranked by the command-and-answer pairs that use
  them. The set is the smallest prefix of that ranking that reaches 80% of
  the eligible pairs, with whole ties: a shape used as often as the last one
  taken is taken too, so that the order of the shapes' names breaks no tie.

**Result:** 33 shapes, holding 133 of the 167 eligible native commands and
202 of their 236 command-and-answer pairs (85.6%), which is 79.8% of all 253 native pairs.
The 34 shapes below the cut hold one pair each and are listed in the file
(`not_selected`). The cut fell inside the tier of shapes used twice; taking
the whole tier makes 33 shapes where a cut at the first shape over 80%
would have made 27 and left out six shapes as common as the last one taken.

| | Tool | Job | Inputs | Pairs | Shape | Most used command |
|---|---|---|---|---:|---|---|
| C01 | ffmpeg | `remux` | MP4/MOV, Matroska/WebM | 33 | job:copy, streams:all | `-i INPUT -c copy OUTPUT` |
| C02 | ffmpeg | `join` | MP4/MOV | 31 | job:concat, streams:all | `-f concat -i INPUT -c copy OUTPUT` |
| C03 | ffmpeg | `remux` | MP4/MOV, Matroska/WebM | 14 | job:copy, map, streams:all | `-i INPUT -c copy -map 0 OUTPUT` |
| C04 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 12 | entries:stream, select_streams, writer:default | `-v error -select_streams v:0 -show_entries stream=avg_frame_rate -of default=noprint_wrappers=1:nokey=1 INPUT` |
| C05 | ffmpeg | `clip` | MP4/MOV | 10 | end, job:copy, seek:output, streams:all | `-i INPUT -ss 0 -t 120 -c copy OUTPUT` |
| C06 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 8 | format, streams, writer:json | `-v quiet -print_format json -show_format -show_streams INPUT` |
| C07 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 6 | select_streams, streams, writer:default | `-v quiet -show_streams -select_streams v:0 INPUT` |
| C08 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 6 | streams, writer:default | `-show_streams INPUT` |
| C09 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 6 | entries:format, writer:default | `-v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 INPUT` |
| C10 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 5 | entries:stream, select_streams, writer:csv | `-loglevel error -select_streams a -show_entries stream=codec_type -of csv=p=0 INPUT` |
| C11 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 5 | entries:stream, writer:default | `-v error -show_entries stream=codec_name INPUT` |
| C12 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 5 | format, streams, writer:default | `-show_streams -show_format INPUT` |
| C13 | ffmpeg | `extract-audio` | MP4/MOV | 5 | job:copy, streams:audio | `-i INPUT -vn -c:a copy OUTPUT` |
| C14 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 5 | entries:format, writer:csv | `-i INPUT -show_entries format=duration -v quiet -of csv=p=0` |
| C15 | ffmpeg | `clip` | MP4/MOV | 4 | end, job:copy, seek:output, streams:audio | `-i INPUT -ss 00:02:54.583 -t 300 -acodec copy OUTPUT` |
| C16 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 4 | streams, writer:json | `-of json -show_streams INPUT` |
| C17 | ffmpeg | `remux` | MP4/MOV, Matroska/WebM | 4 | job:copy, streams:video | `-y -i INPUT -vcodec copy OUTPUT` |
| C18 | ffmpeg | `clip` | MP4/MOV | 3 | end, job:copy, seek:input, streams:all | `-ss 60 -i INPUT -to 70 -c copy OUTPUT` |
| C19 | ffmpeg | `clip` | MP4/MOV | 3 | end, job:copy, seek:input, streams:audio | `-ss 3 -i INPUT -t 5 -c:a copy OUTPUT` |
| C20 | ffmpeg | `extract-audio` | MP4/MOV | 3 | job:copy, map, streams:audio | `-i INPUT -map 0:a -c copy OUTPUT` |
| C21 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 3 | entries:stream, select_streams, writer:flat | `-v error -of flat=s=_ -select_streams v:0 -show_entries stream=height INPUT` |
| C22 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 3 | entries:stream, select_streams, writer:json | `-v error -select_streams v -show_entries stream=width,height -of json INPUT` |
| C23 | ffmpeg | `remux` | MP4/MOV, Matroska/WebM | 3 | job:copy, movflags:faststart, streams:all | `-i INPUT -c copy -movflags +faststart OUTPUT` |
| C24 | ffprobe | `probe-packets` | MP4/MOV, Matroska/WebM, MPEG-TS | 3 | packets, writer:default | `-i INPUT -show_packets` |
| C25 | ffmpeg | `extract-audio` | MP4/MOV | 2 | job:copy, map, metadata, streams:audio | `-i INPUT -acodec copy -map 0 -map_metadata 0:s:0 OUTPUT` |
| C26 | ffmpeg | `remux` | MP4/MOV, Matroska/WebM | 2 | job:copy, map, streams:all, timestamps | `-fflags +genpts -i INPUT -c:v copy -c:a copy -c:s copy -map 0 OUTPUT` |
| C27 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 2 | chapters, writer:json | `-hide_banner INPUT -print_format json -show_chapters -loglevel error` |
| C28 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 2 | entries:stream, entries:stream_tags, select_streams, writer:json | `-select_streams v:0 -show_entries stream=width,height:stream_tags=rotate -of json INPUT` |
| C29 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 2 | entries:stream, select_streams, writer:compact | `-v 0 -select_streams v:0 -show_entries stream=pix_fmt -of compact=p=0:nk=1 INPUT` |
| C30 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 2 | entries:stream, writer:csv | `-loglevel error -show_entries stream=codec_type -of csv=p=0 INPUT` |
| C31 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 2 | format, writer:default | `-loglevel quiet -show_format INPUT` |
| C32 | ffmpeg | `split` | MP4/MOV | 2 | job:segment, map, streams:all | `-i INPUT -c copy -map 0 -f segment -segment_time 300 -segment_format mp4 OUTPUT` |
| C33 | ffprobe | `probe` | MP4/MOV, Matroska/WebM, MPEG-TS | 2 | chapters, writer:csv | `-print_format csv -show_chapters INPUT` |

Every command of an entry is measured, not only the most used one: the
file lists them (`commands`), 133 in all, with the `-show_entries` keys
each entry prints (`keys`).

**Containers.** The `ffprobe` entries run on all three container families.
A copy entry runs on the inputs its CoreAV job reads today: `remux` reads
MP4/MOV and Matroska/WebM, and `clip`, `join`, `split` and `extract-audio`
read MP4/MOV. Every output is MP4 or M4A. `-c copy` from a transport stream
is not in `commands-0.1.0`: no CoreAV writer reads MPEG-TS, so that command stays
refused by name (or falls back, below), as does a Matroska input to the
jobs other than `remux`. A command that forces a demuxer (`-f mpegts` before
the input, in C16) means what it means only on files of that family: on an
MP4 file the forced MPEG-TS probe lists no stream at all. The file records
the family per command (`input_container`), and every bar runs such a command
on the fixtures and files of that family alone, which is its denominator.

**What the set leaves out.** No native command of `commands-v1` uses `-f hls`,
so HLS output is not in the set; `coreav hls` is unchanged. `-f segment` is
in through C32 only. The keys the set prints are not a narrow subset:
C06, C08, C12 and C16 print every format and stream key, C27 and C33 every
chapter key, and C24 every packet key, so the set's keys are every key of
those sections that B3a measures; what drops out is the program keys of
`-show_programs`.

### The 0.1.0 bars, pre-registered

Every run of these bars sets `COREAV_STRICT_SAFE=1`, so a command that is
not native is refused and never handed to FFmpeg ("Fallback to FFmpeg",
below), and sets `COREAV_FALLBACK_LOG` to a file of the run; a run whose log
holds any line is void. A fallback would pass every comparison by being
FFmpeg.

| | Bar | Refines |
|---|---|---|
| **R1 Keys, fixtures** | for each `commands-0.1.0` `ffprobe` command, standard output identical to the pinned `ffprobe`'s on **100%** of the generated fixtures of each container family, after the documented known divergences, as B3a sets them aside | B3a's fixture half, for the 0.1.0 commands |
| **R2 Keys, held-out draw** | for each key the set prints, CoreAV's value equals the pinned `ffprobe`'s on **at least 98%** of the files of the held-out draw below that the key appears in, in each container family | B3a's corpus half: the same comparison and threshold, over the set's keys and a fresh draw |
| **R3 Copy output** | the output of each `commands-0.1.0` `ffmpeg` command passes the oracle of its job (the `Job` column) at that oracle's own rules on **100%** of the fixtures, and on **at least 98%** of the (command, file) pairs of the draw on which the pinned `ffmpeg` runs the same command to success | B3b, which it keeps for the fixtures and extends to the draw |
| **R4 Speed** | per command class (the `Job` column), the median of the per-(command, file) ratios against the same FFmpeg command is **at most 1.00**, in each container family and overall, with the bootstrap 95% interval; a "faster" claim needs the interval's upper bound below 1.00 | B4, per class of the set instead of per command |

**Method.** R1 runs `run.py commands` on the 0.1.0 commands, as B3a's fixture
half does. R2 compares the keys of the two listings of `run.py keys`, as
B3a's corpus half does, counting only the keys the set prints and
counting keys as B3a's rounds count them (keys in at least five files; tag
keys in fewer pooled); a key CoreAV leaves unknown disagrees, and a file
CoreAV rejects disagrees on every key unless `KD-STRICT` explains it, as from
round 5. R3 fills each command's placeholders with a file its container
pair and its `-map` selectors can take and that has none of the stream
types its `input_without` names, as B2 does, and a command no file of the
draw can take is reported, not counted. R4 runs as B4, on the host
registered below: the file in the page cache, one untimed run of each first, then
5 rounds alternating CoreAV and FFmpeg, wall time of the whole process, the
ratio of a pair the median of CoreAV's rounds over the median of FFmpeg's;
an `ffmpeg` command writes its output to the same local file system for
both tools and the file is deleted after each run. R4 takes from each
container family the first 20 draw files of at most 256 MB in the order
`sha256("coreav-compat-r4:" + sha256)`. The report gives, beside each median
and interval, the share of pairs on which CoreAV is faster, the host and
the FFmpeg version, and per-command medians, which are not gated.

**The R4 host, registered before the run.** A GCP `c4-standard-16` VM with
SMT off (one thread per core), started through `scripts/gcp_vm_job.py`, the
rest of the VM idle, and both tools' processes pinned to the same four
cores. Before the draw is opened the host passes a calibration gate on
spent files (the first 20 per container family of the spent corpus v1 and
`v1-ts-audio` held-out sets, in the R4 order): an A/A arm (FFmpeg against
itself) and a null arm (CoreAV against itself) over the 0.1.0 commands,
each with every class's median ratio within 0.98 to 1.02 and its 95%
interval containing 1.00. A failed gate is a host fault: the draw stays
closed and the round runs on another VM of the same class. The round's
record names the host class, the CPU model and the gate's numbers before
the draw's results. A shared runner pool does not qualify: its CPU model
varies between jobs.

**The held-out draw.** The ISO base media and Matroska held-out files of
corpus v1 were used for development (rounds 2 to 4), and the MPEG-TS
held-out files of `v1-ts-audio` have been reported from in rounds 5 and 6,
so every one of them is spent. R2, R3 and R4 run on a new `v2-heldout` draw,
operation `compat`, salt `coreav-corpus-v2-heldout-compat:`, held out whole,
under these rules, fixed before any file is fetched:

- **Form:** files unchanged, at most 200 MB.
- **Coverage:** 60 to 100 `real` files of each container family: ISO base
  media (`mp4`, `mov`, `m4v`, `m4a`, `3gp`), Matroska and WebM (`mkv`, `webm`),
  and transport streams (`ts`, `m2ts`, `mts`). At 60 files one file is 1.7%,
  so a key in all of them can miss on one file and meet 98%.
- **Nothing seen before:** a candidate whose SHA-256 is in corpus v1,
  `v1-fmp4-large`, `v1-ts-audio`, `mediarecorder-v1` or draws 1 to 5 is dropped.
- **Diversity:** a file whose structural signature (draw 4's) is already in
  this draw or an earlier one is skipped.
- **Sets and oracle** as the shared corpus rules; files the set's jobs do
  not read stay in the draw for the `ffprobe` bars.
- **The collection runs no measurement:** `ffprobe` is the only tool run over
  the files; no `coreav` command runs on the draw before the round that
  opens it.

The draw is spent by the first round reported from it. A round that misses
a bar is final for that round; a later attempt, on new code, takes a new
draw with its own salt, and development uses spent sets only.

**What meeting them allows.** When one round on new code meets R1 to R4,
the commands of the set are supported in 0.1.0: the experimental label of
`coreav ffprobe` no longer applies to them, and stays on every other
`ffprobe` command until B3a is met. B1 is unchanged and still applies to every
command outside the set.

## The command

This part is written in the pull request that builds `coreav ffprobe`,
before any measurement of it. `coreav ffmpeg` followed in its own pull
request, under the same rules ("`coreav ffmpeg`" below).

### Surface: subcommands, and the same binary under the tools' names

`coreav ffprobe ARGS...` and `coreav ffmpeg ARGS...` take the arguments in
`ffprobe`'s and `ffmpeg`'s own syntax, not CoreAV's: everything after the
subcommand name is handed to the compatibility parser untouched, so
`coreav ffprobe -v error -show_entries stream=width -of csv=p=0 in.mp4`
means what it means to `ffprobe`. `coreav ffprobe --help` is therefore not
clap's help either: `--help`, `-h` and `-help` print a short usage text that
names this note.

**Multicall.** When the program is started under the name `ffprobe` or
`ffmpeg` (the file name of `argv[0]`, with an `.exe` extension removed and
compared without case on Windows), it behaves as `coreav ffprobe` or
`coreav ffmpeg` with the whole argument list. A link named `ffprobe` that
points to `coreav`, earlier on `PATH`, substitutes it for scripts and
programs that run `ffprobe`. Any other name keeps the ordinary command, so a
copy renamed by a package manager (`coreav-1.0`) still works. Rejected: an
environment variable to select the mode (it would leak into every child
process), and separate thin binaries (three artefacts to package and keep in
step for one behaviour).

### Grammar

The grammar `ffprobe`'s documentation gives: options are single-dash words,
an option that takes a value takes the next argument whatever it starts
with, and a bare argument is the input (the value of `-i` is too). Options
and the input can come in any order. Two inputs fail as `ffprobe` fails
them, and so does `--opt`, which `ffprobe` does not read as `-opt`.

Accepted: the vocabulary table above. Values are parsed by `ffprobe`'s
documented syntax:

- `-of`/`-print_format`/`-output_format` `NAME[=OPTIONS]`, options
  `key=value` pairs separated by `:`, with the short and long names the
  documentation lists for each writer: `default` (`nokey`/`nk`,
  `noprint_wrappers`/`nw`), `compact` and `csv` (`item_sep`/`s`, `nokey`/`nk`,
  `escape`/`e` with `c`, `csv` or `none`, `print_section`/`p`), `flat`
  (`sep_char`/`s`, `hierarchical`/`h`), `json` (`compact`/`c`). An unknown
  writer (`xml`, `ini` included) or writer option is refused.
- `-show_entries` `SECTION[=KEY[,KEY...]][:SECTION...]`, sections as in the
  vocabulary table. A key the section does not have selects nothing, as in
  `ffprobe` (`format=nosuchkey` prints an empty `format`); a key the section
  has but CoreAV does not derive is refused (below).
- `-select_streams` the stream specifiers of `ffmpeg`'s documentation that
  name streams by what a container says: `N`, `v`, `a`, `s`, `d`, `t`, `V`
  and `TYPE:N`. Program, metadata and stream-id specifiers are refused.
- `-read_intervals` the documented interval syntax; the forms supported are
  listed in the implementation's refusal messages and below.
- `-v`/`-loglevel` a level name or number, with or without `+`/`-` flags;
  it only sets what CoreAV writes on standard error (below).

### Refusal and exit codes

An option outside the vocabulary, a value of an accepted option CoreAV
does not implement (a writer, a specifier form, a section, a key), or a
command whose only output would be what CoreAV does not produce, is
**refused**: one line on standard error naming the option or property and
why, nothing on standard output, no output written or changed, and **exit
status 4**, a status no other `coreav` command uses, so a script can tell
"not supported here" from a failure. Rejected: reusing 1 (a rejected file)
or 2 (a usage error), which would make a refusal look like a bad file or a
typo. The other statuses keep their meaning in every `coreav` command: 1 the
input was rejected (not a container `coreav ffprobe` reads, or invalid; the
message names the input, as `ffprobe`'s does), 2 a malformed command (a
missing value, a second input, a value that does not parse; `ffprobe` also
refuses those), 3 the input could not be read (missing, unreadable, a
failed remote read), 141 standard output closed by its reader. `ffprobe`
itself exits 1 for all of 1 to 3; a script that tests for success sees the
same answer.

A refusal while the command line is parsed comes before the input is
opened. A refusal while the job is planned or run comes once the input is
read: a property of the source that `coreav ffmpeg` does not copy, an
output that already exists, or an encoder value it does not implement
(`-crf 90`).

A refused command runs FFmpeg's `ffmpeg` or `ffprobe` instead unless
strict-safe mode is on (`COREAV_STRICT_SAFE=1`, `coreav --strict-safe`),
the mode for services that read untrusted input: see "Fallback to FFmpeg".

### Output fidelity

**Standard output is the contract**, byte for byte: the section order,
the writers' layout, quoting and escaping, number formats, which keys
appear and in what order, and `-show_entries`'s selection rules, as the
pinned `ffprobe` 7.0.2 prints them for the same command. The black box is
the only source for those rules.

**Standard error is not.** `ffprobe` writes a banner and a summary of the
input on standard error at the default log level; CoreAV writes nothing on
standard error on success, at any `-v`. A command that asks for no section
(`ffprobe INPUT`, `ffprobe -v debug -i INPUT`) exists only for that summary,
so it is refused (exit 4), never run to print nothing. `-show_error` prints
the error section on standard output, as `ffprobe` does.

**Keys.** Every key the sections have in `ffprobe`, in its order, with the
value derived from what the container and the codec configuration say,
without decoding. A key is either **derived**, or **unknown**: printed the
way `ffprobe` prints a value it does not know (`N/A` in the text writers,
left out by `json`), never guessed. A key CoreAV
leaves unknown is refused when `-show_entries` names it, since that command
asks for nothing but the value. The keys left unknown are listed in the
implementation (`ffprobe::UNKNOWN_KEYS`) and in this note's table once the
measurement is in; a key moves from unknown to derived in the pull request
that derives it, with its agreement measured.

### One implementation

`coreav_cli::ffprobe` (the library, so the WebAssembly build can carry
it): the parser, a section model (`Section`: ordered entries of integer,
string, rational or unknown values, and child sections), the writers, and
one builder per container that maps the native probe result to sections:
`coreav-isobmff`'s `Movie`, `coreav-mkv`'s `Segment` and `coreav-mpegts`'s
`TransportStream`, detected as `coreav probe` detects them. Packets come
from each crate's packet iterator and are written as they are read, never
held in memory: a listing is as long as the file.

`coreav probe --ffprobe-compat` stays, as what it already is: the checked
subset of `-show_format -show_streams` that `docs/schema/probe-v1.md`
lists, which the WebAssembly build, the Workers package and the fixture
workflows' `compat_check.py` use. For every key both print, the
`coreav ffprobe` builders call the same functions it does (`compat.rs`,
`matroska.rs`, `mpegts.rs`), so a derivation is written once. Its output
does not change here. Rejected: removing it (unreleased, so allowed, but it
would change three consumers in a pull request about something else), and
re-deriving it from the `coreav ffprobe` sections now: those list streams
`--ffprobe-compat` drops (codecs CoreAV does not identify) and take some
values from the stream rather than the container, so its documents, and the
known-divergence overlays keyed on them, would move. Making it a fixed
selection of `coreav ffprobe`'s output is a change of its own, with its
`compat_check` run.

### `coreav ffmpeg`

`coreav_cli::ffmpeg`, one implementation in three steps. `args::parse` reads the
command line into one input, one output and their options, and refuses
anything outside the vocabulary before the input is opened. `plan` reads the
input's streams as `coreav ffprobe` lists them, selects the streams as
`ffmpeg` does and picks the writer. `run` writes the output to a temporary
file next to it and moves it into place only when the whole job succeeded.
`coreav ffmpeg` prints nothing on success; `run` returns the summary the
writer's own command prints, which the fixture test hands to the oracles.
The writers are the existing jobs, each with the options the command
implies:

| The command | The writer |
|---|---|
| a copy to `.mp4` (or `-f mp4`) | `coreav remux`, MP4/MOV or Matroska/WebM input, the selected streams, failing rather than dropping a stream MP4 cannot carry |
| a copy of one audio stream to `.m4a` (or `-f ipod`) | `coreav extract-audio` |
| `-ss`, `-t` or `-to`, before or after `-i` | `coreav clip`: one video stream and at most one audio stream |
| the same, with no video selected | the clip of audio alone (`clip_audio_mp4` of `coreav-isobmff-write`): one AAC stream |
| `-f segment` with MP4 segments | `coreav split --every` the `-segment_time` (2 s when absent) |
| `-f concat` with an MP4 output | `coreav join` of the list's files (`coreav_cli::concat::read`, with the demuxer's `safe` option): the first video stream and, unless `-an`, the first audio stream, which is what `ffmpeg` selects from the first file when the selection is that |

Every other output container, MOV and 3GP included, is refused: the MOV
muxer's output is not the MP4 writer's.

**Stream selection**, as the pinned `ffmpeg` 7.0.2 makes it (black box: its
`Stream mapping` for the same command). Without `-map`: the video stream
with the default disposition, else the largest picture; the audio stream
with the default disposition, else the most channels; the first of equals
in both; and a text subtitle only when a codec option names the subtitle
type alone (`-c:s`, `-codec:s`, `-scodec`): neither `-c` nor `-c:s:0` selects
one. With `-map`: the selectors in order, a leading `-` subtracting. `-vn`,
`-an`, `-sn` and `-dn` remove their type either way. Every selected stream
must be covered by a `copy` codec option, since `ffmpeg` would encode the
others, and kept in the input's order, which the writers keep.

**Times.** Every time counts from the input's start time (`ffprobe`'s
`format.start_time`, 0 when it has none): on a file presented from 5 s,
`-ss 6` before or after `-i` is 11 s on its timeline (black box). An input
`-ss` moves the output's time 0 to it, so an output `-to` counts from there;
an input `-t` or `-to` stops reading the input; an output `-t` counts from
the output `-ss`; `-t` wins over `-to` given with it. Minutes and seconds of
`[HH:]MM:SS` are below 60, or the command is a usage error, as in `ffmpeg`.
The clip starts at the keyframe at or before the start, as `coreav clip` does, with an
edit list that presents from the start. `ffmpeg`'s output seeking with `-c
copy` differs: it writes no video before the next keyframe, so a range
shorter than a GOP has no video at all (black box: `-ss 100ms -t 600ms` on
the fixtures). The clip oracle is the bar, at its own rules.

A clip without video is not snapped: it presents the range exactly, through
an edit list in the audio's media timescale, from one frame of pre-roll
before the frame the start falls in, as a clip's audio does, and its time 0
is the start. `ffmpeg`'s differs at its ends: with the seek after `-i` it
starts at the first frame at or after the start and places it there with an
empty edit, and with either seek it ends at the end of the frame the end
falls in (black box: `-ss 0.5 -t 1` on 48 kHz AAC presents 0.512 s to
1.514667 s of the input with the seek after `-i`, 0.5 s to 1.514 s with it
before). The clip oracle, run without video, is the bar.

**Segments.** `coreav split` cuts at the latest clean cut point at or before
each multiple of `-segment_time`; `ffmpeg`'s segment muxer cuts at the first
keyframe at or after it (black box: segments from 0, 6, 10 and 16 s at
`-segment_time 5` on a 2 s GOP). The split oracle is the bar, at its own
rules, so the cut points need not be `ffmpeg`'s.

**One refusal point.** Every command that is not native, whether the
parser, the plan or a writer finds it (a writer's `Unsupported` or a codec
it does not copy), reaches one call, `fallback_cmd::refuse` in `run_ffmpeg`
(`compat_cmd.rs`), as the `NotNative` of `fallback.rs` that `coreav
ffprobe` refuses with, its typed `Reason` `Option(name, value)` for an
option or the value refused, `Property(text)` for a property of the command
or its input. In strict-safe mode it exits 4; otherwise FFmpeg's `ffmpeg`
runs the command as given ("Fallback to FFmpeg"). Each refusal
leaves the files as they were, so FFmpeg finds them so: most come before
any output is written, the ones found once the input is read included (an
output that exists without `-y` or `-n`), and a join's come at its end,
with the join written to a temporary file that is then removed. A file CoreAV
rejects exits 1, a malformed command 2, an unreadable input 3. `-h`,
`-help` and `--help` are refused like the other informational queries
(unlike `coreav ffprobe -h`, which prints a usage text): `commands-v1` lists
them as out-of-scope `ffmpeg` commands, which B1 requires to exit non-zero.

**R3 on the fixtures (CI).** The `the_0_1_0_commands_on_the_fixtures` test of
`coreav-cli` runs every `ffmpeg` command of `commands-0.1.0` on every
generated fixture of its container families that has none of the stream
types its `input_without` names, writing `out.mp4`, `out.m4a` for an audio
extraction or `out%03d.mp4` for segments. Every entry must run natively on
at least one fixture of each of its families.
`tools/compat-corpus/copy_check.py` then checks every output twice: the pinned `ffmpeg`, run on the same command, must map the
same input streams to the same output streams, and the oracle of the job
(`remux_check` or `mkv_remux_check`, `clip_check`, `split_check`,
`extract_check`, `join_check` with `--ffmpeg-concat`'s comparison) must pass it
at its own rules, which set aside the two fixtures whose picture size
changes inside the track for the clip, split and join oracles. A clip
without video must also start where the command says. A join's
list names the fixture twice, and its timing is held to `ffmpeg -f concat`'s
with no tolerance: every input placed where the demuxer places it (after
the files before it, each as long as its format duration, less its own
start time, to the microsecond), and the video packets' times, from the
first one's, those of the pinned `ffmpeg`'s output of the same command.
The audio packets are not compared one for one there: where a file's audio
runs past its edit, `ffmpeg`'s muxer moves the next file's first packet to
one tick after the overlap (black box), so the placement check stands for
them. Threshold: 100% of the outputs, in `fixtures.yml` and
`mkv-fixtures.yml`.

**Not covered: refused by name (exit 4 in strict-safe mode, else run by
FFmpeg).**

- A join of other streams than the first video and audio stream of the
  list's first file, a join without video (`join_mp4` cuts at video), a
  join that also seeks or trims, and a list with a directive other than
  `file` and `ffconcat version 1.0` (`duration`, `inpoint`, ...) or a path
  that names a protocol. A list `ffmpeg` fails too (an unsafe file name
  without `-safe 0`, no file, a directive written quoted or escaped) is
  rejected, exit 1.
- A join that would not be timed as the concat demuxer times it: an input
  `join_mp4` places elsewhere than the demuxer, which follows each file at
  its format duration where `join_mp4` follows it at its video's end (black
  box: a 0.3 s video with 1 s of audio, listed twice, is 2.021333 s from
  `ffmpeg`), and inputs whose tracks have several edits, which the demuxer
  reads by its own edit model. Also inputs that do not match the first
  (codec, configuration, size, time base), inputs that hold the copied
  streams at other indexes than the first (the demuxer pairs streams by
  index, `join_mp4` takes each file's first video and audio), and streams `join_mp4` does not
  copy (PCM audio, say). The join runs to a temporary file and finds these
  at its end; the output is left as it was.
- A segmented output of a file without video (C32 on such files): `coreav
  split` cuts at video. A clip without video of other than one AAC
  stream, or written as M4A.
- A clip whose range presents no video, or without video no audio (a
  start past the end): `coreav clip` and the clip of audio alone refuse
  it, where `ffmpeg` writes a short file anyway (black box: `-ss 60` on a
  30 s file writes about 2 s; `-ss 3` before `-i` on 2 s of AAC writes one
  frame, after `-i` a file without packets).
- What the writers do not copy to MP4: subtitle streams, PCM and MP3
  audio (MP3 by the remux), other audio in a clip than AAC, video other than
  H.264 `avc1`, HEVC `hvc1`, AV1 or VP9 in a clip, an M4A file of other than
  one audio stream (C20's `-map 0:a` on a file with several), a clip or
  segments of more than one video or audio stream.
- Options with a meaning the writers do not reproduce: `-fflags` other than
  `+genpts` (C26's `+igndts` changes the timestamps `ffmpeg` writes; black
  box: the decode times of the first audio packets), `-map_metadata` other
  than `-1` (C25's `0:s:0`), and `-map_metadata -1` except on an M4A
  output of a stream without a language: the remux and the clip copy the
  metadata, and the M4A writer keeps the stream's language, which `ffmpeg`
  drops.
- Matroska and WebM inputs to the jobs other than the remux, and
  MPEG-TS inputs to all of them, as "Containers" says.
- An input or output named with a protocol: standard input or output
  (`-`, `pipe:`), and any name with a colon before its first `/` (on
  Windows, its first `/` or `\`), where `ffmpeg` may read a protocol and its
  options: `file:in.mp4`, a URL, `subfile,,start,0,end,99999,,:in.mp4`,
  `:in.mp4`, `a,b:c.mp4`. Some of these `ffmpeg` opens as a plain file, so
  the rule refuses more than it must (black box: `foo:in.mp4` and
  `:out.mp4` fail with "Protocol not found", `a,b:c.mp4` is written as
  named). A name that starts with `/` or `./` is a path, as is, on Windows
  only, a drive letter before a separator (`C:\in.mp4`, `C:/in.mp4`):
  elsewhere `ffmpeg` reads `C:/in.mp4` as protocol `C`.

### The measurements, fixed before the runs

**B1, B2 and the writers (CI, every change).**
`tools/compat-corpus/run.py` runs every `ffprobe` command of `commands-v1`
with each placeholder filled by every generated fixture (the ISO base media
fixtures of `scripts/gen-fixtures.sh`, the Matroska and WebM copies of
`scripts/gen-fixtures-mkv.sh`, the transport streams of
`scripts/gen-fixtures-ts.sh`). `out-of-scope`: exit non-zero, a message,
nothing on standard output (B1). `native`: exit 0 (B2), and standard output
**identical** to the pinned `ffprobe`'s, once the values of unknown keys and
documented divergences are set aside (B3a on the fixtures, 100%). A
fixture made to show a documented divergence is listed in `run.py`
(`SET_ASIDE`) with its record and the keys the record names; its
differences are set aside, and counted apart, only when every key the two
tools print differently over the whole file (the stream and the packet
listings of `run.py keys`) is one of those. The size fixtures stand under
`KD-DECLARED-SIZE`, `ts_duplicates.ts` under `KD-TS-DUPLICATE`.

**B3a on the corpus.** Every corpus v1 `real` file whose container
`coreav ffprobe` reads (MP4, MOV, M4A, M4B, M4V, 3GP, Matroska, WebM), and
every `real` transport stream of corpus v1-ts-audio, holdout files
included (B3a is a correctness bar; nothing is tuned on the holdout). Per
file, both tools run `-v error -of json -show_format -show_streams
-show_chapters -show_programs` and `-v error -of json -count_packets
-show_packets`, and every key `ffprobe` prints is compared, streams paired
by index and packets by position in the listing. A key's rate is the share
of files it appears in on which every occurrence agrees. A key CoreAV
prints as unknown counts as disagreeing. Bar: 98% per key, as above.
From round 5 the known divergences the bar sets aside are applied to the
corpus too, as the library's oracle applies them: a file `coreav ffprobe`
rejects (exit 1) comes under `KD-STRICT` (`docs/known-divergences.md`)
when `ffprobe` reads it as ISO base media (`format_name` `mov,mp4,...`),
`coreav probe` names the defect (a truncated or invalid structure, or a
limit) and the file's bytes show that kind of defect by the walk of
`scripts/oracle_kd.py`. Every key of the listings it rejected then agrees
through the record and stays in the denominator; the report counts these
files and keys apart (`run.py keys`, `via_known_divergence`). A rejection the bytes
do not explain, and any rejection of a Matroska or MPEG-TS file, still
disagrees on every key. Rejected: reading such files in `coreav ffprobe`
as `ffprobe` does (stopping at a box past the end, skipping a second movie
box), which would loosen for one command the policy the library keeps for
every reader; and counting them as disagreement, as
rounds 2 to 4 did, which counted a documented divergence as a bug,
contrary to the bar's own "after the documented known divergences".
Development and fixes use the non-holdout files only.
From round 5, each container's rates are also reported for the
development and the held-out files apart. In rounds 2 to 4 the
development files of ISO base media and Matroska were, by a selection
error, held-out files (224 of 303 and 69 of 69); those two held-out sets
are marked as used for development in every later report. The B3a
verdict stays over all files, as registered: B3a is a correctness bar, and
its held-out part shows whether a fix generalizes, not a performance
claim. Of the B4 files of round 3, 19 of the 20 ISO base media files and
all 20 Matroska and WebM files were among those development files (the
MPEG-TS files were not); speed was not tuned on them, so the round 3
result is kept, but a new claim that
`coreav ffprobe` is faster than `ffprobe` is measured on held-out files
that no development has used.

**B4 Speed.** On held-out files only: from each of the three containers,
the first 20 held-out `real` files of at most 256 MB in the order
`sha256("coreav-compat-b4:" + sha256)` (fewer where the set has fewer).
Every `native` `ffprobe` command of `commands-v1` runs on each of them,
`coreav ffprobe` and the pinned `ffprobe` alternating, 5 rounds, one run at
a time on an otherwise idle host, wall time of the whole process, the file
in the page cache (one untimed run of each first). The ratio of a
(command, file) pair is the median of CoreAV's rounds over the median of
`ffprobe`'s. Bar: the median of the ratios is at most 1.00 for every
command, for each container and overall, with the bootstrap 95% interval
reported; "faster" only where its upper bound is below 1.00.

**B6 Guard.** The CI benchmark guard (`scripts/bench_guard.py`) gets
`coreav ffprobe` cases: a stream and format listing in JSON and a packet
listing in the default writer, on ISO base media, Matroska and transport
stream fixtures, so a slower `coreav ffprobe` fails CI as any other
command does.

**The record.** `docs/measurements/compat-ffprobe.md` states each
round's numbers, and the round's JSON under
`docs/measurements/compat-ffprobe/` is their evidence: the record may say
nothing the evidence contradicts. `tools/compat-corpus/record.py` writes
every per-key block of the record (each container's counts line and its
table of the keys below 98%) from `b3a-roundN.json`. A rate has two
decimals, or as many more as keep a share below the bar from reading as
98.00% (1028/1049 is 97.998%). `record.py --check` runs in
`tools/compat-corpus/tests`, which the deny workflow runs on every pull
request; that workflow has no path filter, so a change to the record alone
is checked. It fails on a block that differs from its rendering, on a
per-key table outside a block, and on a figure in the prose that the JSON
does not hold:

- a rate, or an agree or files count, tied to a named key. The key is
  named in backticks after its container, and a key without a dot is a
  stream key (stream `bit_rate`). In "X to Y", X is the round before, or
  the round an "against round K" in the paragraph names. A rate with
  decimals must be the share as the tables write it. A whole-number rate
  claims the share to the nearest whole percent and on the same side of
  the bar, so 97.71% may be written "97.71%" or "below 98%" but neither
  "98%" nor "97%": a rounding that crosses the bar would state the
  verdict the evidence denies, and a truncation would be a second rule
  for the reader to know. After "below" or "under" the share must be
  under the rate; after "above", "over", "crossed" or "reaches", at or
  over it;
- a container's count of keys below 98%, overall, for development and
  for held-out files, and as "round K: N", where K must be the round the
  paragraph compares with (the round before, or its "against round K");
- the file counts of each round's corpus paragraphs: the files, the
  held-out files, each container's files and their two parts, and the
  files each tool rejects.

It also checks each JSON's own `rate` and `below_98` against its
counts. Each failure names the line of the record, the round, the key and
the JSON's value. Not checked, because no committed JSON holds them or
they name no key: the fixture tables (B1, B2, B3a on the fixtures), whose
`run.py commands` reports are not committed; B4, which has its own JSON
and method; changes in the count of keys below 98% between rounds written
as "from 84 to 37" or "(84 in round 1)"; statements about a group of keys
that name none of them ("the packet keys rose from 93.33% to 95.56%");
the development and held-out split of the files `ffprobe` rejects; and
the figures this note quotes in "Where the bars stand". `record.py` is a
sibling of `run.py`, not one of its modes, because `run.py` runs both
tools on media and `record.py` reads committed files only, in well under
a second. Rejected: generating the whole record from the JSON, which
would turn each round's account of why numbers moved into a template; and
checking the tables alone, which would have passed round 6 as first
written, when its prose gave `format.duration` as 97.81% and the table
below it 97.71% (coreav-dev/coreav#21).

### How the values are derived

What the black box showed, rule by rule, and what each rule rests on.
None of it reads FFmpeg's source; each was found by running the pinned
`ffprobe` on crafted files and the development files of corpus v1, and
checked by `tools/compat-corpus/run.py keys`.

**Writers.** One state machine per writer, driven by "section starts",
"entry", "section ends", over the section tree `ffprobe -sections` prints
(`ffprobe/model.rs`, `ffprobe/writer.rs`). Rules that are not in the
documentation: `json` leaves out a value it does not know and prints an
empty array or object with a blank line inside; `compact` puts a nested
section's entries on its parent's line, prefixed with the element's name
(`tag:`, `disposition:`, and for side data its type,
`side_datum/display_matrix:`), and a typed section starts with the
separator whether it prints entries or not; a stream or packet that has
side data prints its side data section whenever it is printed itself, with
no entry when none is selected; `default` prints a nested section as
prefixed entries (`TAG:`) unless it is an element of an array
(`[SIDE_DATA]`); `flat` quotes everything but integers and escapes `"`,
`\`, `` ` ``, `$` and line breaks. `-show_entries stream=...` names the
streams of programs and of stream groups too, so their arrays are printed
(empty ones included), while `stream_tags` names only the streams'.
Numbers are formatted from the binary value as C's `%f` does (half to
even on the exact value; `writer::fixed`), not by Rust's formatter, which
rounds differently.

**`nb_read_packets`** is the number of packets read when there is one, and
unknown when no packet of the stream was read, with `-count_packets` too.

**`-read_intervals`.** One interval from the start of the file with an
end (`%END`, `%+DURATION`, `%+#PACKETS`); reading stops at the first packet
past the end, of any stream. A start needs a seek, which is not emulated
(`ffprobe` seeks to it and reads on: to the end of the file without an
end, and from the last packet for a start past the end), and an interval
with no end reads every packet or none depending on the sections asked
for (`%` lists every packet with `-show_entries` and none with
`-show_packets` alone): both are refused by name, so `999999` and `10%20`
exit 4.

**ISO base media.** The stream and format keys `--ffprobe-compat` already
prints keep its derivations, which the stream oracle checks. The others:

- `pix_fmt`, `has_b_frames`, `chroma_location`, `field_order`,
  `bits_per_raw_sample`, `coded_width`/`coded_height`, and the codec's own
  sample aspect ratio and frame rate, from the stored configuration
  (`ffprobe/video.rs`: the H.264 and H.265 sequence parameter sets with
  their VUI, `av1C`, `vpcC`, the first ProRes frame header). `yuvj420p` for
  full-range 8-bit 4:2:0 H.264 and H.265, as `ffprobe` names it. Without a
  reordering depth in the H.264 VUI, `has_b_frames` is the depth the first
  packets' presentation order shows.
- `sample_aspect_ratio`: the `pasp` box, else the stream's VUI.
- `r_frame_rate`: an estimate from the decode times of the first 21
  packets, as for Matroska (`mkv::estimate_rate`): among the standard
  rates (twelfths of a frame per second, whole rates, NTSC rates) whose
  frames cover at least three quarters of the intervals, the one on whose
  grid the times fall most evenly, when the variance of their distance to
  it is under 0.01 frames squared; else the rate every interval is a
  whole number of frames of. Equal intervals give their own rate. The rate
  the stream's own timing states plays no part (taking it where there is
  one agreed less often on the development set).
- `avg_frame_rate`: frames over the decode durations of every sample, in
  lowest terms.
- `start_pts`: with one edit, the leading empty edit in the track's ticks;
  without an edit list, the first packets' earliest time; for audio, after
  the samples the first packet skips.
- Audio priming: the samples the edit list cuts from the first packet, or
  without an edit list the iTunes `iTunSMPB` delay, are a "Skip Samples"
  side data element on the first packet.
- Stream durations: without an edit list, the shorter of the media
  header's and the samples' (the samples' when fragments follow, and
  when every sample is in fragments the first of which is decoded from a
  later time, `tfdt`, that start plus the samples' durations; `ffprobe`
  follows a later jump of `tfdt` between fragments, which CoreAV does not
  yet); with one
  edit, no longer than the edit presents; with several, all of them, empty
  ones in between included, rounded to the nearest tick. A data or subtitle track without
  an edit list lasts at least as long as the longest track, in its own
  ticks rounded to the nearest; a timecode track keeps its own.
- The stream `bit_rate`: PCM and IMA ADPCM their constant rate (sample
  rate, channels and bits); otherwise the bytes over a duration,
  truncated, whatever average `esds` or `btrt` declares. Video and audio
  take their whole media, edits aside: the shorter of the media header's
  duration and the samples' (all 176 audio streams of regular samples in
  the development set, with an edit from media time 0, from a later one
  or none; and `edit_multi_audio_average.mp4`, several edits whose
  declared averages were patched). A data or subtitle track takes its
  own span (`fragmented_text.mp4`). A fragmented movie's video and audio
  rate is over the stream's duration as printed (`fragmented_late.mp4`),
  and every track's rate in a fragmented movie is rounded to the nearest;
  with a segment index (`sidx`) `ffprobe` counts only the first
  fragment's bytes, over the whole duration.
- Packets of a subtitle track in a movie with fragments have no
  duration, and every one is a key frame whatever the fragment's flags
  say (`fragmented_text.mp4`; the library's rule,
  `docs/design/sample-tables.md`).
- The format's duration ends where the last audio, video or subtitle
  stream ends (a data stream's end does not count: GoPro's `gpmd` a
  millisecond past the video), and starts where the first audio or video
  stream starts.
- `field_order` of H.265, VP9 and AV1, which their configuration records
  do not state, is the QuickTime `fiel` box's: one field is progressive,
  two fields are named by the detail (1 `tt`, 6 `bb`, 9 `tb`, 14 `bt`),
  anything else unknown (entries crafted with each value; the corpus's
  AV1 and H.265 without the box are unknown).
- `has_b_frames` of an H.264 stream that reorders frames and has several
  edits is 16: `ffprobe` feeds the decoder edit after edit, and reports
  its deepest delay (two and three edits, contiguous, overlapping or
  around an empty one; not one edit, nor H.265).
- `coded_width` and `coded_height`: the H.264 sequence parameter set's
  cropped size, or the sample entry's when it is no larger and falls in
  the same macroblocks in both directions and the cropping keeps the top
  left corner (entries patched around a 1920×1080 set: 1905×1080 and
  1920×1073 are taken, 1904×1080, 1920×1072 and 1921×1080 are not); the
  H.265 set's uncropped size; the size the first ProRes frame header
  states.
- Audio: AAC's `channel_layout` from the `AudioSpecificConfig`'s
  `channelConfiguration` (6 is `5.1`, 7 is `7.1`), and for 0 from its
  program config element, which `ffprobe` names only as a front pair with
  an LFE (`2.1`) or with a back pair (`quad`) among every layout `ffmpeg`
  encodes with one; the ISO/IEC 23003-5 PCM
  entries (`ipcm`, `fpcm`) from their `chnl` box, by `definedLayout` or
  by the list of speaker positions, as `ffprobe` names every layout
  `ffmpeg` writes, and none without the box; Opus's `initial_padding` is
  the `dOps` pre-skip; integer PCM of 24 and 32 bits has a
  `bits_per_raw_sample`.
- A timecode stream's `avg_frame_rate` is its `tmcd` entry's timescale
  over its frame duration, unreduced.
- A protected stream of a movie with protection system headers (`pssh`)
  carries an "Encryption initialization data" side data element; its
  other values come from the format `frma` names.
- iTunes cover pictures (`covr`, and the QuickTime `artwork` key) are
  attached-picture video streams after the tracks, each with its one
  packet; WebVTT tracks are data streams.
- Packets: one listing merged from every track, in file order, except that
  a packet due more than a second before the next one stored is listed
  first (that agreed with every interleaving on the development set); the
  keyframe flag of H.264 and AV1 from the coded data, as the packet oracle
  sets it; the last packet shown of an audio track, and the one presented
  last of a video track, lasting to the end of the presentation; a
  QuickTime chapter track listed as a `bin_data` data stream without
  packets.
- Codecs CoreAV does not decode are named from their sample entry type (and
  `objectTypeIndication`), with `ffprobe -codecs`' long names; the
  `listing` allowlist (`Allowlist::listing`, added for this) keeps their
  streams instead of dropping them.

**Matroska and WebM.** `ffprobe/mkv.rs`, whose module documentation lists
each rule found black box: how the metadata dictionary orders and replaces
keys (`DURATION`, `ENCODER` and the other `Tags`), chapters, attachments and
attached covers as streams, the codec details `ffprobe`'s decoders report
(read from `CodecPrivate` and the first frames' headers in `mkv/video.rs`,
`mkv/audio.rs`, `mkv/image.rs`), colour precedence and HDR, stereo,
spherical and rotation side data, decode-time rebuilding for reordered
video, lace timing, and how many packets `ffprobe`'s analysis reads before
it takes a stream's start and duration from the format's. `coreav-mkv`
reads `Tags` and `Chapters` for it, and keeps tracks of codecs it does not
name with `Allowlist::any()` (`docs/design/mkv.md`).

**MPEG-TS.** `ffprobe/ts.rs` and `ffprobe/ts/params.rs`: the programs with
their service names (from the SDT, which `coreav-mpegts` now reads), the
TS-only keys (`ts_id`, `ts_packetsize`, `id` as the PID), the values a
stream states in its first access units (sequence parameter sets, slice
headers and picture timing, MPEG-1/2 sequence headers, audio frame
headers), streams numbered as `ffprobe` numbers them (which removes
`KD-TS-STREAM-ORDER` for this command), and packets in `ffprobe`'s order
with the "MPEGTS Stream ID" side data and its rebuilt decode times
(`docs/design/mpegts.md`). The `bit_rate` of ADTS AAC is a running mean
over the frames `ffprobe`'s analysis reads: whole PES packets in file
order, up to and including the first after which some stream's latest
decoding time lies more than five seconds (450,000 ticks, strictly) past
its first timed frame's presentation time, or the first that starts past
five million bytes (a 35 Mb/s stream crafted for it; of two AAC streams
past that limit, `ffprobe` counts a few more frames of the second, which
this window does not model); each frame's rate is its
`frame_length` bytes over 1024 samples at its sampling rate, and the mean
is truncated toward zero at every frame (`m += (rate − m) / n`). It agrees
on every AAC transport stream of the fixtures and on 11 of the 14 AAC
files of the development set; on the others `ffprobe`'s mean starts or
ends at frames this window does not explain.

Streams are numbered in two steps, as if `ffprobe` read the file twice:
first the streams of the first Program Map Table completed after the PAT,
in that table's order (after the Event Information Table if its first
section comes earlier); then, reading again from the first packet, every
other stream where the file first shows it -- a Program Map Table section
that names it, even one that comes before the PAT, or a PES packet on its
PID whatever the tables have said so far, padding (`stream_id` 0xBE)
aside; streams one table names at once keep its order
(`Stream::with_first_table`, `Stream::found_at`). A PID that carries only
padding is no stream. Each program lists its streams in its own table's
order, whatever their indexes, so a PID two programs share sits where each
table puts it. Found on files crafted with `ffmpeg -program` and edited
packet by packet (a program's table delayed behind its streams' data, or
placed before the PAT; a PES packet moved before every table; padding on
a declared and on an undeclared PID), and checked on the seven
multi-program captures of the development set, whose numbering (up to 37
streams) and program lists this reproduces exactly, where before all
seven disagreed on both. Rejected: the first
PES packet after the first Program Map Table (the earlier rule), which
misses the data a capture carries before its tables, where `ffprobe`
places most streams of a capture whose PAT comes late; the packet that
completes a stream's first PES packet, which puts behind the video the
audio streams of a capture that `ffprobe` numbers before it; and listing a
program's streams in index order, which put a PID sixteen programs
share first in each where every table, and `ffprobe`, has it last.
Not followed: a file of at most 4 MB (read again from its start for its
tail) whose Program Map Table sent before the PAT names a stream the later
version drops numbers that stream first, where `ffprobe` numbers it last;
the early tables are looked for on at most `max_programs` PIDs, so a file
that opens more never-completed tables than that before its PAT falls back
to the order of the packets. A file without tables whose only PES packets
are padding has no stream and is rejected (`NoPrograms`), as a file without
tables or PES packets is.

Codec-level fields of a transport stream's first access units, each found
on crafted files and the development set:
- LOAS/LATM AAC takes the `AudioSpecificConfig` its `StreamMuxConfig`
  carries, copied to its syntactic end (ISO/IEC 14496-3 1.6.2.1, 4.4.1),
  as `extradata`; explicit SBR (object type 5 or 29) gives `HE-AAC` at the
  extension rate. A configuration this reader cannot end (a program config
  element, `epConfig` 2 or 3) falls back to the two-byte one. Parametric
  stereo signalled implicitly inside SBR data is not found: that needs the
  raw data blocks, as `KD-TS-AAC-PROFILE` describes.
- Audio frames before a stream's first stated time take that time minus
  the durations between, and the stream's start moves with them (crafted
  AAC, MPEG audio, AC-3 and Opus files). Rejected: leaving them untimed,
  which put the start a whole PES packet late. At rates whose frames are
  not a whole number of 90 kHz ticks (44.1 kHz) several untimed PES
  packets drift by a few ticks from `ffprobe`'s. Video is not timed back.
- MPEG-4 part 2 video reads the visual object sequence, visual object and
  video object layer headers (ISO/IEC 14496-2 6.2.2, 6.2.3): size, aspect,
  profile (the high nibble of `profile_and_level_indication`) and level
  (the low one, mapped from files crafted with all 256 values),
  `has_b_frames` as not `low_delay`, DivX packed bitstreams from their user
  data, and the first configuration up to the first group or picture as
  `extradata`.
- HEVC `coded_width`/`coded_height` are the uncropped picture size, and a
  display aspect ratio whose terms pass 2^20 is the closest fraction with
  both terms within it. Rejected: reducing by the greatest common divisor
  alone, which printed ratios `ffprobe` does not.
- E-AC-3 reads `bsi()` to its additional information and reports `Dolby
  Digital Plus + Dolby Atmos` when `flag_ec3_extension_type_a` is set
  (ETSI TS 103 420 8.3; clearing it in a capture drops the profile).
- H.264 `extradata` is every SPS and PPS from the first SPS to the first
  slice after it, across access units. Rejected: the first access unit's
  only, which halved it on streams that repeat their sets.
- An H.264 stream whose VUI clock cannot time one picture (a picture lasts
  under one tick) has `avg_frame_rate` 0/0 unless the first 64 units step
  evenly, within one tick; then it is `r_frame_rate`.
- Opus channel layouts follow the Vorbis order (RFC 7845 5.1.1.2), and an
  ISO 639 `audio_type` 3 sets the `visual_impaired` and `descriptions`
  dispositions.
- MPEG-1 and MPEG-2 video is listed picture by picture
  (`docs/design/mpegts.md`). A picture no PES packet times decodes one
  picture duration after the picture before it, and a B-picture, which no
  picture refers to (ISO/IEC 13818-2 6.1.1.11), is shown then too; the
  others have no presentation time. A picture lasts its frame period
  rounded down (3,753 ticks at 24000/1001). Decoding times are rebuilt as
  for H.264 reordering one picture (`DecodeTimes`), except a B-picture's,
  which keeps its presentation time. An MPEG-2 stream (not MPEG-1, which
  no measured file needed) ends one picture
  duration after its latest presentation time in the tail, whatever the
  step between its PES packets' times. Evidence: a capture with four
  pictures to a PES packet of 120 KB, one of field pictures each in a PES
  packet of its own, three captures whose first picture is a B-picture
  with a presentation time only, and streams crafted from `ffmpeg`'s at
  29.97 and 23.976 frames a second, regrouped one to three pictures (and
  5,000 bytes) to a PES packet, with every other PES packet's times taken
  out and starting one to five pictures in. Rejected: rebuilding a
  B-picture's decoding time, which gave the B-pictures that start those
  captures none; and ending the stream at the smallest step between its
  times, which ends the field-picture capture half a picture after its
  last field. Not explained, so left: a stream that never states a
  decoding time gives its first reference picture its presentation time
  less one picture (an I-picture) or its presentation time (a P-picture),
  and a reference picture after leading B-pictures the decoding time one
  picture after theirs, which no window over the presentation times
  gives (crafted streams only). For H.264 and HEVC the same exemption
  does not hold: a muxer of the corpus that states decoding times equal to
  the presentation times has `ffprobe` rebuild those of its non-reference
  pictures, while streams that state no decoding time for their
  B-pictures (an HEVC capture and crafted streams starting with a
  B-picture, or jumping forward before one) keep the presentation time.
  What tells the two apart is not found, so H.264 and HEVC keep the
  window for every picture.
- A field picture's `field_order` is its `top_field_first`, as a frame
  picture's is (`bb` for a capture whose first field is a bottom field,
  and for crafted field pictures of either parity with the flag clear;
  `tt` with it set). Rejected: `unknown` for a field picture, the rule
  until that capture.
- An SCTE 35 stream has no `codec_tag` (0, on three captures and crafted
  streams) unless its own descriptors register an identifier, which is
  then the tag, `CUEI` or any other; a stream of type 0x86 whose program
  registers no `CUEI` is not one of these, and keeps its own identifier or
  its stream type as the tag, as any other stream does (crafted streams of
  every layout). An SCTE 35 stream starts at the time of its first section
  that has one (the PCR before it) and, when the head read to the end of
  the file, ends at its last; one with no timed section is timed as a
  stream the file times nowhere. A section in the tail of a longer file
  would give `ffprobe` an end the probe does not read (none of the
  development files has one there).

**Selections.** `-show_streams` also prints the disposition and the tags of
a program's streams, empty unless `-show_entries` names their entries,
whenever the program's streams are printed; a printed program prints its
streams, as empty objects when none of their entries is selected
(`-show_entries program=program_id`).

**Prior art.** None read: every rule above was found by running the
pinned `ffprobe` on the fixtures, the development files and files crafted
with the pinned `ffmpeg`, and from the specifications. The frame-rate
estimate is the fit those crafted tests support (the variance of the times
around each standard rate's grid); a variant that also fits the grid moved
by half a frame is not used, since no measured file needs it.

**Gaps, each printed as unknown or left out, never guessed:**
`nb_read_frames` (refused when named: it needs decoding); the
`amve` ambient viewing environment of MP4 video, and other values only
decoding shows;
TrueHD's sample format and DTS-HD profiles in Matroska; `r_frame_rate` where
`ffprobe` guesses from irregular timing; and the packet times of MP4 edit
lists `ffprobe` rewrites (`KD-EDIT-*` of `docs/known-divergences.md`
describe the same differences for the packet oracle). Each key's agreement
is measured below.

## Fallback to FFmpeg

Implemented in `coreav-cli` (`fallback.rs` redacts and writes the log
line, `fallback_cmd.rs` finds and runs the program) under these rules. A
command that is not native (one refused above with exit status 4,
the commands that ask for no section included) runs the real `ffmpeg` or
`ffprobe` instead, unless strict-safe mode is on. With the fallback, the
compatibility command can stand in for FFmpeg in a pipeline that also runs
commands CoreAV does not implement.

### Strict-safe mode, for services that read untrusted input

`COREAV_STRICT_SAFE=1`, or `coreav --strict-safe COMMAND ...`, never runs an
`ffmpeg` or `ffprobe` program, in any build and for any command (maintainer
decision, 2026-10-30):

- **In place of a command.** A `coreav ffprobe` or `coreav ffmpeg` command
  that is not native is refused by name with exit status 4, as without the
  fallback.
- **As the decoder.** In a build with the `ffmpeg-fallback` backend
  (`decode.md`, "The FFmpeg fallback"), the backend is replaced by a
  stand-in that says which streams it would take and opens none. The
  policy's order is unchanged, so the native, dav1d and libvpx backends
  decode what they decode, `--exact` included, where the fallback comes
  first. When the order reaches the stand-in and no backend after it opens
  the stream, `coreav thumbnail`, `coreav transcode` and the `libx264`
  lines of `coreav ffmpeg` are refused with exit status 4, and the message
  names the codec, the build that decodes it without FFmpeg (`native-h264`,
  `dav1d`, `libvpx`) and turning the mode off. The refusal comes once the
  container is read and before any frame is decoded; the output path is
  never created, and the temporary file next to it is removed.
- **As the encoder** (`transcode.md`, "The encoder program").
  `--encoder ffmpeg` is refused, and `--encoder auto`, the default and what
  the `libx264` lines of `coreav ffmpeg` use, takes an `x264` program only
  and is refused when there is none, with exit status 4 and a message that
  names installing x264 and turning the mode off. An `x264` program the
  launcher would refuse (below), named by `--x264` or `COREAV_X264`, is
  refused the same way. All come before the input is read: a `coreav ffmpeg` line that names `libx264` takes its
  encoder from the command line alone, so in the mode it is refused
  without x264 even when its input turns out to hold no video to encode.
- **In the hidden entries.** `coreav` runs itself as its decode worker
  (`__coreav_worker`) and as the sandbox launcher of its encoder
  (`__coreav_exec`; `decode.md`, "The sandboxed worker"), before it
  parses any argument. Every launch clears the environment, so the mode
  travels in the arguments: in the mode the command starts them with
  `--strict-safe` right after the entry argument (`StrictSafe::entry`),
  and run directly they also read the variable. In the mode the
  worker refuses to host the FFmpeg backend, and the launcher refuses a
  program whose file name, or the name of the file it links to, begins
  with `ffmpeg` or `ffprobe`, with exit status 126. A copy of FFmpeg under
  another name is not recognised, as an `x264` program that is FFmpeg
  under another name is not: the mode trusts the names the operator gives.

`coreav thumbnail` and `coreav transcode` print the refusal as a
`coreav.error/1` object of kind `unsupported_feature`; `coreav ffmpeg` as
its other refusals, with the log line's `not_run` `strict_safe`.

One type decides all of them, `StrictSafe` (`coreav-cli`, `strict_safe.rs`),
made from the flag and the variable: the fallback asks it, a transcode's
encoder is an `Encoder` only it makes, the FFmpeg backend reaches the
decoder list only through `Decoders::push_ffmpeg`, which takes it, and
the hidden entries ask it, the worker `StrictSafe::worker` and the
launcher `StrictSafe::program` (`coreav_worker::worker_main` and
`sandbox_exec_main` take the check from their caller), with the mode
carried to them by `StrictSafe::entry`. A new
command that decodes or encodes goes through the same list and the same
type. Rejected: a check in each command, which a new command could leave
out; and leaving the FFmpeg backend out of the list in strict-safe mode,
which would make a stream only FFmpeg decodes a rejected file (exit 1, "no
decoder") instead of a refusal that names the way out.

Under a link named `ffprobe` or `ffmpeg` every argument is the tool's, so
only the variable works there. Any value but empty and `0` turns it on, so
a mistyped value fails closed. This is the mode for a service that reads
files it did not make, and the user documentation and the commands' help
text present it first: only in this mode does no FFmpeg code run on the
service's input, in every build. Every bar of this note is measured in it.
A check that needs FFmpeg as the decoder or the encoder (`transcode.md`, B9;
the instruction counts of `thumbnail` and `transcode` in
`scripts/bench_guard.py`) runs outside it, with `COREAV_FALLBACK` naming no
program so that a refusal still stands.

The tests (`crates/coreav-cli/tests/strict_safe.rs`, in CI with the
fallback alone and with the native H.264 decoder alone) put a stand-in
`ffmpeg` on `PATH` and in `COREAV_FFMPEG` that leaves a file behind when it
runs, and check that none runs in the mode for `coreav thumbnail` (with
and without `--exact`), `coreav transcode` and a `coreav ffmpeg` `libx264`
line, and that it does run outside the mode. They also check that a
`libx264` line is refused without reading a missing input or a pipe no
one writes to, that an `x264` program named `ffmpeg` (by `--x264`, by
`COREAV_X264` or through a link) is refused, that the launcher run
directly refuses the stand-ins and a link to one and still runs `x264`,
also when only the arguments a launch in the mode passes turn the mode on,
and that the worker run directly refuses to open the FFmpeg backend.

### Which program runs

1. `COREAV_FALLBACK=<path>`: a directory names its `ffmpeg` and `ffprobe` (with
   `.exe` on Windows); a file names that program, for the tool its file name
   names. A relative path is taken from the working directory, never looked
   up on `PATH`. When the path names the other tool, does not exist or is not
   executable, nothing runs: the refusal stands (exit 4) with one more line
   that names the variable and the reason. It never falls through to `PATH`,
   which would run a different FFmpeg than the one asked for.
2. Otherwise the first `ffmpeg` or `ffprobe` on `PATH` that this process can
   run (on Unix, `access` with `X_OK`, so an execute bit for others only does
   not count) and is not this program, made absolute before it is logged or
   run (an empty element of `PATH` is the working directory): its file identity is compared with the running
   executable's, so the multicall link and a hard link to `coreav` are
   skipped. Windows has no file identity in Rust's standard library, so
   there the bytes are compared, which skips a copy of `coreav` too.
3. None found: the refusal stands, exit 4, as today.

Only a refusal is handed on. A file CoreAV rejects (exit 1), a malformed
command (2) and an input that cannot be read (3) keep their status: the
rejection of a file is CoreAV's verdict on that file, and handing it on
would send FFmpeg exactly the files the checks stopped. No refusal writes
output, so no partial output exists. A refusal while the command line is
parsed comes before the input is opened, so FFmpeg gets standard input
unread; one while the job is planned or run comes once the input is read
("Refusal and exit codes"). Rejected: falling
back on exit 1 too, which a drop-in would like but which defeats the
checks; and a per-tool pair of variables, where one directory is how FFmpeg
installs both tools.

### Running it

The program gets the arguments unchanged, standard input, output and error
inherited, and the environment with one variable added. On Unix CoreAV
replaces itself with it (`exec`), so the process id, signals and exit status
are FFmpeg's; on Windows it starts the program, waits, and exits with its
exit code. Either way the exit status of a fallback is the program's.

**Recursion guard.** The added variable is `COREAV_FALLBACK_ACTIVE=1`. A
`coreav` started with it set, to any value, empty and `0` included, never
falls back: it refuses (exit 4) with a line
saying the fallback led back to CoreAV. That stops a loop through a wrapper
script, or a copy of `coreav` under another name, that the path comparison
cannot see. The variable reaches FFmpeg's own children too, which is
harmless: it only turns the fallback off, never a native command.

**Notice.** One line on standard error before the program starts, never on
standard output, which carries `ffprobe`'s JSON or a piped
`-f mpegts -` and must stay byte for byte the program's:

    coreav: ran /usr/bin/ffmpeg instead (not native: -vf)

The reason is the option or property the refusal would have named. The
line is printed at every log level, `-v quiet` and `-loglevel quiet` included: a
quiet level silences FFmpeg's own messages, and the notice is CoreAV's
statement that FFmpeg's code ran instead of CoreAV's, which a caller who
asked for quiet output needs as much as any other. It costs one line on
standard error and leaves standard output untouched. A fallback is never
silent; the only way not to see the line is strict-safe mode, which refuses
instead. Rejected: leaving the line out under `-v quiet`, which would make a
service that runs everything quietly unable to tell from its own logs that
FFmpeg read its inputs.

**Log.** `COREAV_FALLBACK_LOG=<path>` appends one JSON line for every command
that was not native, whether it ran FFmpeg or was refused, whatever the log
level:

    {"time": "2026-10-29T08:00:00Z", "coreav": "0.1.0", "tool": "ffmpeg", "args": ["-i", "INPUT", "-vf", "VALUE", "-c:a", "copy", "OUTPUT"], "reason": "-vf", "ran": "/usr/bin/ffmpeg"}

`ran` is null when nothing ran, and `not_run` then says why: `strict_safe`,
`no_program`, `fallback_unusable` (`COREAV_FALLBACK` names nothing that can
run, or the program could not start) or `recursion_guard`. The line is
written before the program starts, since on Unix nothing of CoreAV runs
after it; a program that then cannot start (a script whose interpreter is
missing) adds a second line for the same command, with `ran` null and
`not_run` `fallback_unusable`, and the command exits 4. The file
is opened to append and each line is one write, so processes running at
once do not interleave lines. A log that cannot be written costs one line
on standard error, at every log level, and never stops the command.

`args` is redacted when the line is written, never left for a later step,
by allow-lists of enumerated public names: a part of the command is kept
only when it is one of the names listed for its place, or a time or an
integer where the table says so, and short identifier syntax alone never
qualifies, since a metadata key or a tag value can take that form too.
Every file name, URL and other bare argument becomes a placeholder (`INPUT`,
`OUTPUT`, `FILE`), and every other value becomes `VALUE`.

- **Option names.** An option is written as its base name (`-c`, `-map_metadata`),
  up to any `=` in it, when the pinned binaries' `-h full` lists it, and as
  `-OPTION` otherwise: after an option whose arity is unknown, a value that
  starts with `-` would otherwise be logged as an option name. An option off
  the listing is taken to have a value, which becomes `VALUE`. A stream-specifier suffix is kept only when it is a
  stream type or an index (`-c:v`, `-c:a:0`, `-b:1`); any other suffix becomes
  `:SPEC`, so `-c:m:comment:private_id` is logged as `-c:SPEC`.
- **Values:**

| Options | Value kept when it is |
|---|---|
| `-ss`, `-sseof`, `-t`, `-to`, `-segment_time`, `-hls_time` | a time in FFmpeg's syntax (`classify.is_time`) |
| `-threads`, `-hls_list_size`, `-max_muxing_queue_size`, `-reset_timestamps`, `-safe` | an integer |
| `-map`, `-select_streams`, `-map_metadata`, `-map_chapters` | a stream specifier of file index, type and index only (`0`, `-0:a`, `0:v:1`, `-1`); a metadata selector (`0:m:KEY:VALUE`) becomes `VALUE` |
| the codec options (`-c`, `-c:v` ...) | `copy`, or an encoder name the pinned `ffmpeg -encoders` lists |
| `-f`, `-segment_format` | a format name the pinned `ffmpeg -formats` lists |
| `-v`, `-loglevel` | a level name or number of FFmpeg's documentation, with its `+`/`-` flags |
| `-movflags`, `-fflags`, `-avoid_negative_ts`, `-hls_playlist_type` | flags or values the pinned FFmpeg's documentation names for that option, joined by `+` or `-` |
| `-of`, `-print_format`, `-output_format` | a writer of the vocabulary, with settings only of its documented names (`nokey`, `nk`, `noprint_wrappers`, `nw`, `print_section`, `p`, `item_sep`, `s`, `escape`, `e`, `compact`, `c`, `sep_char`, `hierarchical`, `h`) and a value of `0`, `1`, an `escape` mode, or one character |
| `-show_entries` | section names of the vocabulary, and keys on the list of keys CoreAV's sections print; every key of a tags section (`format_tags`, `stream_tags`, `packet_tags`, `chapter_tags`, `program_tags`) and every key off the list becomes `KEY`, so `format_tags=private_label` is logged as `format_tags=KEY` |

The name lists are taken from the pinned binaries' own listings and from the
documentation, committed with the implementation, never from the command
being logged: `scripts/fallback_names.py` generates
`crates/coreav-cli/src/fallback/names.rs` from FFmpeg 7.0.2's `-h full`,
`-encoders`, `-formats` and `ffprobe -sections`, and the keys from
`coreav ffprobe`'s source. libaom-av1's private options are left out of it. The same redaction applies to `reason`, which names the option
or property the command was refused for, so a refusal of
`-show_entries format_tags=private_label` logs `format_tags=KEY` there too.

Every option outside the table, step 1's or not, keeps its redacted name
(`-OPTION` when it is off the listing) and loses its value. A command line can carry private data in places no list of
dangerous options foresees: `-headers "Authorization: ..."`, a URL with a
token, a proxy password, a metadata selector inside the vocabulary
(`-map 0:m:comment:...`), a metadata key in a stream specifier
(`-c:m:comment:...`) or in `-show_entries`. Keeping only enumerated public
names means the file holds nothing a user could not share, in every mode,
strict-safe included; the user still sees which option was not native, and
the project learns which commands to implement next. Rejected: accepting
any short lower-case identifier, which a metadata key satisfies; keeping
the values of the vocabulary's options, which lets a metadata selector or a
tag value through; logging the arguments as given and redacting on export,
which leaves private data on disk; and `classify.normalize` as the
redaction, which keeps the values of options it does not know.

A native command reads none of this: no lookup, no notice, no log line.

**What the fallback means for claims.** With the fallback, a command that
is not native runs FFmpeg's C code on its input, and in a build with a
decoder backend FFmpeg can decode or encode the video. The statement that no
FFmpeg code runs holds in strict-safe mode only, in every build, and every
place that makes it says so. It is not a statement that no C code runs: the
`dav1d` and `libvpx` backends are C libraries, sandboxed, in the builds that
have them.

**Not in 0.1.0: a shadow mode** that runs both tools and reports their
differences. It waits for what the fallback logs show (0.1.x).

**Tests of the implementation** need no FFmpeg: a stand-in program in a
directory named by `COREAV_FALLBACK` that prints its arguments and exits 7
shows the exit status passed through, standard output untouched, the
notice at every log level (`-v quiet` included), the log line, strict-safe refusing, and a
link to `coreav` in that directory stopped by the guard, and a `coreav
ffmpeg` command refused once its input is read (an output that exists)
handed over with the output untouched. The redaction has
its own regression cases, each a refused command with a private marker
string that must appear nowhere in the log line, `args` and `reason` alike:
`-c:m:comment:MARKER copy`, `-map 0:m:comment:MARKER`,
`-show_entries format_tags=MARKER` and `stream_tags=MARKER`, an inline
`-opt=MARKER`, `-headers "Authorization: Bearer MARKER"`, an input URL with
`MARKER` in its query, a codec value and a writer setting of `MARKER`, and a
command refused for the option that carries the marker.

## What is not here yet

A writer that clips audio alone (C15 and C19).

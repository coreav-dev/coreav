# The compatibility command: `coreav ffprobe` and `coreav ffmpeg`

Status: Proposed

Roadmap step 8.7 (1) and an earlier change: a command that accepts the common `ffmpeg`
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

Every one of those is refused by **naming it**: the error says which
option or property is not supported, and the exit status is non-zero. A
refusal is never a silent fallback.

Three option meanings the set makes unavoidable, pre-registered here so
the implementation does not get to choose later:

- **`-ss` before `-i` is input seeking, after it output seeking.** Both
  are in the vocabulary, and with `-c copy` the two produce different
  files. The oracle for either is FFmpeg running the same command, so
  FFmpeg's semantics are the bar in both positions, not an opinion of
  ours.
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
(`docs/measurements/compat-ffprobe.md`, round 4) B1 is met, B2 and B3a
are missed, and B4 was last measured in round 3 (met) and not run again on
this code. B3a now misses on the
corpus only: the fixtures print byte-identical output for every command
that runs, in all three containers (100%, with the runs a recorded known
divergence explains set aside), but on the corpus 37 of 101 ISO base media
keys and 65 of 157 MPEG-TS keys agree on fewer than 98% of files
(Matroska and WebM: 2 of 101). B2 misses on refusals by design (the
commands without a section, intervals that need a seek) and on the
fixtures CoreAV rejects, not on wrong values, so it does not gate the
label. B3b waits for `coreav ffmpeg`. The bars stay as registered. Until
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

## The command

This part is written in the pull request that builds `coreav ffprobe`,
before any measurement of it. `coreav ffmpeg` follows in its own pull
request, under the same rules.

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
**refused before the input is opened**: one line on standard error naming
the option or property and why, nothing on standard output, and **exit
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
corpus too, as the library's oracle applies them: an ISO base media file
CoreAV rejects comes under `KD-STRICT` (`docs/known-divergences.md`) when
`coreav probe` names the defect (a truncated or invalid structure, or a
limit) and the file's bytes show that kind of defect by the walk of
`scripts/oracle_kd.py`. Every key of such a file then agrees through the
record and stays in the denominator; the report counts these files and
keys apart (`run.py keys`, `via_known_divergence`). A rejection the bytes
do not explain, and any rejection of a Matroska or MPEG-TS file, still
disagrees on every key. Rejected: reading such files in `coreav ffprobe`
as `ffprobe` does (stopping at a box past the end, skipping a second movie
box), which would loosen for one command the policy the library keeps for
every reader (proposal section 6.2); and counting them as disagreement, as
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
  header's and the samples' (the samples' when fragments follow); with one
  edit, no longer than the edit presents; with several, all of them, empty
  ones in between included, rounded to the nearest tick. A data or subtitle track without
  an edit list lasts at least as long as the longest track, in its own
  ticks rounded to the nearest; a timecode track keeps its own.
- The stream `bit_rate`: PCM and IMA ADPCM their constant rate (sample
  rate, channels and bits); audio whose edit starts inside the media
  (priming), or with several edits, the average the container declares
  (`btrt`, else `esds`); otherwise the bytes over the duration, truncated,
  where video, and audio whose edit starts inside the media, take their
  whole media (edits aside) and a data track its own span. A fragmented
  movie's rate is rounded to the nearest, and with a segment index
  (`sidx`) `ffprobe` counts only the first fragment's bytes, over the
  whole duration.
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

## What is not here yet

`coreav ffmpeg`, the concatenation writer and its oracle come next, under
these bars.

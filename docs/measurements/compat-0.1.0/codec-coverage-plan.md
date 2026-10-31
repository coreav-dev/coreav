# 0.1.0 codec coverage: the plan of V1 to V3

Status: planned. Nothing of V1, V2 or V3 has run.

The bars are registered in
[`design/compat-cli.md`](../../design/compat-cli.md), "The 0.1.0 codec
coverage bars", and this page does not change them. It fixes, before any
run, what the note leaves to the run: the builds and the record of them,
how each denominator is built, the agreement rule and the oracle of each
job, how failures, timeouts and oracle failures are counted, the reasons a
miss is listed under, and the breakdowns the result reports. A change to
it after the first run of the rehearsal is recorded under "Deviations",
not edited in.

| | Bar, as registered |
|---|---|
| V1 | at least 74% of the denominator below decode, every frame agreeing |
| V2 | at least 40% of the 488 commands of the encode set count, so at least 196 |
| V3 | the share of the 630 filter-free usable commands that count by V2's rules; no bar |

## The run manifest

V1, V2 and V3 run once, on the release candidate commit named in the
round's [README](README.md), "The commit", after the tools below have
merged. Every item of this table is recorded with the result, and an item
that differs from what the table names voids the run (the note, "When they
run, and what voids a run").

| Item | What it is | Recorded |
|---|---|---|
| Commit | the release candidate, a commit of `main` | its SHA |
| `coreav` | the full build's archive binary: `packages/cli/scripts/platform.sh linux-x64 x86_64-unknown-linux-musl full` at the commit, so feature `release-full` and profile `dist`, with dav1d, libvpx and libopus built static by `packages/cli/scripts/codecs.sh` at the versions it pins | SHA-256, the features, the three library versions |
| `decode-oracle` | `tools/decode-oracle` at the commit, `--profile dist --target x86_64-unknown-linux-musl --features native-h264,dav1d,libvpx`, linked against the same static dav1d and libvpx that `codecs.sh` built for the archive | SHA-256, the features |
| `ffmpeg`, `ffprobe` | the pinned FFmpeg 9.x that `scripts/install-oracle-ffmpeg.sh` installs at the commit, checked by `scripts/oracle_pin.py --verify-bin` | the version line and the SHA-256 of each |
| `x264` | the program `scripts/install-x264.sh` builds at the commit (V2, V3) | its x264 commit and SHA-256 |
| Host | GitHub-hosted `ubuntu-24.04` x86-64 runners; for the reruns of "Host limits" below, one Compute Engine `c4-standard-16` VM | each runner job's image version, CPU model, core count, memory and kernel; the VM's class, CPU model and kernel |
| Fixtures | `scripts/gen-fixtures.sh`, `gen-fixtures-mkv.sh` and `gen-fixtures-ts.sh` at the commit, run with the pinned `ffmpeg` (V2, V3) | the SHA-256 of each of the five inputs |

Every run of the pinned `ffmpeg` and `ffprobe`, and every run of CoreAV,
sets `GCONV_PATH` to a directory with no gconv modules, as the round does.
Nothing is timed.

## V1, decode breadth

### The files and the stream

- **Files.** The 1,351 `real` files of corpus v1's manifest, held out and
  not, and no other file. They run in the order of
  `sha256("coreav-codec-v1:" + sha256)`, split into shards by position.
- **The stream.** The pinned `ffprobe` lists the file's streams
  (`stream=index,codec_type,codec_name,width,height,pix_fmt,field_order,bits_per_raw_sample`
  and `stream_disposition=attached_pic`). The stream is the video stream
  with the largest width times height, the first on a tie, leaving out
  attached pictures: the rule of `default_video` in
  `scripts/thumbnail_oracle.py`, which is FFmpeg's default and `coreav
  thumbnail`'s. A file on which `ffprobe` exits nonzero or lists no such
  stream has no stream.
- **The codec.** The stream's `codec_name` puts the file in one of five
  codec groups: `h264`, `av1`, `vp9`, `vp8`, and `other` for every other
  codec, listed by name in the result.

### The reference

The pinned `ffmpeg` decodes the whole stream to `framemd5`, prepared as
`framemd5` in `scripts/thumbnail_oracle.py` prepares it for decode.md's
bar 1, without `-frames:v`:

    ffmpeg -nostdin -v error -noautorotate [-ignore_editlist 1] [-c:v DECODER] -flags unaligned \
           -i FILE -map 0:N -fps_mode passthrough [-pix_fmt FORMAT] -noautoscale -f framemd5 -

- `-ignore_editlist 1` for the extensions that script treats as MP4 and
  MOV (`mp4`, `mov`, `m4v`, `3gp`, `m4a`, `m4b`).
- `DECODER`: `libdav1d` for `av1`, `vp9` for `vp9` and `vp8` for `vp8`
  (FFmpeg's own decoders, never its libvpx wrapper), and none for `h264`
  and `other`, so FFmpeg's default decoder runs.
- `FORMAT`: for a stream whose pixel format has alpha (`yuva*`, `gbrap*`),
  the same format without it; otherwise the option is left out and frames
  keep the decoder's format.
- `N`: the stream's index from `ffprobe`.

A file has a reference when this exits 0 within its time limit and lists
at least one frame. The denominator is the files with a stream and a
reference. The files without one are counted, never compared, under these
codes:

| Code | When |
|---|---|
| `no_stream` | `ffprobe` exits nonzero, or lists no video stream other than attached pictures |
| `reference_failed` | the reference exits nonzero |
| `reference_empty` | the reference exits 0 and lists no frame |
| `reference_unfinished` | the reference reached its time limit on the rerun host too ("Host limits") |

### CoreAV's side

`decode-oracle` gains a mode, in the tools pull request, that decodes one
whole stream:

    decode-oracle stream FILE

- It reads the file and chooses the stream as `coreav thumbnail` and
  `coreav transcode` do.
- The backends are the full build's, in the CLI's order: CoreAV's H.264
  decoder, then dav1d and libvpx, each in the sandboxed worker, with
  strict-safe mode on (`COREAV_STRICT_SAFE=1`). The full build has no
  FFmpeg fallback (`design/release.md`, "Two builds"), so no FFmpeg
  backend is on the list.
- The decode limits are the ones `coreav transcode` applies to a whole
  stream, so a stream fails a limit here exactly when a transcode of it
  would.
- It sends every packet of the stream in decode order, reading each when
  it is sent rather than holding them all, takes every frame the decoder
  returns after each packet, sends the end of the stream, and takes the
  frames left.
- It prints one JSON object: the stream index, the codec, the backend that
  opened the stream and its decoder, every frame's pts and MD5 (planes
  packed without padding, `framemd5`'s layout) in presentation order, and
  an outcome. The outcome is `ok`, or the stage that failed (`read`,
  `open`, `packet`, `eof`), the error kind (a `coreav-decode` `ErrorKind`
  name, or the reader's `coreav.error/1` kind), the message and the number
  of packets sent.

The runner runs it with the time limit below and records its exit status
and signal beside the object.

### The agreement rule

A file of the denominator counts when all of these hold:

1. `decode-oracle stream` exits 0 with outcome `ok`.
2. It decoded the stream the reference decoded (the same index).
3. Its frame count equals the reference's.
4. Each frame's MD5 equals the reference's, frame for frame, in
   presentation order.

No frame is set aside. The note allows the known divergences decode.md
records, and decode.md records none for decoded frames at this plan's
commit. A divergence applies to this run only if decode.md records it,
with the clause of the specification that decides it and a predicate over
the file's own properties, in a commit before the release candidate.
Nothing recorded after the run changes its result.

### Misses by reason

Every file of the denominator that does not count gets exactly one code,
the first that applies in this order. The tools pull request commits one
exact-match table from (stage, kind, message) to the sub-codes below,
before any corpus run. A message the table does not hold gets the sub-code
`other` with its text kept, never a classification by hand, and the table
is not edited after a corpus run.

| Order | Code | When | Sub-codes |
|---|---|---|---|
| 1 | `not_read` | the `read` stage failed: CoreAV did not open the file or find the stream | `container` (`ffprobe`'s `format_name` is neither `mov,mp4,m4a,3gp,3g2,mj2` nor `matroska,webm`), `rejected` (a `coreav.error/1` of kind `truncated_structure` or `invalid_structure`), `encrypted`, `other` |
| 2 | `other_stream` | CoreAV decoded another stream than the reference's | |
| 3 | `codec_not_in_build` | the stream's group is `other` | the codec name |
| 4 | `out_of_scope` | the group is `h264`, `av1`, `vp9` or `vp8`, and an error of kind `unsupported` ended the decode at any stage | for `h264`, from the stream's `ffprobe` properties, the first that holds: `10bit` (more than 8 bits a sample, by `bits_per_raw_sample` or `pix_fmt`), `chroma` (a `pix_fmt` other than 4:2:0, 4:0:0 included), `interlaced` (a `field_order` other than `progressive` or unknown); else `other`. For the other groups, the table's sub-codes |
| 5 | `timeout` | the runner stopped the side at its time limit on the rerun host too ("Host limits"), whatever signal or status that left | |
| 6 | `decode_error` | any other error at `open`, `packet` or `eof`, or a crash or signal of the tool or its worker that the runner did not send | `limit`, `invalid_data`, `backend`, `crash`, `other` |
| 7 | `frame_count` | the decode finished, and the frame count differs from the reference's (no frame returned included) | |
| 8 | `md5` | the counts are equal, and at least one frame's MD5 differs | |

For `frame_count` and `md5` the record also keeps both counts and the
index of the first frame that differs. The runner records each deadline it
enforces, so a process it kills at its deadline is `timeout`, never
`crash`.

### Host limits

- **Limits on the runner.** Each side of a file, the reference and
  `decode-oracle stream`, has 150 minutes of wall time. A shard is a job,
  with GitHub's limit of 360 minutes.
- **What a host limit is.** A side stopped at its 150 minutes, killed by
  the runner's memory limit, failing for lack of disk, or not reached
  because its job ended or was lost. It is decided from the runner's
  record of the process (the runner's kill, the job's end), never from a
  result. An error CoreAV reports itself, its own decode limits included,
  is a result and is final on the runner.
- **The rerun.** Every file with a host limit on either side runs again,
  both sides, on one Compute Engine `c4-standard-16` VM through
  `scripts/gcp_vm_job.py`, with the same binaries, 6 hours a side, and a
  hard time limit and a spend cap in the script within the project's
  per-task limit. The VM reads the files from the corpus store with
  read-only credentials. A side that still does not finish is
  `reference_unfinished` or `timeout`.
- The GitHub pass and the rerun together are one run. A file of the set
  with no result after the rerun voids the run, as the note says.

### The result

- The share: counted files over the denominator, with both numbers,
  against 74%.
- Per codec group: the denominator, the counted files and the share.
- Misses by code and sub-code, per codec group. For `h264` this gives the
  split into `out_of_scope` (`10bit`, `interlaced`, `chroma`, `other`),
  `decode_error`, `frame_count` and `md5`.
- The files without a reference, by code, per codec group.
- For information: the frames compared, and the number of files rerun for
  a host limit.

Public evidence holds counts only. The per-file records (each file's hash
and corpus key, its codes and its counts) never leave the runner unsealed:
`codec-coverage.yml`, the workflow the tools add, seals them before it
uploads them, as `compat-0.1.0.yml` seals its R2 and R3 records, to
`tools/compat-corpus/evidence-public.pem` alone. The run needs no other
recipient: whoever holds that key reads the `frame_count` and `md5` files,
and their list stays private. A second recipient could read every record,
so it is the maintainer's decision. It is added only if the maintainer
approves a named key, by an amendment of this page that gives the key's
fingerprint before any run, and the workflow then seals to it only when
the fingerprint matches. Until then there is none.

## V2, encode breadth

### The commands and their inputs

- **The set.** The 488 commands of class `media` in
  [`commands-0.1.0/coverage.json`](../../../tools/compat-corpus/commands-0.1.0/coverage.json),
  in the file's order. `tools/compat-corpus/tests/` already derives the
  file again and checks the note's numbers.
- **Inputs.** The words `INPUT`, `INPUT2` and `INPUT3` are input
  placeholders. Each takes one of the five fixtures in the note's order
  (`basic.mp4`, `mkv_h264_aac.mkv`, `webm_vp9_opus.webm`,
  `ts_h264_aac.ts`, `audio_aac.m4a`), the placeholder that comes first in
  the command changing slowest. For the input of `-f concat`, the
  placeholder is a list that names the fixture twice by a name relative to
  the list, in the same directory, so that the concat demuxer's safe mode
  takes it. Every other upper-case word (`FILE`, `ALL`, `CALCULATED`,
  `BITRATE`, `HERE`) is the command's own and is passed as written.
- **The output.** `OUTPUT` is replaced by the command's `output` from the
  file (`OUTPUT.mp4`, `OUTPUT%03d.jpg`), as a name in an empty directory
  of its own for each run, which is also the run's working directory.
- **The input chosen.** For each assignment in turn, the pinned `ffmpeg`
  runs the command, with 120 seconds of wall time. The first assignment on
  which it exits 0 and writes its output (for a name with a number
  pattern, at least one file that matches it; for HLS, the playlist) is
  the command's input. A command with none is `no_input`.

### CoreAV's run

`coreav ffmpeg` runs the command on that input once, with 300 seconds of
wall time, outside strict-safe mode as the note says, with:

- `COREAV_FALLBACK=/nonexistent/coreav-fallback`, as
  `scripts/transcode_check.py --compat` sets it, so that a refusal stands
  and nothing is handed to FFmpeg;
- `COREAV_FALLBACK_LOG`, a file of the run;
- `COREAV_X264` and `COREAV_FFMPEG`, two wrappers, each of which appends
  one JSON line with its arguments to the run's program log and then runs
  the pinned `x264` or `ffmpeg`. With `COREAV_X264` set, `--encoder auto`
  takes `x264` for H.264 (`design/transcode.md`, "The encoder program").

An `ffmpeg` start is an encoder start only when its one input is raw audio
samples on standard input (`-f` a raw sample format, `-i pipe:0` or `-i
-`) and its one output is one audio stream encoded with `-c:a aac`. The
tools pull request tests this rule on the arguments that the merged AAC
encode passes. Every other `ffmpeg` start makes the command `ffmpeg_job`.

### The oracles of the jobs

A command's jobs are the ones the pinned `ffmpeg` reports for it: on the
chosen input, its `Stream mapping` names, for each output stream, the
input stream it comes from and whether that stream is copied or which
encoder makes it, as `copy_check.py` already reads it. The report is taken
from a run of the command with its log level set to `info` (the command's
own `-v` or `-loglevel` value replaced, which changes no output). A copy
is therefore known by what the command asks, not by comparing packets with
the whole source, so a join, a clip or a segmented copy is a copy.
CoreAV's output streams are listed with the pinned `ffprobe` and matched
to the mapping by order; the stream listing check below requires the same
order in both outputs.

| Job in the mapping | Oracle |
|---|---|
| `copy` | a command whose every stream is copied: R3's oracle, `tools/compat-corpus/copy_check.py`, against the pinned `ffmpeg`'s output of the same command, its range and join checks included. A stream copied beside an H.264 encode: B3, as the note registers it for B9. A stream copied beside an AAC encode with no H.264 encode: `copy_check.py`'s checks of that stream |
| `libx264` | an `x264` start in CoreAV's program log, then B9 of `design/transcode.md` as `transcode_check.py --compat` applies it: B1, B2, and B4 with Q1 at least −0.50 dB and Q2 at least 30 dB, against the pinned `ffmpeg` running the same command line |
| `libopus` | CoreAV's stream is not a packet copy of a source stream (equal in count and data: an encode cannot reproduce the source's packets), then O1 to O4 of `design/transcode.md`, "Opus bars", for one output, with the pinned `ffmpeg`'s output of the same command as the FFmpeg arm: O1 as written; O2 with that output in place of the summary (`start_pts` 0, `initial_padding` equal to its, `duration_ts` within 48 samples of its); O3's bound for one input, 0.85 to 1.15 (its median is over a set and does not apply to one command); O4 as written, 1.0 dB, for the sources below |
| `aac` | an encoder start in CoreAV's program log, then A1 to A4 of `design/transcode.md`, "AAC bars", for one output in the same way: A1 as written; A2 with the FFmpeg arm's output in place of the summary, exactly; A3's bound for one input, 0.95 to 1.05; A4 as written, 0.5 dB, for the sources below |
| any other encoder, image encoders included | none registered at this plan's commit |

The fixtures' audio is a 440 Hz sine, a tonal source: at 48 kHz in
`mkv_h264_aac.mkv`, `webm_vp9_opus.webm`, `ts_h264_aac.ts` and
`audio_aac.m4a` (`ASRC` and `A` in the generators), and at 44.1 kHz in
`basic.mp4`, whose sine in `SRC` sets no rate. O4 and A4 are applied as
registered, with no extension. O4 judges the tonal sources at 48 kHz and
leaves out the 44.1 kHz source S6, so it judges an Opus output whose input
is one of the four 48 kHz fixtures, and an Opus output from `basic.mp4` is
judged on O1 to O3 only. A4 judges the tonal sources S1, S2, S3 and S6,
44.1 kHz included, so it judges every AAC output. The fixtures are used as
the generators make them. The reference of O4 and A4 is the source's audio
decoded by the pinned `ffmpeg` to 48 kHz float with the command's own
timing options (`-ss`, `-t`, `-to`, `-sseof`, `-itsoffset`), and both arms
are compared with it over the presented samples.

If a design note registers an oracle for another job (CoreAV's JPEG and
PNG images, for example) before the release candidate, this page is
amended before any run to name it, and the tools implement it. Otherwise
that job's commands miss as `no_oracle`.

### Counted, and misses by reason

A command counts when `coreav ffmpeg` exits 0 within its time, the
fallback log has no line for it, every start in the program log is an
encoder start, every output stream's job has an oracle and passes it, and
the output lists the same streams, by kind and codec, in the same order,
as the pinned `ffmpeg`'s output of the same command. Every other command
gets the first code that applies:

| Order | Code | When | Kept with it |
|---|---|---|---|
| 1 | `no_input` | no assignment on which the pinned `ffmpeg` succeeds | |
| 2 | `refused` | exit status 4 | the fallback log line's `reason`, the option CoreAV names, or `none` without a line |
| 3 | `rejected` | exit status 1 for a truncated or invalid structure | the kind |
| 4 | `timeout` | the runner stopped the run at its 300 seconds, whatever signal or status that left | |
| 5 | `failed` | any other nonzero exit, a signal the runner did not send, or exit 0 with no output | the status and the last line of standard error |
| 6 | `ffmpeg_job` | a start in the program log that is not an encoder start | |
| 7 | `no_oracle` | an output stream whose job has no registered oracle | the job |
| 8 | `oracle` | a check of an oracle fails | the first check that failed (`copy_check`, `B1` to `B4`, `O1` to `O4`, `A1` to `A4`) |
| 9 | `streams_differ` | the stream listing differs from the pinned `ffmpeg`'s | |

A fallback log line whose `ran` is not null voids the run, as the note
says.

### The result

- The share: counted commands over 488, with both numbers, against 40%
  (196).
- The count per output container (the file's `container`).
- Misses by code.
- **Unhandled options.** The `refused` commands per logged `reason`, by
  count. A refused command counts once, under the option CoreAV named.
- **Per encoder.** Each command is put in the groups of the encoders the
  pinned `ffmpeg`'s mapping names for it: `x264` (`libx264`), `libopus`,
  `aac`, `other encoders` (any other encoder, image encoders included) and
  `copy only` (every stream copied). A command can be in several groups.
  For each group: its commands, the counted ones and its misses by code.

## V3, the native share

The 630 commands of `coverage.json`: V2's 488, run as above, and the other
142, run the same way with their outputs as `output` names them. A pipe to
standard output (`-`) is captured to a file, which is the output; the null
device and every other name are passed as written. An address (`rtmp://`,
`http://`, `udp://`, `rtsp://`) or a device is not provided, so the pinned
`ffmpeg` cannot run such a command, and it is `no_input`. A command whose
job is decoding only (`-f null`), subtitles or metadata has no registered
oracle and is `no_oracle`.

The result: the counted commands over 630, the counts per class of
`coverage.json` (`media`, `unknown`, `decode-only`, `subtitles`, `other`),
misses by code, and beside them the 79 commands `classify.py` labels
`native`. There is no bar.

## The tools

One pull request adds them, tested only on media its tests make: the
fixtures, and streams the tests encode with the pinned `ffmpeg` for the V1
codes (10-bit, 4:2:2 and interlaced H.264, a stream whose reference lists
more frames than CoreAV returns, a stream with one frame changed, a
container CoreAV does not read). It reads no corpus file.

1. `decode-oracle stream` (above), with a test that its frames equal
   `framemd5`'s on the fixtures.
2. `tools/compat-corpus/decode_breadth.py`: V1's runner (the stream, the
   reference, CoreAV's side, the reason table, the summary and the sealed
   records), and the merge of the GitHub pass with the rerun.
3. `tools/compat-corpus/encode_breadth.py`: V2's and V3's runner, calling
   `copy_check.py`, the B1 to B4 checks of `transcode_check.py`, and the
   checks of `opus_encode_check.py` and `aac_encode_check.py`, rather than
   copies of them.
4. A `workflow_dispatch` workflow, `codec-coverage.yml`, with the commit
   and the set (`rehearsal` or `v1`) as inputs. It
   builds the manifest's binaries once, runs V1's shards with the corpus
   store's read-only credentials, and runs V2 and V3 in a job of their own
   that reads no corpus file. A pull request that changes the workflow
   runs V2 and V3 on the fixtures, and V1 on the decode fixtures only.
5. A driver for the rerun host over `scripts/gcp_vm_job.py`.

## Order

1. This plan merges.
2. The tools pull request merges.
3. **Rehearsal.** The workflow at the tools' merge commit, on the first 40
   `real` files of corpus v1 in the order of
   `sha256("coreav-codec-rehearsal:" + sha256)`, with V2 and V3 in full.
   Its numbers are not reported; it exists to find the tools' faults. Only
   a fault of a tool (a crash, a wrong record, a step that cannot finish)
   changes the tools after it, never a result. Its records are kept,
   sealed as the run's are, and a change it causes is recorded under
   "Deviations".
4. The release candidate commit is named in the round's README.
5. **The run.** The workflow at the release candidate with the set `v1`,
   then the rerun of the files with a host limit, then the result. Once: a
   run that is not void is final for its commit, and a miss is never a
   reason to repeat it.
6. **The record.** `docs/measurements/compat-0.1.0.md`, as the note says:
   V1, V2 and V3 with their verdicts, the manifest above and the
   breakdowns, with the public summaries beside it in this directory.

## Deviations

None yet.

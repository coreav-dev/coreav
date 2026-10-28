# `coreav extract-audio`

Status: Accepted. Scope: the stage 3 stream-copy primitives (use case 6) and the command over `extract_audio` that the M4A
extraction's API leaves to a separate change.

## What it does

    coreav extract-audio [--stream N] [--allow CODECS] [--strict] [--force] INPUT OUTPUT

Copies one AAC track of an MP4/MOV file into an M4A file with
`coreav_isobmff_write::extract_audio` and the library defaults (`Limits`,
`Allowlist`), and prints a `coreav.extract-audio/1` summary
(`docs/schema/extract-audio-v1.md`). The library does the work and is
measured (`docs/spikes/audio-extract-bench/`); the
command adds argument parsing, the output file and the summary. Its library
half is `coreav_cli::extract` (`run`, `run_slice`, `document`), so other
builds of the command can use the
same serializer.

## Conventions shared with `coreav clip`

The command follows `coreav clip`, and both now go
through `coreav_cli::output`:

- Exit codes: 0 success, 1 the file was rejected or cannot be extracted, 2
  usage error (including an existing OUTPUT without `--force`, or OUTPUT
  naming the input), 3 I/O error or a source that changed between passes,
  141 standard output closed by its reader.
- Every write to standard output is checked (the summary, and the output
  itself with `-`); a command never exits 0 when a write to standard
  output failed. A closed pipe (`coreav extract-audio f - | head -c 100`) ends
  it with 141, what a shell reports for a process ended by `SIGPIPE`
  (Rust ignores the signal, so the command exits with that status itself),
  without a message; any other write error (a full disk) is exit 3. With
  `-`, a summary that cannot be written to standard error is exit 3 (141
  for a closed pipe), without a message. Rejected: exit 3 with an error
  for a closed pipe, which makes `| head` print errors and differ from
  every other command-line tool; restoring `SIGPIPE`'s default action so
  the signal ends the process, which needs a C call (`unsafe`) and would
  change it for a Python process running the same code; a silent exit 0,
  which hides a lost document. The `coreav` command's crate documentation
  (`crates/coreav-cli/src/main.rs`) states the rule for every command.
- Errors are a `coreav.error/1` object on standard error.
- OUTPUT `-` writes to standard output; the summary then goes to standard
  error. Otherwise the output is written to a new temporary file next to
  OUTPUT and renamed into place only after the last byte is flushed, so a
  failure the command handles never leaves a partial file at OUTPUT or
  touches an existing one (a killed process can leave the `.part` file; the
  output is not synced to disk). The temporary file is
  `.{OUTPUT name}.coreav-{pid}.part`, with the OUTPUT name shortened on a
  character boundary so the whole name stays within 255 bytes, the usual
  file-name limit: an OUTPUT name that fits always gets a temporary file
  that fits. Rejected: a short random name, which would hide
  which output a leftover `.part` file belongs to. Without `--force` the
  file is moved into place by a hard link, which fails rather than replace
  a file created at OUTPUT while the command ran (a rename after one more
  check where the file system has no hard links); with `--force` it is
  renamed over OUTPUT, replacing a symbolic link rather than writing
  through it, and keeps the replaced file's permissions. An OUTPUT that is
  a directory is refused before any work. Cleaning up the `.part` file
  when the process is interrupted would need a signal handler (a C call or
  a dependency, for the reason given for `SIGPIPE` above); it is not done.
- `--allow` and `--strict` take the names of `coreav probe`.

## Choices

- **`--stream N` names the track** by its position among all tracks of the
  file, as `ffprobe`'s `index` and the probe output's `stream_index` do; without it, the first audio track the allowlist
  keeps. Rejected: `--audio N` as in `coreav clip`, where it chooses among
  two kinds of track; here there is only one kind, and `-map 0:N` users read
  `--stream` directly.
- **No track of the asked kind is `unsupported_feature`.** `Error::NoSuchTrack`
  (no audio track, or no stream at the index) used to map to
  `invalid_structure`, written when only probe existed and never asked for a
  track. For the commands that write, it is a refusal of the request, not a
  malformed file, so it is now `unsupported_feature` with the message "no
  such track"; this also applies to `coreav clip` on a file without video.
  Rejected: a new error kind, which would extend `coreav.error/1` for one
  case the existing kind already describes.
- **One output helper for every writing command.** `output::to_file` holds
  the temporary-file-and-rename logic and `output::refuse` the overwrite and
  same-file checks that `coreav clip` had inline, so the next writing
  commands (subtitles, chapters, normalization) reuse them instead of copying
  them. `clip::ClipFailure` stays as an alias of `output::WriteFailure`.
- **The summary is small.** Stream index, track ID, codec, sample count and
  duration, as `extract_audio`'s `Summary` reports; the codec is always
  `aac`, kept so that a later codec does not change the
  shape. Rejected: copying the probe document of the output, which a caller
  can get from `coreav probe OUTPUT`.

## Verification

- `crates/coreav-cli/tests/cli.rs`: the summary against
  `docs/schema/extract-audio-v1.schema.json`, output bytes equal to the
  library's for the same request (to a file and to standard output), the
  refusals and their error kinds, exit codes, `--force`, never writing over
  the input, and no partial or temporary file after a failure.
- On every AAC stream of every MP4, MOV and M4A fixture (CI, `fixtures`
  workflow), the binary's output equals the library's bytes, and
  `scripts/extract_check.py` checks the binary's outputs with the same
  oracles as the library's: `ffprobe` stream and packet hashes, `ffmpeg
  -copyts -f framemd5`, and MediaInfo. Bar: 100% of the fixture streams, as
  for the library. No corpus run: the bytes are the library's, which
  was measured on corpus v1 in `docs/spikes/audio-extract-bench/`.
- No new fuzz target: the command reads no untrusted bytes outside
  `extract_audio`, which has one (`coreav-isobmff-write/fuzz`,
  `extract_audio`).

# coreav

The `coreav` command for Node.js. It probes MP4, MOV, Matroska and WebM
files and prints JSON. It cuts keyframe-aligned clips, splits files at
keyframes, remuxes to MP4 with the movie header first (faststart) and
removes metadata. It extracts AAC audio, subtitles and chapters. All of
this works by stream copy, without re-encoding. It also checks files
against a JSON upload policy.

```sh
npx coreav probe video.mp4
```

Installing the package installs the native `coreav` binary for Linux
(x86-64, arm64), macOS (arm64, x86-64) or Windows (x86-64). On other
platforms it runs the same command built as WebAssembly under Node's
built-in WASI.

Node.js 20 or later. Stdout, output files, exit codes and the `kind` of
errors are those of the native command on every platform (tested on every
fixture); in the WebAssembly build an error's `message` may word its cause
differently.

## Untrusted input: strict-safe mode

If you run `coreav` on files you did not make, such as uploads, turn on
strict-safe mode: set `COREAV_STRICT_SAFE=1`, or pass `--strict-safe`
before the command (`coreav --strict-safe ffprobe ...`). Any value of
`COREAV_STRICT_SAFE` other than empty or `0` turns it on.

The mode concerns `coreav ffprobe` and `coreav ffmpeg`, which take
`ffprobe` and `ffmpeg -c copy` command lines. CoreAV implements a subset of
those command lines. Without strict-safe mode, a command line outside it
runs the real `ffprobe` or `ffmpeg` program on `PATH` (or the one
`COREAV_FALLBACK` names), which is FFmpeg's C code, and CoreAV prints a
one-line notice on stderr. In strict-safe mode CoreAV refuses it with exit
code 4 instead, and no FFmpeg code runs. The other commands never run
FFmpeg. Details: <https://coreav.dev/docs/strict-safe>.

## Commands

| Command | What it does | Prints on stdout |
|---|---|---|
| `coreav probe FILE` | Describes an MP4, MOV, Matroska or WebM file | `coreav.probe/1` (MP4/MOV) or `coreav.probe-matroska/1` JSON |
| `coreav probe --ffprobe-compat FILE` | The same, in the shape of `ffprobe -of json -show_format -show_streams` | ffprobe-shaped JSON |
| `coreav check --policy POLICY.json FILE` | Checks a file against an upload policy (codecs, resolution, duration, size and more) | `coreav.verdict/1` JSON, also for a rejected file |
| `coreav clip --start S [--end E] INPUT OUTPUT` | Cuts a clip of an MP4/MOV file, starting at the keyframe at or before `S` seconds | `coreav.clip/1` summary |
| `coreav remux INPUT OUTPUT` | Rewrites MP4, MOV, Matroska or WebM as MP4 with the movie header first | `coreav.remux/1` summary |
| `coreav sanitize INPUT OUTPUT` | Like `remux` for MP4/MOV, keeping only allowlisted boxes: drops location, device, dates and other metadata | `coreav.remux/1` summary |
| `coreav extract-audio INPUT OUTPUT.m4a` | Copies the AAC track of an MP4/MOV file into M4A | `coreav.extract-audio/1` summary |
| `coreav extract-subtitles INPUT OUTPUT.vtt` | Writes the text subtitles (mov_text) of an MP4/MOV file as WebVTT | `coreav.extract-subtitles/1` summary |
| `coreav split --every S INPUT DIR/%03d.mp4` | Splits an MP4/MOV file at keyframes into MP4 segments | `coreav.split/1` summary |
| `coreav chapters FILE` | Lists the QuickTime chapters of an MP4/MOV file | `coreav.chapters/1` JSON |
| `coreav ffprobe ARGS` | Experimental. Runs an `ffprobe` command line over CoreAV's own probes | What `ffprobe` prints, in its writers |
| `coreav ffmpeg ARGS` | Experimental. Runs an `ffmpeg -c copy` command line with CoreAV's own writers | Nothing |

`coreav ffprobe` and `coreav ffmpeg` stay experimental until they meet
their 0.1.0 bars.

`coreav --help` and `coreav COMMAND --help` list every option. `OUTPUT` may
be `-` to write the file to stdout, and the summary then goes to stderr. An
existing output file is kept unless `--force` is given, and a failed run
leaves no partial file (`split` keeps the segments it finished).

Examples:

```sh
npx coreav probe --ffprobe-compat input.mov
npx coreav clip --start 12.5 --end 42 input.mp4 clip.mp4
npx coreav remux input.webm output.mp4
npx coreav sanitize --keep none phone.mov clean.mp4
npx coreav check --policy policy.json upload.mp4 && echo accepted
```

A policy (`coreav.policy/1`) looks like this; see the schema and examples in
the repository's `docs/schema/`:

```json
{
  "schema": "coreav.policy/1",
  "container": { "formats": ["mp4", "mov", "webm"] },
  "duration": { "max_seconds": 600 },
  "streams": {
    "video": { "codecs": ["h264", "hevc", "vp9", "av1"], "max_count": 1, "max_width": 3840, "max_height": 2160 },
    "audio": { "codecs": ["aac", "opus"], "max_count": 1 }
  }
}
```

## Results and errors

| Exit code | Meaning |
|---|---|
| 0 | Success; for `check`, the file is accepted |
| 1 | The file was rejected (unsupported, damaged, not allowed); for `check`, the verdict is on stdout |
| 2 | Usage error (bad arguments, invalid policy) |
| 3 | I/O error (missing file, unwritable output) |
| 4 | `coreav ffprobe` or `coreav ffmpeg` refused a command line it does not implement (see strict-safe mode) |
| 141 | Stdout was closed by its reader (for example `coreav probe f \| head`) |
| 70 | The WebAssembly module could not run (the WebAssembly build only) |

On exit 1 and 3, stderr holds one JSON object (except a rejection by
`check`, which is the verdict on stdout):

```json
{"kind":"unsupported_container","limit":null,"message":"not an ISO base media or QuickTime file","offset":null,"schema":"coreav.error/1"}
```

`kind` is stable and meant for programs; `message` is for people. Usage
errors are plain text starting with `error:`.

## Paths

Absolute paths and paths relative to the working directory work as with
any command. With the native binaries, an input may also be an `https://`
URL, read by byte ranges. The WebAssembly build reads local files only,
and refuses inputs that name a URL (`scheme://...`, exit 2).

With the WebAssembly build only:
- On Windows (installs without the native package), paths must be on the
  working directory's drive, not another drive or a network share; `\`
  and `/` both work.
- The native command refuses an output path that is a hard link to the
  input. The WebAssembly build cannot tell hard links apart, because WASI
  does not expose file identities. It reports that the output exists, or
  with `--force` replaces that link. The input file itself is never
  changed: outputs are written to a new file and renamed into place.

## Speed

Installed (`npm install coreav`, then `coreav ...`, or `npm install -g
coreav`), `coreav` is the native binary on Linux and macOS: no Node starts
first (measured on Linux: about a millisecond to start). A small Node
launcher starts the binary instead, adding about 30 ms per call, in three
cases:
- on Windows;
- in installs that run no install scripts (`--ignore-scripts`);
- with package managers that skip dependencies' scripts by default (pnpm 10
  and later, Bun) unless they are allowed.

The WebAssembly build adds about 60 ms per call. `npx coreav` adds
npm's own start-up, about 0.2 s, to every call; for many files, install
the package and call `coreav` directly. Measurements against FFmpeg: the
repository's `docs/measurements/npx-coreav.md`.

## More

Documentation: <https://coreav.dev/docs>. Source, design notes, schemas and
measurements: <https://github.com/coreav-dev/coreav>. Licensed under MIT or Apache-2.0,
at your option.

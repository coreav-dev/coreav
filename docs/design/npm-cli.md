# The `coreav` command on npm (`npx coreav`)

Historical results below that name `--ffprobe-compat` measure the retired
command. They do not measure its replacement,
`coreav ffprobe -of json -show_format -show_streams`.

`npx coreav ...` runs the `coreav` command with nothing to install but
Node. The npm package `coreav` (`packages/cli`) depends, as optional
packages, on one native binary per platform (`@coreav/cli-<platform>`). On
install, the `coreav` command becomes the binary for the platform at hand.
Where no binary applies, the package runs the command built for WASI under
Node's built-in WASI. This is the npm half of the zero-install CLI and
registry-readiness work. Publishing is the maintainer's decision; the packages
are ready up to `npm publish --dry-run`.

## What exists

| Path | What |
|---|---|
| `packages/cli/bin/coreav` | The `coreav` command: replaced by the native binary on install where possible, otherwise the Node launcher. |
| `packages/cli/lib/main.js` | The executable Node launcher: starts this platform's binary, else the WASI build. Also restores the command after an optional binary is removed. |
| `packages/cli/lib/wasi.mjs` | The WASI fallback: runs `wasm/coreav.wasm` under `node:wasi`. |
| `packages/cli/install.js` | The install script that makes `coreav` the native binary. |
| `packages/cli/scripts/build.sh` | Builds `coreav-cli` for `wasm32-wasip1` into `wasm/coreav.wasm`. |
| `packages/cli/scripts/platform.sh` | Builds one native package into `npm/<platform>/`: the release's default build (`docs/design/release.md`, "Two builds"). |
| `packages/cli/scripts/codecs.sh` | Builds dav1d, libvpx and libopus as static libraries for the native binaries. |
| `packages/cli/test` | The WASI build and the launcher against the native command; the install script; `smoke.sh`, CI's install-and-run check. |
| `bench/npx` | The speed measurement below. |

Users can run the subcommands of `coreav` that read and write files
(probe, check, clip, extract-audio, remux, sanitize, split,
extract-subtitles, chapters) through `npx coreav` with Node 20 or later.
The native binaries also read inputs at `https://` URLs by byte ranges, as
the command does. The WASI build has no network, so it refuses URLs with a
usage error (exit 2) and reads local files only. It is built without the
command's `net` feature.
They get the native command on Linux (x86-64 and arm64, glibc or musl),
macOS (arm64 and x86-64) and Windows (x86-64), and the WASI build
elsewhere, with the same output, output files and exit codes.
The native binaries outside Windows also have `coreav thumbnail` and
`coreav transcode`, with dav1d (AV1) and libvpx (VP8, VP9) linked statically,
as the release's default build has them. The WASI build has neither: WASI
cannot start the sandboxed decoder worker or the encoder program
(`docs/design/release.md`, "npm").

## Design

### Installation update plan

Keep the platform packages and the direct native command on Unix. Replace
the install-time binary copy with a relative symbolic link to the platform
package, so that one install stores the binary once. Keep the Node launcher
when install scripts are blocked or a native binary is unavailable, and
document how pnpm and Bun users can allow the script. Verify real tarballs
with npm, pnpm and Bun, including the blocked-script path. Update the
installation guidance and measured artifact sizes without changing the
recorded speed rounds below.

### Native binaries, with the WASI build as the fallback

The first version of the package shipped only the WASI build. It measured
correct and slow (round 1 below): every call paid about 60 ms to start Node
and the module, while FFmpeg's command line finished the whole job in 7 to
24 ms on typical files. A command that is many times slower than FFmpeg
through npm would misrepresent the native command, which takes 0.13 to
0.21 times FFmpeg's time. The package therefore follows the pattern of
esbuild and Biome:

- **One package per platform**, `@coreav/cli-linux-x64`, `-linux-arm64`,
  `-darwin-arm64`, `-darwin-x64` and `-win32-x64`. Each holds its
  binary and notices and is selected by npm through its `os` and `cpu` fields. The
  main package lists all of them as `optionalDependencies` of its own
  version, so npm installs the one that fits and skips the others.
- **The install script makes the binary the command.** `install.js` runs
  after install. If this platform's package is there and its binary prints
  this version, a relative symbolic link replaces `bin/coreav`, which npm has
  already linked as the `coreav` command. A call then starts the binary
  directly, with no Node in between.
  - It acts only inside `node_modules`, so a checkout's tracked launcher
    stays what `npm pack` ships; `prepack` refuses a `bin/coreav` that is
    not the launcher.
  - It installs only the platform package's binary, never one that
    `COREAV_BINARY` names.
  - It creates the link through a temporary name of its own process. The
    platform package remains the binary's only copy, and the relative link
    still resolves if the installation directory moves.
  - If the filesystem refuses symbolic links, the Node launcher stays.
  - A rebuild after the optional binary is removed, rejected by the version
    check or unable to execute links the command to the canonical Node
    launcher, `lib/main.js`, instead. The script never writes through the
    previous link, even when its destination is missing.
    After `npm prune --omit=optional`, run `npm rebuild coreav` to restore
    that launcher. Pruning alone does not run the install script.
  - pnpm and Yarn link the binary the same way: their shims see no shebang
    and run it directly.
- **The Node launcher runs everywhere else.** `bin/coreav` starts out as a
  small Node script (`lib/main.js`). It runs this platform's binary with the
  caller's arguments, streams and exit code. It passes SIGINT, SIGTERM
  and SIGHUP on to the binary, so that stopping the launcher stops the
  command, and it ends as the binary ended, also when a signal ended it.
  Without a binary it runs the WASI build. It also falls back to the WASI
  build when the binary cannot be spawned (`ENOENT`, `EACCES`, or an
  error that `spawn` throws at once, as macOS does for `ENOEXEC`). A binary
  that starts and then fails is not retried; on Linux, one of another
  architecture starts through `/bin/sh` and exits 126. The install script
  has run the binary once already. On Windows only SIGINT and SIGTERM are
  forwarded. These cases go through it:
  - Windows, where npm's command shims always run Node;
  - installs without scripts (`--ignore-scripts`), and package managers
    that skip dependencies' scripts by default (pnpm 10 and later, Bun);
  - platforms without a package.

  It costs about 28 ms of Node start-up per call. `COREAV_BINARY` names
  another binary, for tests and for platforms the packages do not cover.
- **The binaries** are `coreav` built with the workspace's `dist` profile
  (what is shipped: the release profile optimized across crates, `lto =
  "fat"`, one codegen unit), stripped for the package
  (`CARGO_PROFILE_DIST_STRIP=symbols`).
  - Linux binaries are static, linked against musl, so one build runs on
    glibc and musl systems alike. CI builds each architecture on its own
    runner; a local arm64 build on x86-64 links with the toolchain's own
    `rust-lld`.
  - dav1d, libvpx and libopus are compiled with `musl-gcc`
    (`scripts/codecs.sh`), natively on each architecture.
  - The crates that compile C, `ring` for the HTTPS of URL inputs, use zig
    as the musl C compiler for both architectures (`scripts/zig.sh`). CI
    installs zig from PyPI (`ziglang`, MIT), pinned by version and hash.
    It is a build tool: of it, only musl's C headers are used; the C
    runtime linked is Rust's own self-contained musl. Rejected: a
    musl GCC, which Ubuntu packages for the host architecture only, and
    building without URL support, which the native command has).
  - Windows binaries link the C runtime statically (`crt-static`), so they
    need no Visual C++ redistributable.
  - macOS binaries are built on their own architecture.

Rejected:

- **The WASI build alone** (round 1): one artifact for every platform, but a
  fixed 60 ms per call before any work, and 14 to 39 times FFmpeg's time
  through `npx`.
- **A Node launcher on every platform, without the install script.** Biome
  ships that way. It costs Node's start-up, about 28 ms, on every call,
  which on small files is several times FFmpeg's whole run.
- **A `postinstall` that downloads the binary.** It needs network access
  during install, which locked-down installs refuse, and it moves integrity
  checks out of npm's lockfile.
- **One package with every binary.** Each install would download all five
  to use one. The macOS arm64 binary alone is 11.65 MB before compression
  (see the artifact sizes below).
- **glibc Linux binaries.** They tie the binary to the oldest glibc of the
  build machine and do not run on musl systems. A static musl binary runs
  on both. Measured against the native glibc build on development files, it
  was within a few percent (round 2 reports the ratio).

`npx` adds npm's own start-up, about 0.2 s per call, before any of this
runs. No package can remove it; it is reported apart, as the cost of the
launcher npm provides.

### The WASI build

The fallback is `coreav.wasm`, the `coreav` binary built for
`wasm32-wasip1` with the `edge` profile and `wasm-opt -O3`: 4.23 MB,
1.77 MB compressed with gzip -9. Without Binaryen it is 5.14 MB,
1.94 MB compressed. `lib/wasi.mjs` runs it with `node:wasi`.

Rejected for the fallback:

- **The `wasm-bindgen` module (`coreav-wasm`) with file I/O in JavaScript.** It
  covers fewer subcommands than the command and would need a second
  command-line front end; the WASI build is the command itself, so new
  subcommands and options reach npm with no package change.
- **A WebAssembly runtime package (`wasmtime` and similar).** A native
  dependency again; Node's WASI is built in.

### WASI files: the host's root as `/`, the working directory passed in

The WASI launcher preopens the host's root directory as `/` and passes the
caller's working directory in `COREAV_CWD`; on WASI, `coreav` enters it
before it parses its arguments (`std::env::set_current_dir`, which
`wasi-libc` emulates). Absolute paths and paths relative to the working
directory then mean what they mean to the native command.

Rejected: preopening the working directory as `.` next to `/`. `wasi-libc`
resolves relative paths against the preopen that comes last, and with `.`
last absolute paths stop resolving (observed with Node 24); one preopen and
a real working directory has no such order.

Node's WASI is not a security boundary (Node's documentation says so), and
the launcher does not use it as one: the WASI build reads and writes what
the user names, with the user's permissions, as the native command does.

With the WASI build on Windows (installs without the native package), the
root is that of the working directory's drive. Arguments,
and the values of `--name=value` arguments, are rewritten before they
reach the module: `\` becomes `/` and a leading drive letter equal to the
current drive is dropped. A path on another drive or a network share
cannot be reached; the launcher refuses it as a usage error (exit 2)
rather than let it fail as a missing file.

### What the command changes for WASI

- `std::process::id` is unsupported on WASI and panics. The temporary
  output file's name (`.{name}.coreav-{tag}.part`) uses the clock's
  nanoseconds there, read once per process, instead of the process id;
  opening it with `create_new` still refuses a name in use.
- `COREAV_CWD`, read on WASI only.

Both are WASI only; the native command is unchanged.

What the WASI build does differently from the native command, documented in
the README:

- An error's `message` can name the cause differently (WASI error numbers
  and kinds); its `kind`, the exit code and stdout do not change.
- A hard link to the input given as the output is not recognized as the
  input: `std` has no file identity on WASI. The command then reports that
  the output exists, or with `--force` replaces that link; the input file
  is never written, since outputs are written to a new file and renamed.

### The WASI launcher

- Standard streams and the exit code pass through (`returnOnExit`), so
  stdout carries the JSON documents and stderr the `coreav.error/1` objects
  and usage errors, as natively. A module that fails to load or traps
  exits with 70 and one line on stderr; the command itself never panics.
- Node prints an `ExperimentalWarning` when WASI is first loaded. The
  launcher drops that one warning, so that stderr holds only the command's
  errors, which callers parse; every other warning is printed.
- Node opens pipes and sockets on stdio in non-blocking mode, and its WASI
  hands `EAGAIN` to the module, so a write to a reader slower than the
  command (`coreav clip ... - | upload`) failed with "operation would
  block". The launcher sets the stdout and stderr handles blocking again
  (`setBlocking(true)` on the handle, an undocumented Node method; the
  call is skipped where it does not exist, and the slow-reader test fails
  if it stops working). Rejected: retrying on `WouldBlock` in the command,
  which would wrap every write the command makes, on WASI only, to undo
  what the host did.
- Node 20 is the oldest release whose `node:wasi` takes
  `version: "preview1"`; `engines` says `>=20`. The native path needs only
  `child_process`, so it works on any of them.

### Registry metadata

`package.json` has a plain description, keywords naming what the command
does and reads, the repository, issue tracker and license (`MIT OR
Apache-2.0`, with both license files and `NOTICE` in the tarball, `docs/release/notices.md`). The README is usage
documentation for a reader new to the command: every subcommand, its JSON
output, exit codes, the error object, paths and per-call start-up cost. As
for the Python package (`docs/design/python.md`), comparative or claim
wording and keywords are positioning and the maintainer's to decide.

### Artifact sizes

Measured for package version 0.0.1 on macOS arm64, with Rust 1.99.1,
the native `dist` build with `release-default`, and the WASI `edge` build.
MB below means 1,000,000 bytes. These are artifact sizes, not speed results.

| Artifact | Bytes | gzip -9 bytes |
|---|---:|---:|
| macOS arm64 native binary | 11,651,632 | 5,618,582 |
| WASI module, without Binaryen | 5,141,655 | 1,941,108 |
| WASI module, Binaryen 133 `-O3` | 4,232,892 | 1,772,426 |

`npm pack` with the unoptimized WASI module produced a 1.97 MB main
tarball (5.55 MB unpacked) and a 5.67 MB macOS arm64 platform tarball
(12.59 MB unpacked). Installing both with npm stored 18.14 MB of regular
files, with one 11.65 MB native binary. The relative link replaces the
former second copy, saving 11,651,632 bytes on this macOS build.
The WASI module remains in the main package even on native platforms.

Reproduce the artifact counts with `wc -c FILE` and
`gzip -9 -n -c FILE | wc -c`. Run `npm pack --json` in `packages/cli`
and `packages/cli/npm/darwin-arm64` for packed and unpacked sizes, then
install both tarballs in an empty project. Count regular-file sizes under
`node_modules` without following symbolic links. Other platforms and
builds have different sizes.

The postinstall script remains necessary to avoid starting Node for every
native Unix call. With pnpm 10, add `coreav` to `onlyBuiltDependencies` in
`pnpm-workspace.yaml`. With pnpm 11 and later, use `allowBuilds: { coreav: true }`
instead. Then run `pnpm rebuild coreav`. With Bun, run `bun pm trust coreav`.
If scripts stay blocked, the launcher still works.

### Build

Native and WASI builds use the compiler version and commit in `scripts/rust-dist.env`, with `rust-src` and the target standard library. `scripts/notices/main.mjs` generates notices for the same Cargo package, target, profile and features as the artifact. The normal dependency graph excludes procedural macros; the target standard-library closure is a conservative superset of retained code. Every platform package and the main WASI package ships `THIRD-PARTY-NOTICES.txt` (`docs/release/notices.md`). This changes distribution notices, not licences.

- `packages/cli/scripts/build.sh`: `cargo build --locked -p coreav-cli
  --no-default-features --features cli --profile edge --target wasm32-wasip1`, then `wasm-opt -O3` when binaryen
  is on `PATH` or named by `WASM_OPT` (CI uses binaryen version_133).
- `packages/cli/scripts/platform.sh PLATFORM`: `cargo build --locked -p
  coreav-cli --profile dist --target T --features release-default` (the
  default features on Windows), with dav1d, libvpx and libopus from
  `scripts/codecs.sh` and a check that the binary loads none of them at
  run time, and the package around the binary, with `NOTICE`
  and target-specific Rust notices, plus `licenses/` when the platform links the C libraries (`docs/release/notices.md`)
  (`scripts/platform-package.js` writes its `package.json` from the main
  package's, so names, version and metadata cannot drift).
- The module, the binaries and the license files are build outputs, not
  committed. Native CI regenerates notices and checks artifact evidence before packing. The main WASI package's `prepack` hook also verifies its matching build-owned notice record using Node only, without rewriting files or requiring Rust. Generated native platform packages have no `prepack` hook, so a direct `npm pack` there does not run that record check. The record stays out of the packages.
- CI builds the WASI module and the Linux x86-64 package on every ready
  pull request. Tags, manual and weekly runs, and pull requests labelled
  `binaries` also build the four other platforms, each built, run and
  smoke-tested on its own platform, as the Python wheels are built. The
  static dav1d, libvpx and libopus are cached per target until
  `codecs.sh` changes.

## Verification

- `packages/cli/test/cli.test.mjs`: the WASI build against the native
  command, for every fixture of `scripts/gen-fixtures.sh` and
  `scripts/gen-fixtures-mkv.sh`. It runs every subcommand: probe,
  `ffprobe`, `--scan-frame-sizes`, chapters, extract-audio, clip
  to a file and to stdout, extract-subtitles, remux, sanitize, split, and
  check with each example policy. Each runs with absolute paths, with the
  input relative to the working directory, and with outputs relative to it.
  Stdout, stderr, the exit code and every output file's bytes must equal
  the native command's.
  - Every case must succeed on at least one fixture, so that a case cannot
    pass by failing the same way twice.
  - No temporary file may be left behind.
  - A 2.6 MB remux to stdout is read only after a second, so that the
    module's writes meet a full pipe.
- `packages/cli/test/native.test.mjs`:
  - the platform list, the optional packages and the versions agree;
  - the Node launcher passes arguments, streams and exit codes through to
    the binary, on fixtures, and falls back to the WASI build when the
    binary cannot start;
  - the install script links `bin/coreav` to the platform binary without a
    second copy, survives relocation and reinstall, and leaves the
    launcher for another version's binary or for none.
- CI (`.github/workflows/npm.yml`) builds, runs the tests, and dry-runs
  `npm publish` on every tarball. `test/smoke.sh` then installs the packed
  tarballs into new projects and runs them through `npx`: Linux x86-64
  with Node 20, 22 and 24, macOS arm64 and Windows with 20 and 24, and
  macOS x86-64 with 24. The Linux arm64 binary runs its checks in its
  build job on an arm64 runner, without the npm smoke test:
  - once with the platform's package, where on Linux and macOS the
    installed command must be the binary itself;
  - once without optional packages, which runs the WASI build.

  Each run checks relative and absolute paths in the platform's form, also
  as a `--policy=` value, output bytes against the native command's, and
  the error object and exit code of a missing file.

## Installed-artifact rehearsal

The optional `package-rehearsal.yml` workflow also installs the native and WASI tarballs on Linux x86-64, Linux arm64, macOS arm64 and Windows x86-64. It checks `--version`, `ffprobe` and `ffmpeg` stream copy on a committed fixture, and loads the installed WASM library in Node for a probe call. It consumes existing build artifacts without publishing; dispatch inputs, artifact expiry and retained failure reports are described in [Installed-artifact rehearsal](release.md#installed-artifact-rehearsal).

## Speed

### Where the time goes

Measured during development on this repository's development host (8
vCPU, Node 24), as the median of `--version` runs:

| Way to start the command | Time |
|---|---:|
| The installed native binary (`coreav` after install) | about 0.7 ms |
| The Node launcher, then the binary (Windows, installs without scripts) | about 28 ms |
| The WASI build under Node (platforms without a package) | about 54 ms |
| `npx coreav`, the package installed | about 220 ms |

Node's own start-up (`node -e 0`) is about 25 ms. On a 242 MiB development
file, `coreav remux` took 0.47 times `ffmpeg`'s time through the installed
native binary, and probe 0.04 times `ffprobe`'s.

### Round 1: the WASI build as the command

Its bars, as fixed before its held-out run:

- **Inputs.** Corpus v1's held-out `real` files with extension mp4, mov, m4v,
  m4a, m4b, 3gp, mkv or webm, at most 128 MiB (`bench/npx/inputs.py`):
  302 files. None was used in development.
- **Operations and references.** Each operation is timed against the FFmpeg
  command line doing the same job on the same file:

  | Operation | `coreav` | FFmpeg reference |
  |---|---|---|
  | probe | `probe --ffprobe-compat F` | `ffprobe -v error -of json -show_format -show_streams F` |
  | check | `check --policy web-upload.json F` | the same `ffprobe` command (as policy-check's B6) |
  | chapters | `chapters F` | `ffprobe -v error -of json -show_chapters F` |
  | extract-audio | `extract-audio F O` | `ffmpeg -i F -map 0:a:0 -c copy -f mp4 O` |
  | clip head, clip middle | `clip --start S --end S+10 F O`, S = 0 and the middle of the duration | `ffmpeg -ss S -i F -t 10 -map 0:v:0 -map 0:a:0? -c copy -f mp4 O` |
  | remux | `remux F O` | `ffmpeg -i F -map 0:N... -c copy -movflags +faststart O`, the streams `coreav` kept (as the remux spike) |
  | sanitize | `sanitize F O` | the remux command with `-map_metadata -1 -map_chapters -1` |
  | split | `split --every 10 F D/%03d.mp4` | `ffmpeg -i F -map 0:v:0 -map 0:a:0? -c copy -f segment -segment_time 10 -reset_timestamps 1 D/%03d.mp4` (as `docs/design/split.md`, with the streams `coreav split` keeps by default) |
  | extract-subtitles | `extract-subtitles F O` | `ffmpeg -i F -map 0:s:0 -f webvtt O` |

  A case counts where both the native `coreav` and FFmpeg succeed; the rest
  are counted apart, by reason.
- **Timing.** Process wall time, sources in the page cache, outputs on the
  local disk. Per case, 5 rounds; each round runs the three commands (the
  launcher, the native `coreav`, FFmpeg) in a rotating order; each command's
  time is the median of its 5. The launcher is run as `node
  packages/cli/bin/coreav.js`, which is what the installed `coreav` command
  runs. (That file is now `lib/wasi.mjs`, the WASI fallback.)
- **Bar.** For each operation, the median over files of the
  per-file ratio launcher / FFmpeg is at most 1.00, overall and in every
  class with at least 10 files. Classes: ISO base media and Matroska/WebM
  files; files under 16 MiB and files of 16 MiB or more. A class with fewer
  than 10 files is listed but not judged. Saying that `npx coreav` is faster
  than FFmpeg for an operation needs the bootstrap 95% upper bound below
  1.00.
- **Correctness bar.** On every case, the launcher's stdout, exit code and
  output bytes equal the native command's; no trap.
- **Reported, not judged.** Native `coreav` / FFmpeg; launcher / native;
  `npm exec` overhead (the median of 20 runs of `npx coreav --version`
  minus the installed command's), and the ratio with it added, which is
  what typing `npx coreav` costs on each call.
- **Statistics.** Medians over files, bootstrap 95% intervals
  over files, 10,000 resamples, seed 20261018.
- **Counting** (`bench/npx/summarize.py`, committed before the run). A
  case with a failed or timed-out round of any of the three commands is
  counted apart, as are files the native command refuses, files FFmpeg
  fails on and middle clips of files without a duration. An operation with
  fewer than 10 timed files is listed but not judged, as a class is.

Results (`docs/measurements/npx-coreav.md`, round 1): correctness passes
and every speed bar misses.
- 3,016 cases compared, all equal to the native command's, with no trap.
- The installed command took 3.9 to 9.9 times FFmpeg's time per call, and
  `npx coreav` 14 to 39 times. The native command took 0.13 to 0.21 times
  FFmpeg's time on the same files.
- The cost is mostly fixed per call: `--version` alone took 60 ms through
  the installed command. For the operations that copy bytes, it shrinks
  relative to FFmpeg as files grow: remux of files of 16 MiB or more took
  1.72 times FFmpeg's time.

### Round 2: the native path

The bars of round 1, unchanged, for the command as the package now installs
it, fixed here before round 2's run:

- **What is timed.** The installed `coreav` command: the main package and
  the Linux x86-64 package, packed and installed with npm into a new
  project, so that `coreav` is the native binary
  (`bench/npx/run.py --path native`). It runs where the round-1 launcher
  ran, in the same rounds with the native workspace build and FFmpeg.
- **Inputs, operations, references, timing, classes, statistics and
  counting.** As round 1, on the same 302 held-out files: round 1 measured
  another build of the command, and no development choice used these
  files.
- **Bar.** For each operation, the median over files of the per-file ratio
  installed command / FFmpeg is at most 1.00, overall and in every class
  with at least 10 files. "Faster than FFmpeg" needs the bootstrap 95%
  upper bound below 1.00, overall and per class.
- **Correctness bar.** On every case, the installed command's stdout, exit
  code and output bytes equal the native workspace build's; no trap.
- **Reported, not judged.**
  - The installed command against the native workspace build: static musl
    and `dist` against glibc and `edge`.
  - The fixed cost of each way to start the command, measured as the median
    of 20 runs of `--version`: the installed native binary, the Node
    launcher (Windows, installs without scripts), the WASI build (platforms
    without a package) and `npx coreav`.
  - `npm exec`'s own cost: `npx` minus the installed command. It is npm's,
    and the same for any package.

Results (`docs/measurements/npx-coreav.md`, round 2): correctness passes
and every bar passes.
- 3,016 cases are equal to the native build's, with no trap.
- The installed command takes 0.09 to 0.20 times FFmpeg's time per
  operation, and it is faster than FFmpeg, with the upper bound of the
  confidence interval below 1.00, for every judged
  operation but the check. Subtitle extraction,
  with one file, is not judged.
- The check is faster overall but on par for Matroska and WebM: median
  0.80, upper bound 1.62.
- `npx` adds 219 ms of npm's start-up per call.
- The launch costs: native 0.7 ms, Node launcher 28.6 ms, WASI 61.2 ms.

The CI regression guard (`ci.yml`'s `bench-guard` job,
`docs/design/benchmarks.md`) counts the instructions of the `coreav`
command, the same source as the native packages' binaries; the npm
packages add no code to that path.

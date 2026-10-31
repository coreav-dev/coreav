# Release archives of the `coreav` command

Status: Implemented (`.github/workflows/release.yml`)

How a version of the `coreav` command becomes downloadable archives on GitHub Releases, and how a user checks one. The npm package (`docs/design/npm-cli.md`) installs the same binaries and has its own workflow.

## What runs when

| Event | What the workflow does |
|---|---|
| A pushed tag `vX.Y.Z` | Builds every target, checks that the tag names the version the command prints, writes `SHA256SUMS`, attests the files' build provenance, and attaches them to a **draft** release of the tag. |
| A manual run (`workflow_dispatch`) | The dry run: the same builds and checks, the files uploaded as the `release-dist` workflow artifact only. No release, no attestation, no tag. |
| A pull request that changes the workflow or the build scripts it runs | The dry run. GitHub starts a manual run only of a workflow that is on the default branch, so this is how a change is tried before it merges. |

Publishing the draft, and pushing the tag before it, are the maintainer's steps. A tag whose version differs from the workspace's (`coreav --version`) fails before anything is attached. A rerun on the same tag replaces the files of its draft and refuses to touch a release that is already published.

## Targets

| Target | Built and checked on |
|---|---|
| `x86_64-unknown-linux-musl` | Linux x86-64 (`ubuntu-24.04`) |
| `aarch64-unknown-linux-musl` | Linux arm64 (`ubuntu-24.04-arm`) |
| `aarch64-apple-darwin` | macOS arm64 |
| `x86_64-apple-darwin` | macOS x86-64 |
| `x86_64-pc-windows-msvc` | Windows x86-64 |

These are the five platforms of the npm packages, built by the same script (`packages/cli/scripts/platform.sh`): the `dist` profile, stripped; Linux static against musl, with zig as the C compiler of `ring` and musl-gcc as that of dav1d and libvpx; Windows with the C runtime linked statically. One binary per platform and build therefore has one definition, and its reasons are in `npm-cli.md`, "Native binaries".

- **Linux is musl only.** A static musl binary runs on glibc and musl systems alike, and has no oldest-glibc floor. A glibc build would add an archive that runs on fewer systems; the npm round 2 measured the installed musl command at 0.09 to 0.20 times FFmpeg's time.
- **Linux arm64 is built on arm64.** GitHub's arm64 runner is free on a public repository. On it, the C libraries need no cross-compiler, and the checks run the binary and the `x264` program natively; before the C decoders, the binary was cross-built on x86-64 and checked under QEMU.
- **macOS x86-64 is kept.** Intel Macs are still in use, the npm package already ships the binary, and the hosted Intel runner builds and runs it natively at no cost. When that runner image is retired, the target can be cross-built on arm64 and lose only its run check.
- **Not built:** Windows arm64 and other architectures, which the npm packages do not cover either.

## Two builds

The release makes two builds of every target (maintainer decision, 2026-10-31). The **default build** is what the archives `coreav-VERSION-TARGET` and the npm packages carry. The **full build** is a second set of archives, `coreav-full-VERSION-TARGET`, that adds the decoders of codecs on the patent list (`scripts/patent_free_check.py`).

| Build | Cargo feature of `coreav-cli` | Features it turns on | Adds to the default features |
|---|---|---|---|
| Default | `release-default` | `cli`, `net`, `dav1d`, `libvpx` | `coreav transcode` and `coreav thumbnail`, with AV1 decoded by dav1d (BSD-2) and VP8 and VP9 by libvpx (BSD-3) |
| Full | `release-full` | `release-default`, `native-h264`, `aac` | CoreAV's own H.264 decoder for both commands, and AAC LC in `coreav decode-audio` |

- **One definition.** The two features are where a build's feature list is written; `platform.sh` takes the build's name (`default` or `full`) and passes its feature. `patent_free_check.py` checks `release-default` like the other builds a package script makes, so the required `deny` check fails if the default build reaches a crate on the patent list.
- **No FFmpeg.** Neither build has `ffmpeg-fallback`. HEVC, ProRes and the other codecs stay with the fallback to the `ffmpeg` program, as in the default features. libvpx is built with its VP8 decoder too, which `coreav-backend-libvpx` uses for VP8.
- **Encoders.** No encoder is linked. H.264 is encoded by an `x264` or `ffmpeg` program the operator installs (policy record 0042, point 5), in both builds, and AAC by the operator's `ffmpeg -c:a aac` in the same way. Opus encoding with libopus (the `libopus` feature) joins `release-default` when it lands: libopus is BSD-3, off the patent list, and builds as a static library on every target, as dav1d and libvpx do.
- **Windows.** `coreav transcode` cannot run on Windows: its encoder starts through the sandbox launcher, which sets its resource limits with Unix calls and starts nothing elsewhere (`coreav_worker::exec`), and the decode worker runs there without limits. The Windows default build is therefore the default features, and the Windows full build adds `aac` alone (`platform.sh`). Windows gets the decoders when the worker has a Windows sandbox, which needs its own policy record.

### dav1d and libvpx in each target

Both libraries are built from source as static libraries and linked into the `coreav` binary, so each archive still holds one self-contained program. They are not taken from the build machine's packages, which are shared libraries a user would have to install, and they are not built by the crates: `dav1d-sys` can clone dav1d at build time, pinned by no hash.

- `packages/cli/scripts/codecs.sh TARGET PREFIX` downloads each library's release tarball, checks its SHA-256 against the pinned value, builds it static into `PREFIX`, and copies its licence files to `PREFIX/licenses`. Pinned: dav1d 1.5.3 (the binding needs 1.3 or later) and libvpx 1.14.1 (the decoder ABI that `coreav-backend-libvpx` declares), each with its decoders only. `platform.sh` runs it into `COREAV_CODECS` unless that holds them already, so the two builds of a job share one.
- The crates find them as they do for system libraries, through `pkg-config` with `PKG_CONFIG_PATH=PREFIX/lib/pkgconfig`. `PREFIX` holds the static libraries only, so the linker takes them. On the musl targets the pkg-config files drop `-lm` and `-lpthread`: musl's `libc.a`, which Rust links, holds both, and the flags would find the build machine's glibc copies.
- **Linux:** compiled natively with `musl-gcc` (Ubuntu's `musl-tools`), with nasm for the x86-64 assembly. zig, the C compiler of `ring`, is not used for them: meson cannot identify its linker.
- **macOS:** compiled with the runner's clang, each architecture on its own, at the deployment targets Rust builds for (11.0 on arm64, 10.12 on x86-64).
- **Windows:** nothing is built (above).

**Sandbox (policy record 0001, point 5).** Static linking does not move dav1d or libvpx into the main process. The CLI decodes AV1, VP8 and VP9 only in the sandboxed worker, which is the `coreav` binary run again with a hidden argument. The libraries' code is in the binary and runs in the worker alone, as it did with shared libraries. On macOS the memory budget stays advisory, so strict-safe mode starts no worker there (`decode.md`, "Memory").

### npm

The five native packages (`@coreav/cli-PLATFORM`) carry the default build, with `NOTICE` and the `licenses` directory. Their names are unchanged, and no full build goes to npm.

The WebAssembly fallback stays the `cli` feature alone and carries no decoder. WASI (preview 1) cannot start a process, so neither the sandboxed worker nor the encoder program can run, and dav1d inside the WebAssembly process would be the in-process mode that policy record 0001 makes opt-in. Where the launcher falls back to it, `coreav transcode` and `coreav thumbnail` are absent, as in a source build without a decoder feature.

## The archives

| Build | Archive | Directory inside |
|---|---|---|
| Default | `coreav-VERSION-TARGET.tar.gz` (`.zip` for Windows) | `coreav-VERSION-TARGET` |
| Full | `coreav-full-VERSION-TARGET.tar.gz` (`.zip` for Windows) | `coreav-full-VERSION-TARGET` |

The directory holds `coreav` (`coreav.exe`), `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE`, and, in a build that links dav1d and libvpx, `licenses/` with their licence files as the pinned tarballs carry them (`docs/release/notices.md`). `NOTICE` is in every archive, whatever the features: its statements are each conditional on what a build includes, so it is never wrong to ship, and an archive cannot lose it when the features change.

`SHA256SUMS` lists the SHA-256 of all ten archives, in `sha256sum`'s format. `packages/cli/scripts/release_assets.py` fails the run when the release's files are anything other than those ten archives and `SHA256SUMS`, or when `SHA256SUMS` lists other files: once after the checksums are written, and again in the draft job before anything is attested or attached. The smoke test's clips travel as the `smoke-clips` artifact, outside the `release-TARGET` artifacts that the checksum job gathers. Each archive and `SHA256SUMS` carry a build-provenance attestation (Sigstore, through GitHub's attestation store), which ties the file to this workflow, the tag and the commit:

    sha256sum -c SHA256SUMS --ignore-missing
    gh attestation verify coreav-0.1.0-x86_64-unknown-linux-musl.tar.gz --repo coreav-dev/coreav

## Checks before anything is attached

On each target, for both builds:

- Under `COREAV_STRICT_SAFE=1`, so no FFmpeg is installed or run: `coreav --version` prints the tag's version (a tag run); `coreav ffprobe -v error -of json -show_format -show_streams` on a committed file (`crates/coreav-audio/fuzz/regressions/decode/edit-rounding.mp4`) succeeds, and the ten binaries' outputs are identical; `coreav ffprobe -show_data -show_packets`, a command that is not native, exits 4, and so does the same command run as `ffprobe` (a copy of the binary under that name).
- The decoders, on clips that `packages/cli/test/release-clips.sh` generates once with the pinned FFmpeg (one second of a test pattern as AV1, VP8 and VP9 in WebM and H.264, and a sine tone as AAC), checked by `packages/cli/test/release-smoke.py`. Both builds transcode the AV1, VP8 and VP9 clips, through dav1d and libvpx, to H.264 with an `x264` program built from its pinned commit (`scripts/install-x264.sh`); the full build also transcodes the H.264 clip with CoreAV's decoder, which the default build refuses. Each output is probed: one H.264 stream of the clip's size and frame count. The full build decodes the AAC clip with `decode-audio`, which the default build refuses. On Linux these run in strict-safe mode too; macOS has no enforceable memory limit, so strict-safe mode would start no sandboxed decoder there, and they run without it. On Windows only the AAC check applies.

## Supply chain

- Every action is pinned to a full commit sha (`scripts/workflow_check.py`); no third-party release action: the draft is made with the `gh` command and the built-in token.
- Permissions are per job. The workflow grants nothing; the build and checksum jobs read the repository; only the draft job, which runs on a tag alone, gets `contents: write`, and `id-token: write` and `attestations: write` for the attestation. No secret.
- No build cache: a release builds from `Cargo.lock` (`--locked`) with nothing restored that another workflow saved. The checkout keeps no credentials.

Rejected:

- **cargo-dist**, which generates a release workflow from Cargo metadata: it would be a second definition of the binaries next to `platform.sh` (musl through zig, `crt-static`, the `dist` profile), its generated workflow installs its own tool at run time, and its installers and package-manager formulas are outside this release. Its layout is taken where it fits: build jobs per target, one job that gathers the files and writes the checksums, and the release made last, from those files.
- **One `.sha256` file per archive** (ripgrep, cargo-dist): one `SHA256SUMS` checks any set of downloads with one command.
- **Creating the release first and uploading from each build** (ripgrep): a target that fails would leave a release with some archives.

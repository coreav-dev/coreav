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

| Archive | Built on | Checked on |
|---|---|---|
| `x86_64-unknown-linux-musl` | Linux x86-64 | the build machine |
| `aarch64-unknown-linux-musl` | Linux x86-64, cross-built | QEMU |
| `aarch64-apple-darwin` | macOS arm64 | the build machine |
| `x86_64-apple-darwin` | macOS x86-64 | the build machine |
| `x86_64-pc-windows-msvc` | Windows x86-64 | the build machine |

These are the five platforms of the npm packages, built by the same script (`packages/cli/scripts/platform.sh`): the `dist` profile, stripped; Linux static against musl, with zig as the C compiler of `ring`; Windows with the C runtime linked statically. One binary per platform therefore has one definition, and its reasons are in `npm-cli.md`, "Native binaries".

- **Linux is musl only.** A static musl binary runs on glibc and musl systems alike, and has no oldest-glibc floor. A glibc build would add an archive that runs on fewer systems; the npm round 2 measured the installed musl command at 0.09 to 0.20 times FFmpeg's time.
- **macOS x86-64 is kept.** Intel Macs are still in use, the npm package already ships the binary, and the hosted Intel runner builds and runs it natively at no cost. When that runner image is retired, the target can be cross-built on arm64 and lose only its run check.
- **Not built:** Windows arm64 and other architectures, which the npm packages do not cover either.

## Features

The default features, `cli` and `net`. The 0.1.0 command set (`compat-cli.md`) is `coreav ffprobe` and `coreav ffmpeg -c copy`, and the fallback to FFmpeg, on by default, is part of every build: it is not a Cargo feature. The `ffmpeg-fallback`, `native-h264`, `dav1d` and `libvpx` features add decoding (`coreav thumbnail`, `coreav transcode`), which is outside the 0.1.0 command set and would link system libraries a static archive does not carry.

## The archive

`coreav-VERSION-TARGET.tar.gz` (`.zip` for Windows) holds one directory of the same name with `coreav` (`coreav.exe`), `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE`. `NOTICE` is in every archive, whatever the features (`docs/release/notices.md`): its statements are each conditional on what a build includes, so it is never wrong to ship, and an archive cannot lose it when the features change.

`SHA256SUMS` lists the SHA-256 of every archive, in `sha256sum`'s format. Each archive and `SHA256SUMS` carry a build-provenance attestation (Sigstore, through GitHub's attestation store), which ties the file to this workflow, the tag and the commit:

    sha256sum -c SHA256SUMS --ignore-missing
    gh attestation verify coreav-0.1.0-x86_64-unknown-linux-musl.tar.gz --repo coreav-dev/coreav

## Checks before anything is attached

On each target, under `COREAV_STRICT_SAFE=1`, so no FFmpeg is installed or run:

- `coreav --version` prints the tag's version (a tag run).
- `coreav ffprobe -v error -of json -show_format -show_streams` on a committed file (`crates/coreav-audio/fuzz/regressions/decode/edit-rounding.mp4`) succeeds, and the five targets' outputs are identical.
- `coreav ffprobe -show_frames`, a command that is not native, exits 4, and so does the same command run as `ffprobe` (a copy of the binary under that name).

## Supply chain

- Every action is pinned to a full commit sha (`scripts/workflow_check.py`); no third-party release action: the draft is made with the `gh` command and the built-in token.
- Permissions are per job. The workflow grants nothing; the build and checksum jobs read the repository; only the draft job, which runs on a tag alone, gets `contents: write`, and `id-token: write` and `attestations: write` for the attestation. No secret.
- No build cache: a release builds from `Cargo.lock` (`--locked`) with nothing restored that another workflow saved. The checkout keeps no credentials.

Rejected:

- **cargo-dist**, which generates a release workflow from Cargo metadata: it would be a second definition of the binaries next to `platform.sh` (musl through zig, `crt-static`, the `dist` profile), its generated workflow installs its own tool at run time, and its installers and package-manager formulas are outside this release. Its layout is taken where it fits: build jobs per target, one job that gathers the files and writes the checksums, and the release made last, from those files.
- **One `.sha256` file per archive** (ripgrep, cargo-dist): one `SHA256SUMS` checks any set of downloads with one command.
- **Creating the release first and uploading from each build** (ripgrep): a target that fails would leave a release with some archives.

# Notices in distributions

Every distribution of CoreAV ships its licence files, `LICENSE-MIT` and
`LICENSE-APACHE`. Every distribution of the `coreav` command, or of a
build that links `coreav-cli`, also ships `NOTICE`, unchanged: the
ISO/IEC 8859 tables of `coreav ffprobe` come from the Unicode
Consortium's mapping files, whose Unicode License v3 asks for its notice
in every copy. A distribution whose binary links `coreav-thumbnail` or
`coreav-opus` ships it for those crates too. `coreav-thumbnail`
encodes JPEG with the `jpeg-encoder` crate, licensed "(MIT OR
Apache-2.0) AND IJG", whose IJG licence requires the statement "This
software is based in part on the work of the Independent JPEG Group"
with binary distributions. That licence is allowed for that crate alone
(`deny.toml`, `docs/design/thumbnails.md`, "JPEG encoder").

The Linux x86-64 musl release command links the mimalloc Rust wrapper and C allocator. Their MIT copyright and permission notices are included in `NOTICE`, which is copied unchanged into every native package and release archive. Ordinary source builds and other targets do not link mimalloc.

A distribution that ships dav1d, libvpx or libopus, linked into the
binary or beside it, also ships their licence files. `packages/cli/scripts/codecs.sh`
copies them from the pinned tarballs into `licenses/`, and the release
archives and the npm packages ship that directory:

| File | What it is |
|---|---|
| `dav1d-COPYING` | dav1d's BSD-2 licence |
| `dav1d-PATENTS` | the Alliance for Open Media Patent License 1.0, which asks to be reproduced with binary distributions |
| `dav1d-x86inc-LICENSE` | the ISC licence of dav1d's x86 assembly layer, the comment that opens that file (it has no licence file of its own) |
| `libvpx-LICENSE` | libvpx's BSD-3 licence |
| `libvpx-PATENTS` | Google's additional IP rights grant for the WebM codecs |
| `libvpx-AUTHORS` | the WebM Project authors, whom the licence's copyright line names |
| `libvpx-x86inc-LICENSE` | the ISC licence of libvpx's x86 assembly layer |
| `opus-COPYING` | libopus's BSD-3 licence, with the royalty-free patent licences it names |

The texts are taken from the tarballs' licence files, never from their
source code, except `dav1d-x86inc-LICENSE`, which the script cuts from
the opening comment of the file it covers.

| Distribution | Links `coreav-thumbnail` | `NOTICE` | `licenses/` |
|---|---|---|---|
| `coreav` command built from source with `thumbnail`, `ffmpeg-fallback`, `dav1d`, `libvpx` or `native-h264` | yes | ship it beside the binary | when it ships a copy of dav1d, libvpx or libopus: that library's files |
| `coreav` command, default features | no | ship it beside the binary | none |
| Release archives, default build (`release-default`; Windows: the default features) | yes, except Windows | in every archive | in every archive but Windows |
| Release archives, full build (`release-full`; Windows: `aac`) | yes, except Windows | in every archive | in every archive but Windows |
| npm platform packages (`@coreav/cli-<platform>`, the default build) | yes, except Windows | in every package | in every package but Windows |
| Python wheel (`crates/coreav-py`) | with its `thumbnail` feature | in the wheel's licence files (`pyproject.toml`, `license-files`) | none: the wheel links no C decoder |
| npm `coreav` (WebAssembly) | no | in the package (`packages/cli/scripts/build.sh` copies it; `files` and the `prepack` guard name it) | none |
| A build that links `coreav-opus` (none does yet) | n/a | ship it: the crate's tables come from the code of RFC 6716 Appendix A, whose BSD licence asks binary distributions to reproduce its notice | n/a |
| Crates (`publish = false` today) | `coreav-thumbnail`, `coreav-cli` | when `coreav-cli`, or a crate that links it or `coreav-thumbnail`, is published, `NOTICE` goes in its package | n/a |

`NOTICE` also names dav1d, libvpx and libopus, their licences and patent grants,
and the files above, and says which builds link them. It also carries the
notice and licence of the code of RFC 6716 Appendix A, from which
`scripts/opus_tables.py` generates the Opus decoder's tables; the
generated files name it in their header. And it says that the full builds
(`native-h264`, `aac`) decode H.264 and AAC, which may need a patent
licence where they are used.

A release checks this table against the features it builds with. The
release workflow (`.github/workflows/release.yml`,
`docs/design/release.md`) copies `NOTICE` beside `LICENSE-MIT` and
`LICENSE-APACHE` into every archive, whatever the features: each statement
in it says which builds it applies to, so it is correct for every build.
`packages/cli/scripts/platform.sh` puts the same files, and `licenses/`
where the build links dav1d, libvpx and libopus, beside every native binary, so the
npm platform packages and the archives ship the same set. A new dependency
whose licence asks for a notice adds its statement to `NOTICE` and a row
here; a new C library adds its files to `codecs.sh` and the table above.
`packages/cli/scripts/notices_check.py` checks the packed artefacts
themselves: the npm tarballs (`.github/workflows/npm.yml`), the wheel
(`.github/workflows/python.yml`) and the release archives
(`.github/workflows/release.yml`) each must hold `LICENSE-MIT`,
`LICENSE-APACHE` and `NOTICE` as committed, in one directory.

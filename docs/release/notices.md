# Notices in distributions

Every distribution of CoreAV ships its licence files, `LICENSE-MIT` and
`LICENSE-APACHE`. A distribution whose binary links `coreav-thumbnail`
or `coreav-opus` also ships `NOTICE`, unchanged. `coreav-thumbnail`
encodes JPEG with the `jpeg-encoder` crate, licensed "(MIT OR
Apache-2.0) AND IJG", whose IJG licence requires the statement "This
software is based in part on the work of the Independent JPEG Group"
with binary distributions. That licence is allowed for that crate alone
(`deny.toml`, `docs/design/thumbnails.md`, "JPEG encoder").

| Distribution | Links `coreav-thumbnail` | `NOTICE` |
|---|---|---|
| `coreav` command built from source with `thumbnail`, `ffmpeg-fallback`, `dav1d`, `libvpx` or `native-h264` | yes | ship it beside the binary |
| `coreav` command built with `libvpx`, shipped with a copy of libvpx | yes | ship it beside the binary, and libvpx's own `LICENSE` and `PATENTS` files with that copy |
| `coreav` command, default build | no | not required |
| Python wheel (`crates/coreav-py`) | with its `thumbnail` feature | in the wheel's licence files (`pyproject.toml`, `license-files`) |
| npm `coreav` (WebAssembly) | no | not required |
| A build that links `coreav-opus` (none does yet) | n/a | ship it: the crate's tables come from the code of RFC 6716 Appendix A, whose BSD licence asks binary distributions to reproduce its notice |
| Crates (`publish = false` today) | `coreav-thumbnail`, `coreav-cli` | when a crate that links it is published, `NOTICE` goes in its package |

`NOTICE` also says that builds with the `libvpx` feature link libvpx (BSD-3, with Google's additional IP rights grant for the WebM codecs). The command links the system's shared library and ships no part of it; a distribution that ships libvpx with the command ships libvpx's `LICENSE` and `PATENTS` files, as their licence asks of binary distributions.

`NOTICE` also carries the notice and licence of the code of RFC 6716 Appendix A, from which `scripts/opus_tables.py` generates the Opus decoder's tables; the generated files name it in their header.

A release checks this table against the features it builds with. No
workflow builds binary releases yet; the first one that ships a build
with `coreav thumbnail`, or one that links `coreav-opus`, copies `NOTICE` beside `LICENSE-MIT` and
`LICENSE-APACHE`, as `packages/cli/scripts/build.sh` copies those. A new
dependency whose licence asks for a notice adds its statement to `NOTICE`
and a row here.

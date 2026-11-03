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

## Native SVT-AV1 notice checks

Policy record [0043](../design/encoder-policy.md) permits BSD-3-Clause-Clear only for the SVT-AV1 encoder dependency and its dedicated backend. SVT is an external native library, not a Cargo dependency with its own package metadata. `scripts/svt_av1_licenses.py` enforces that scoped exception on a recipe installation; it does not add Clear to `deny.toml` or change the Rust glue's `MIT OR Apache-2.0` licence.

Run `python3 -I scripts/svt_av1_licenses.py --target TARGET --prefix PREFIX` after the pinned recipe build. The prefix must be absolute and unchanged during the check. The checker reads the version, archive pin, targets and notice destinations from the recipe owner. It requires matching manifest identity, regular retained files with matching hashes, and exactly the five expected notice files. Each notice must also match `scripts/svt-av1-notices.json`, which records SHA-256 fingerprints of the complete, unmodified upstream copyright, licence and patent texts. The fingerprints were obtained from only those legal members after verifying the recipe's archive digest. The record's archive digest binds that evidence to its source; it does not select a second source pin. A recipe archive change requires reviewing the fingerprint evidence again, even if the text is unchanged. Do not derive approved fingerprints from an installed prefix or its self-reported manifest.

The retained texts include the primary Clear licence, BSD-2 licence, AOM Patent License 1.0, and the fastfeat and safestringlib licences. The gate emits an identity and notice-hash record without local paths. The existing four-target native CI runs it after every fresh successful build and retains its log. Offline tests use exact upstream legal fixtures; replacing any text and rewriting its manifest hash still fails.

This gate checks consistency and exact notice bytes. Manifest self-hashes do not authenticate arbitrary replacement libraries or prove their source, architecture or linkage. Those build checks belong to the trusted recipe invocation. The gate neither executes installed code nor establishes Rust linkage, complete release-package coverage or encoder acceptance. No SVT encoder is enabled and no distribution is authorized by this check; packaging must still carry these texts when it ships the native library.

## Rust dependency notices

Every native release archive (default and full), npm platform package, main npm WASI package and private JavaScript WASM package also ships `THIRD-PARTY-NOTICES.txt` beside the root licence files. Python wheels keep the notices described above; their Rust dependency notices are not covered here.

`scripts/notices/main.mjs` reads the exact package, target, profile and feature selection used by the build. It includes the locked normal Cargo dependency graph, excluding procedural macros, and the pinned Rust standard library's target closure rooted at `std` and `panic_abort`. The latter is a conservative superset of linker-retained code. The generator preserves upstream copyright and licence texts, distinguishes project-graph and toolchain versions of the same crate, checks vendored files against upstream checksums and refuses missing or unsupported texts. No copyright line is synthesized.

The distribution compiler version and commit live in `scripts/rust-dist.env`, and builds require `rust-src`. Builds remap Rust source paths. Independent evidence reads source identities from the native binary. Missing identities do not establish absent code; the conservative closures still apply. Unsupported Rust symbol forms fail with instructions to update the evidence reader after a toolchain bump.

The build writes a local `THIRD-PARTY-NOTICES.record.json` binding notice and artifact hashes to the selection, pin, manifests, lock, policy and generator inputs. `verify DIRECTORY` checks it with Node filesystem and crypto operations only, so verified pre-generated builds can be packed without a Rust toolchain or raw target directory. The record detects staleness, not forgery or local registry tampering, and is never shipped. CI's `check DIRECTORY` regenerates notices and rechecks artifact evidence; `notices_check.py --third-party` separately checks target and compiler headers in final archives. Its default mode remains compatible with wheels.

The WASI toolchain's `wasip1` archive omits its licence files. A committed row binds its version, manifest repository and published VCS commit to the upstream repository's root licence files. Their Git blob hashes are checked offline before rendering, the notice section names the source commit, and the build record binds those committed inputs. No copyright line is synthesized.

## C runtime notices

The Rust `libc` crate's licence is not the C library's licence. The shared notice owner also carries a target-specific C-runtime inventory in `scripts/notices/upstream/runtime/manifest.json`. The inventory is bound to the distribution compiler commit. An unknown target or a compiler change refuses generation until this inventory is reviewed. It is independent of Cargo features, so default and full builds cannot omit runtime notices by selecting different Rust dependencies.

The inventory was checked against the ten native binaries from a successful main-branch release dry run, a locally built WASI CLI using the pinned compiler and package flags, and the packed browser module from the WASM workflow. ELF dependencies, Mach-O load commands, PE imports and WebAssembly imports distinguish embedded code from system-provided libraries. For stripped Linux binaries, retained code bytes were also matched against the hash-verified Rust target archives' runtime objects. An absent source string is not evidence of absent code.

| Distribution target | Runtime and linkage | Notice treatment |
|---|---|---|
| `x86_64-unknown-linux-musl`, default and full | Static musl libc and startup, LLVM libunwind, compiler runtime support. No ELF shared-library dependency | Preserve musl's MIT `COPYRIGHT`, the retained math routines' SunPro and Arm notices, Arm's memory-routine MIT attributions as a conservative superset, and the complete LLVM libunwind and compiler-rt licence texts. The LLVM files include their exception and legacy NCSA/MIT terms |
| `aarch64-unknown-linux-musl`, default and full | The same static runtime families, including Arm's `memcpy` and `memset` | The same texts. The Arm routines' copyright dates differ from the umbrella `COPYRIGHT`, so both exact opening legal comments are retained |
| `aarch64-apple-darwin`, default and full | Dynamic, system-provided `libSystem` and Apple frameworks; Rust compiler builtins are embedded | Apple system binaries are not copied into the package. Keep the existing Rust/compiler-builtins notices; do not represent an SDK or operating-system licence as a new CoreAV grant |
| `x86_64-apple-darwin`, default and full | The same system-runtime arrangement | The same treatment |
| `x86_64-pc-windows-msvc`, default and full | Static MSVC startup, vcruntime and Universal CRT, with dynamic Windows system DLL imports | The generated `Microsoft C runtime` entry states that statically linked vcruntime and UCRT are distributed under the Visual Studio redistribution terms and links those terms. This is an informational notice, not a bundled EULA or a change to the project's licence policy |
| `wasm32-wasip1`, npm CLI | Static wasi-libc, its musl and cloudlibc portions and C dlmalloc; WASI calls are host imports | Preserve the complete wasi-libc root licence texts, musl `COPYRIGHT`, cloudlibc BSD-2 and musl-fts BSD-3 texts as a conservative runtime superset. The root statement identifies dlmalloc's CC0 dedication, which requires no attribution |
| `wasm32-unknown-unknown`, `@coreav/wasm` | Rust's `dlmalloc` crate and compiler builtins are embedded; imports belong to the JavaScript bindings, not WASI or a platform CRT | Already covered by the pinned toolchain's Rust `dlmalloc` and compiler-builtins sections. No musl, wasi-libc, glibc or platform CRT is added |

The Linux musl libc and start files come from the pinned Rust target, not the build host's glibc. The retained LLVM unwind code is not GNU libgcc; a compiler stamp in an object does not identify that object's licence. No glibc or `libgcc_s` dependency was present. The existing `compiler_builtins` section retains its compiler-rt-derived builtin and libm notices on every target. Its generic Sun summary does not replace the separate SunPro and SunSoft notices in the retained C math routines. The codec libraries and ring keep their existing independent notices.

The runtime manifest records upstream legal-file locations and exact SHA-256 hashes. musl 1.2.5 is the version selected by the pinned compiler's build scripts. The compiler's LLVM submodule is `1b9c0d5ff9bbe7634aead059efe6b11a7eeba145`. Its WASI SDK is 33, whose wasi-libc revision is `161b3195fc2558d2b1ba3eb9ffae3b2b47407623`. These are licence-input identities, not additional build selectors. Only legal texts and opening legal comments are copied.

`scripts/notices/evidence.mjs` requires each target's runtime identities independently of the Cargo graph. Generation checks every retained legal file's hash. `notices_check.py --third-party` independently requires the complete runtime sections in final CLI archives, not just a Rust header or an identifier. The build record already covers the runtime manifest and texts through its recursive upstream-input owner. Browser-module package checks continue to regenerate and compare the shared notices.

This inventory and these consistency checks do not authorize publication or change the licence policy. In particular, Windows distribution must separately comply with the [Visual Studio licence](https://visualstudio.microsoft.com/license-terms/) and [Microsoft CRT redistribution requirements](https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files). The macOS system libraries remain governed by Apple's platform terms. Python-wheel Rust notices and a future audit of compiler in-tree file exceptions are outside this inventory.

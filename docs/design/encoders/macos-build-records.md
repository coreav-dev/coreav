# VideoToolbox platform build records

## Scope and order

This plan is the first commit of the platform-build receipt change. The [registered plan](../../../scripts/encoders/macos/videotoolbox-plan.md) requires reviewed executable records before the observed physical-host seal. Both records describe actual builds of merged candidate `7abf723411f9dacb6e8f5dca148fa5ea03b62932`, whose native content matches the amended source freeze from coreav-dev/coreav#355.

Commit the existing, unmodified capture records as `macos-builds/macos.json` and `macos-builds/linux.json`. The existing `coreav_build.py` consumer owns their schema. The macOS artifact is the standalone exporter; the Linux artifact supplies independent native decoding. Neither executable is distributed by this change.

| Platform | Record SHA-256 | Executable SHA-256 |
|---|---|---|
| macOS arm64 | `453c232e4c9eeaa0002630922323d3d51e420fefea7acdef5eae57a92abebfec` | `4385e1f90f16d46052f8778094cd58a1fa2ae6261b029230e4703602f7e029d9` |
| Linux x86-64 | `6bfd0e7e51dda682fed46e315debc63cef9ec9d5fa9421f3436c9050dd9de5b8` | `31ee4692fed366e40395c1ad33bd82cad5edca3c994c98afd4ab768e44ccc0c5` |

Both captures used the pinned Rust 1.99.1 toolchain, isolated archived candidate bytes, an offline locked dist build and separate target directories. Registry packages were prepared from archives verified against Cargo.lock, with per-file checksum inventories. No Cargo configuration override or encoder execution was used. Build hosts are not measurement hosts; these records establish neither physical-host identity nor performance.

## Verification and later binding

Require both records to pass the unchanged `coreav_build.validate` for the exact candidate and platform. Recompute their record and executable hashes, candidate tree identity, pinned toolchain, dependency lock entries and every recorded configuration input from Git. Require all frozen native and tool hashes to remain unchanged. Run the existing build-record synthetic regressions; do not add a validator exception.

After this receipt change merges, the later seal must reference the actual merged receipt revision in both `coreav_builds` descriptors. Do not put a pre-squash head or a guessed merge identifier there. The candidate remains the exact revision above. Reviewed records are the provenance trust boundary, not cryptographic source-to-binary attestation.

The macOS baseline identity record, cache observer, raw-reference locations, exact allocation and OS observations remain separate registration requirements. This receipt change changes no source identity, frozen tool/native byte, command, threshold or runtime gate. No rental or judged work is authorized by its merge.

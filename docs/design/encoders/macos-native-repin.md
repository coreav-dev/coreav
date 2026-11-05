# VideoToolbox native-content amendment

## Scope

This plan precedes the native-content amendment to the source freeze from coreav-dev/coreav#342. The original freeze file merged unchanged, but concurrent main changes mean six native-file hashes no longer describe the merged candidate. The strict seal must reject that mismatch rather than accept the unmerged review head.

The sole candidate for this amendment is merged main commit `7abf723411f9dacb6e8f5dca148fa5ea03b62932`. Replace only the following six `native_inputs` values in `scripts/encoders/macos/videotoolbox-sources.json`. All other registration fields, 31 tool hashes, 123 remaining native hashes, native path membership, thirty source/raw/frame identities, selection, uncovered-2160 reconciliation, commands and numerical bars remain byte-for-byte unchanged. The existing seal validator is unchanged.

| Native file | Previous SHA-256 | Replacement SHA-256 |
|---|---|---|
| `Cargo.toml` | `053dbbcc0cfd3dd3cb15d2a6c94e38ec89d0e5be1ac4041c66658784767950b5` | `460b5c8426f343b202d50b066028180e7c3389f66d2ad2783b0cf1eee3c951f0` |
| `crates/coreav-h264/src/decoder.rs` | `50952afdae9c56da5e1d4989200348d52c45d9ddc2b8be72561ba4f300e9da1f` | `dbd879125d7b83761cf716239c282bd79ede4a035f58030f3f10c50ac83d1be6` |
| `crates/coreav-h264/src/inter.rs` | `afdac877b270570635bc87438de4c4e2bf3b048e03747874ba33bc6a7e503cdb` | `f8e09004fdb6c9fe8ca3537ef93b27277105455d6fe4b511f4df651f8de85f9b` |
| `crates/coreav-h264/src/lib.rs` | `3b6a79b59128b3bc26cbbe6fb9041047bfa96d3fccb922cf17baeaf198737d29` | `505c21d6a93833c3a9c59485ad63ad95952c375cd9ae34b22c8ed3a20e957f37` |
| `crates/coreav-h264/src/params.rs` | `805eb37c03378e1bab468c2b9d8d82466a2ee024ebb591cbab313a09c6a1c910` | `1712f44b0eb1e09347ac316b0f9dbc8177b5012d8b041034d95a75e810ee91b7` |
| `crates/coreav-h264/src/picture.rs` | `e9d56f86baf90f6d2b3eaf68ffcb055a59c12eb87ac6260bc7000901e099c713` | `55997da5848baa33940d4cf54d20258a9649aa80fe7f42a21793a3552f22d332` |

## Verification and stop conditions

Before publishing, require the original native map to fail against that exact candidate on exactly these six files and the amended map to match every native file with identical path coverage. Verify that removing `native_inputs` from both JSON objects makes them equal and that the textual diff contains only the six replacements. Recheck every frozen tool hash and run the existing source-freeze synthetic regressions. This is native-binding verification, not a complete build/host seal or a conformance result.

After this amendment merges, bind the actual reviewed amendment revision and amended freeze digest in the separate actual-build/physical-host seal. Keep the fixed candidate above; a later main commit is not an implicit replacement. Before rental, compare the pinned native and tool bytes against then-current main. If any pinned byte or path membership changes, stop and report the drift rather than re-pin again or weaken the validator.

No rental, input materialization, media acquisition, encoder invocation, quality calculation or timing run is authorized by this amendment. The separately reviewed and merged observed build/host seal remains mandatory before judged work.

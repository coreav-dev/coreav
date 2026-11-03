# Build an opt-in H.264 PGO artifact

This recipe produces a Linux x86-64 `decode` example, not a replacement for the installed CoreAV CLI. Default builds and packages do not change. Read the [design and registered evaluation](design/h264-pgo-release.md) before using the result as performance evidence.

## Prepare the compiler and training bytes

Use a clean checkout and a Cargo home without configuration overrides. If `CARGO_HOME` is set, it must be an absolute path. Unset all `CARGO_PROFILE_*`, `CARGO_BUILD_*`, `CARGO_TARGET_*` and `CARGO_INCREMENTAL` variables, compiler wrappers and bootstrap overrides, `RUSTFLAGS`, `RUSTDOCFLAGS`, their encoded Cargo forms, `RUSTC`, `RUSTDOC`, and `LLVM_PROFILE_FILE`. Even empty override variables are refused. The recipe rejects both Cargo configuration filenames in Cargo home and the source directory's complete ancestor chain before every compilation. It accepts no `--config` or other Cargo argument passthrough. Only the tracked manifests and the recipe's recorded command and flags choose the build profile.

```sh
rustup toolchain install 1.99.1 --profile minimal --component llvm-tools-preview
```

Obtain the approved training elementary streams through the existing training extraction procedure. `scripts/h264_pgo_build.py` selects the same sixteen profile-only inputs as the diagnostic, by their recorded elementary-stream hashes. It checks all sixteen hashes before compilation. Do not put held-out media in this directory. Raw media is not distributed with this repository.

Run profile collection on an approved untimed build host. Keep its output outside the checkout:

```sh
python3 -I scripts/h264_pgo_build.py collect \
  --source "$PWD" --media /data/training-es --out /build/pgo-clean
```

The command builds unprofiled and instrumented artifacts with explicit `--target x86_64-unknown-linux-gnu`, qualifies instrumented output, collects only decoder-generated profiles at one and eight threads, and builds the optimized artifact. Host build-script profiles and raw-output preflight profiles have separate directories and never enter the merge. The merged function listing must not contain `build_script_build`.

The command also rebuilds the optimized artifact in another empty target directory and requires byte equality. It checks optimized output against the unprofiled output for every training input at both thread counts. No timing comparison occurs in this command.

It also builds diagnostic-only CLI companions with default features plus `native-h264`: an unprofiled CLI, a CLI using the same retained decoder profile, and an independently rebuilt PGO CLI that must match byte for byte. These are not `release-full` packages, and the diagnostics never train the profile.

On success, `/build/pgo-clean/build.json` records the commands, compiler and linker identities, source and lockfile identities, training input and output hashes, each raw profile, the merged-profile hash, and the binary hashes. Keep this receipt together with `profile.profdata`, `profiles/`, the binaries, and logs. The optimized example is `/build/pgo-clean/optimized/decode`. A compiler, target, source, profile or flag change creates a new artifact and needs new qualification.

An untimed recipe check at source `50581ac8d546cc88efb97bfa485aea6db9d5fa16` collected exactly 32 decoder profiles, with no `build_script_build` functions. Its merged-profile SHA-256 was `2c4008c9f692c6de69c3839205f9b94d3319ab7781b16a9a56e4a7d229571d49`. The optimized and separate-target-directory rebuild both hashed to `3d4a10c5a452f32b528c95e4661bf236f0b42be1bfe7943d228dd626e45a1cbf`. The recipe's complete training-output checks passed. This checks construction and byte preservation, not a speed claim.

A separate clean build host reproduced that optimized hash using the retained profile. All 96 native and pinned-oracle byte, hash and frame-count checks agreed across sixteen training inputs at both thread counts. [The neutral evidence record](design/evidence/h264-pgo-build.json) retains each call's result. Those training checks use the existing training diagnostic policy for known oracle messages; the later held-out executor permits no stderr exception.

That construction evidence predates the CLI companion addition. It does not verify a companion build or diagnostic fixture. The merged-source build, independent companion rebuild and real diagnostic qualification remain post-merge work.

Profile generation at eight threads can produce different branch counts on another run. Rebuilding with the same retained profile, not regenerating it, is the byte-reproducibility contract. The independent target-directory rebuild is not a claim of cross-platform compiler reproducibility.

## Reproduce the historical diagnostic binary

This is separate from the clean recipe. Use source commit `1821f1f005ff78f294a1b529bde5efe145b21e5c`, Rust 1.99.1, matching LLVM tools 23.1.1, and the retained merged profile with SHA-256 `0d12fc7b98c5f94e3b99122b5bc3797cc0ba0d2b8ae983a54041207ca723fb63`. That profile includes three build-script profiles. Do not clean it and call the result a reproduction.

Use the original source/profile paths and the Debian 13 linker environment recorded with the diagnostic. Paths can affect embedded panic strings, so substituting a new absolute checkout path is not a byte-reproduction method. The historical mode deliberately omits the clean recipe's explicit target and path remapping and retains `-Clink-arg=-Wl,--emit-relocs`:

```sh
python3 -I scripts/h264_pgo_build.py reproduce \
  --source "$HISTORICAL_SOURCE" --profile "$RETAINED_PROFILE" \
  --out /build/historical-check
```

The command writes a comparison receipt and fails unless the entire binary matches SHA-256 `e1a3ce41cf39fc77fbdded85f55eb627169065b8a8be3ae38ff1472e584ba3fa`. It never opens media or measures speed. This relocation-bearing binary is not the clean recipe's output.

Two clean builds in Debian 13 reproduced that exact SHA-256. Both used the retained profile, the same source and Rust version, GCC 14.2.0-19, binutils 2.44-3, and libc development files 2.41-12+deb13u4. The second build removed the first target directory before compiling. The container image was `debian@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4`. These are build checks, not a rerun of the diagnostic or held-out performance evidence.

## Run the registered evaluation after merge

Both the allocation reservation and the plan PR must be merged. Freeze acquisition metadata, its exclusion registry, the complete roster and the clean build receipt before running CoreAV on the allocation. Use the same merged revision for the recipe and executor. Run the registered timing procedure only on the designated on-demand host.

The executor expects a new directory containing `roster.json`, `exclusions.json`, four approved training `references.json`, the provider's `instance.json`, the clean `build.json`, `base`, `pgo`, its `independent` rebuild, `profile.profdata`, the verified `ffmpeg` and `ffprobe`, identical base copies named `aa` and `null`, and `media/<neutral label>.h264`. Stage the companion binaries as `cli-base`, `cli-pgo`, and `cli-independent`. After both merges, prepare the fixed synthetic diagnostics with `python3 -I scripts/h264_pgo_diagnostics.py prepare /evaluation`; preparation verifies the merged reservation and pinned oracle. Keep its complete `diagnostics/` inventory.

Create `reservation-checkout.json` with the allocation repository's clean checkout path under `checkout`, the manifest's relative path under `manifest`, and the reserved-input metadata path under `frozen`. The checkout needs read access to its `origin/main` and must be updated to fetched main before freezing the registry. The commands refuse an unmerged revision or mismatched reservation. `registry` executes the reservation's exact committed exclusion checker and writes `reservation-receipt.json` and `reserved-inputs.json`; allocation and execution verify those results again. Do not substitute hand-written receipts. The checker validates the frozen metadata identity and includes both spent and still-reserved inputs.

Stage `population.json` with the allocation label, the reservation's population label, neutral snapshot names and hashes under `snapshots`, and candidate `identity` and `groups` records under `candidates`. Keep the referenced metadata snapshots in `snapshots/`. Generate `exclusions.json` with the `registry` action from all four required owners' registered inventory roots in `registry-inputs.json`. The reservation's acquisition procedure defines those inventories and the fixed population. Do not replace them with a smaller scan.

The registry also scans the standing reservation manifest and frozen metadata automatically. Their required content hashes cannot be omitted from the exclusion set. Both receipts, the diagnostic fixtures and all three CLI binaries enter the content-bound allocation contract.

Before acquiring media, run `allocate` and record the printed contract digest with the committed listing. This freezes `allocation-contract.json`, the profile, binaries, registry, and population. Acquisition writes `collection.json` as an ordered allocation prefix. Each decision retains `identity`, `source_sha256`, `capture_status`, complete `metadata`, and `status`; an exclusion also carries its checked `reason`. Metadata carries codec, planar format, field order, profile, width, height, and positive presented `frames`. The executor derives the selected roster from these decisions rather than trusting a separately supplied list.

```sh
python3 -I scripts/h264_pgo_round.py registry /evaluation
python3 -I scripts/h264_pgo_round.py allocate /evaluation
# Record the printed digest before acquiring media.
python3 -I scripts/h264_pgo_round.py run /evaluation --allocation-sha256 "$ALLOCATION_SHA256"
python3 -I scripts/h264_pgo_round.py audit /evaluation --allocation-sha256 "$ALLOCATION_SHA256"
```

Exit 0 means both PGO bars passed, 1 means a complete run missed a bar, and 2 means the run is incomplete or invalid. Retain `events.jsonl`, bindings and logs in all cases. A miss does not authorize a rerun. Publish results separately, with neutral labels and no private paths or acquisition details.

Diagnostics run after the H.264 timings and before the final host-health checks. Keep `diagnostics.jsonl` and the summary's separate diagnostic result even when a guard fails. A CLI exit, unexpected stderr or explicit diagnostic-output failure yields an incomplete diagnostic result without changing the H.264 verdict. Timeouts, operating-system errors and contract failures propagate and invalidate the shared run.

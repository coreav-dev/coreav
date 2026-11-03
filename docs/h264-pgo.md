# Build an opt-in H.264 PGO artifact

This recipe produces a Linux x86-64 `decode` example, not a replacement for the installed CoreAV CLI. Default builds and packages do not change. Read the [design and registered evaluation](design/h264-pgo-release.md) before using the result as performance evidence.

## Prepare the compiler and training bytes

Use a clean checkout and a Cargo home without configuration overrides. Unset compiler wrappers, `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET`, `LLVM_PROFILE_FILE`, and target-specific Cargo variables. The script refuses these overrides rather than silently building a different artifact.

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

On success, `/build/pgo-clean/build.json` records the commands, compiler and linker identities, source and lockfile identities, training input and output hashes, each raw profile, the merged-profile hash, and the binary hashes. Keep this receipt together with `profile.profdata`, `profiles/`, the binaries, and logs. The optimized example is `/build/pgo-clean/optimized/decode`. A compiler, target, source, profile or flag change creates a new artifact and needs new qualification.

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

The executor expects a new directory containing `roster.json`, `exclusions.json`, four approved training `references.json`, the provider's `instance.json`, the clean `build.json`, `base`, `pgo`, the verified `ffmpeg` and `ffprobe`, identical base copies named `aa` and `null`, and `media/<neutral label>.h264`.

```sh
python3 -I scripts/h264_pgo_round.py run /evaluation
python3 -I scripts/h264_pgo_round.py audit /evaluation
```

Exit 0 means both PGO bars passed, 1 means a complete run missed a bar, and 2 means the run is incomplete or invalid. Retain `events.jsonl`, bindings and logs in all cases. A miss does not authorize a rerun. Publish results separately, with neutral labels and no private paths or acquisition details.

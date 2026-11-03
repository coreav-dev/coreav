# PNG and BMP from YUV: exact V2 registration

This is a separate registration for future V2 runs that explicitly select `png-bmp-yuv-v1`. The existing V1 and V2 registrations, command rows, input assignment, denominator, miss ordering, bars, earlier records and frozen judging material do not change. This registration and its tools must merge before a judged run uses them. Tests before merge use only newly constructed synthetic controls, not the registered command set or held-out inputs. This document is committed before its implementation and any media experiment for this registration.

## Two exact comparisons

A selected command counts only when both comparisons pass for every output image:

1. The pinned FFmpeg 9 decoder reproduces the actual CoreAV pre-encode RGB picture byte for byte from the PNG or BMP. Encoded file bytes need not equal FFmpeg's encoded file bytes. This checks the lossless encoder separately.
2. That captured RGB picture equals the RGB decoded from the pinned FFmpeg 9 output of the same command, byte for byte. There is no colour-conversion tolerance. A difference is an `oracle` miss even when the lossless encoder comparison passes.

The normal candidate executable and the capture harness must produce identical complete encoded outputs on identical input bytes and options. A failed, incomplete, stale or mismatched capture cannot establish a pass. All existing no-fallback, encoder-start, output-shape, selected-stream, stream-field and rational timing checks still apply. Capture instrumentation changes neither decoding, colour conversion, frame selection nor encoding.

The second comparison includes upstream decoding and frame selection as well as conversion. A miss alone does not identify a converter defect. No pixel alignment, relabelling, rescaling, transfer conversion, quantization, dithering or numeric allowance is applied before comparison. Decoding packed BGR from BMP to RGB is channel reordering, not a YUV conversion.

## Eligible outputs

The new comparison covers one selected eight-bit planar YUV video stream converted to packed eight-bit RGB, written as a PNG or 24-bit BMP file or numbered file sequence. Source formats are `yuv420p`, `yuv422p` and `yuv444p`, including their full-range `yuvj` aliases. Each reference image must decode to exactly one frame of the expected codec, dimensions and eight-bit three-channel pixel format. Output names and sequence indices must match in full.

Unsupported reference shapes, alpha or high-bit-depth colour conversion, overwritten `-update` images and image packet pipes remain `no_oracle` under this registration. They are not silently interpreted as a single image or guessed from output length. Empty output does not satisfy a nonempty RGB capture. RGB-source outputs retain their earlier predicate.

## Capture and provenance

An opt-in, feature-gated CLI-library capture entry runs the same parser, planner and frame writer as the ordinary command. It records the actual picture immediately before PNG/BMP encoding, after the existing conversion and rotation. A dedicated Rust integration-test executable supplies this capture mode without adding a public CLI option or environment-controlled hook to a release executable. Its own worker entry hosts the same sandboxed decoders as the normal executable.

Each invocation uses a fresh capture directory and a new call identity. A complete receipt binds the registration, exact command, source bytes, selected source stream, capture executable and output bytes. Each captured frame records its ordinal, source presentation time, geometry, pixel format and packed RGB bytes with their length and digest. The receipt is published only after a successful complete command. Capture errors fail the harness invocation rather than falling back to an unobserved encode. The checker validates provenance, complete file rosters, lengths and digests before comparing samples. It also requires the harness outputs to equal the ordinary executable's outputs, not merely their probes or receipts.

The run manifest records the candidate and capture executable hashes, their build features, the registration hash, the fixed command-set hash, input hashes and both verified oracle executable identities. The Linux x86-64 oracle stays the frozen FFmpeg 9 pin. Local synthetic tests may use the independently pinned macOS arm64 binaries; they do not establish Linux results.

## Difference records

For every judged image command, retain separate lossless-encoder and reference-RGB comparisons. For equal-sized packed RGB streams, report the maximum absolute error for each of R, G and B, the number of unequal channel samples and the total channel-sample count. Aggregate maxima and counts across every corresponding output image, without averaging away a failure. Also retain per-image comparisons so a codec implementer can locate the mismatch. A size or frame-count mismatch is a failed structural check, not a truncated numeric comparison. Metrics that cannot be computed are explicitly unavailable.

These diagnostics never change acceptance. One differing channel sample fails the corresponding exact comparison. A lossless-encoder failure and a conversion/reference failure remain distinct even when both occur. Existing command miss ordering is unchanged.

## Specification boundary

[ITU-T H.273 (07/2024)](https://www.itu.int/rec/T-REC-H.273-202407-I/en), clause 8.3, defines matrix coefficients and the range scaling and offsets. Clause 8.7 and Table 8 define chroma sample locations. Those fields do not specify one reconstruction kernel that yields a unique RGB byte stream. This registration deliberately requires FFmpeg command compatibility even where multiple reconstruction choices may be valid. It introduces no known-divergence waiver.

The existing image path calls `resize_to_rgb` with `ChromaUpsampling::FfmpegCompatible`, whose unscaled chroma branch uses nearest-sample replication. It does not call `Picture::to_rgb`. No change to that path's arithmetic, matrix/range defaults, siting, filters, encoders or codec implementation is authorized here. Differences are reported for separate codec work.

## Implementation and verification

Add the capture boundary, receipt validator and a separately selected V2 tool entry. Default runs retain the prior registration and YUV PNG/BMP exclusion. The new entry reuses the existing assignment, execution, no-fallback and summary accounting rather than maintaining a second command list. Records and summaries name the selected registration, so they cannot be confused with old results.

Synthetic controls must prove successful exact captures for PNG and BMP, sequence completeness and ordinary/captured output identity. Negative controls change one RGB sample, one encoded image sample, source bytes, command, executable, registration, ordinal, geometry, frame count or a digest, and remove or add files or the completion marker. A reference-only pixel change must fail the reference comparison without falsely blaming the lossless encoder. An encoder-only pixel change must fail the lossless comparison. Test nonzero exits, missing captures and malformed receipts. Keep prior-registration exclusions and all unrelated predicates covered by their existing tests.

Linux GitHub Actions runs the real pinned-oracle synthetic controls and affected Rust/Python tests. No new V2 pass count is claimed by this change. A merged, explicitly selected run and its later result record are required for any coverage claim.

## Running the tools

`tools/compat-corpus/png_bmp_yuv.py` is the opt-in runner. The existing `encode_breadth.py` and `current_ci.py` remain unchanged. Acquisition requires a clean checkout whose commit is on `origin/main`. Build the normal executable and copy harness with the registered full-build features, and build the `coreav-cli` test target `image_capture` with those features plus `input-capture`. That test target has its own main function so decoder worker re-entry does not run the Rust test dispatcher.

Supply a build record with schema `coreav.image-build/1`, the checkout's `source_sha`, `cargo_lock_sha256`, toolchain and target, and `binaries` entries for `coreav`, `harness` and `image_harness`. Each binary entry contains its `sha256` and explicit `features` array. This is reviewed build provenance, not cryptographic source-to-binary attestation.

After merge, run:

```sh
python3 -I tools/compat-corpus/png_bmp_yuv.py run \
  --registration png-bmp-yuv-v1 \
  --coreav COREAV --harness COPY_HARNESS --image-harness IMAGE_HARNESS \
  --ffmpeg FFMPEG --ffprobe FFPROBE --x264 X264 \
  --build-manifest BUILD.json --fixtures FIXTURES --work NEW_WORK_DIRECTORY \
  --records RECORDS.jsonl --summary SUMMARY.json --manifest MANIFEST.json
```

The runner retains per-command capture and process evidence under the fresh work directory, with diagnostic comparisons in the records. Keep that directory with the run's evidence. Public records contain no new coverage claim until a merged run is recorded separately. `summarize --registration png-bmp-yuv-v1 --records RECORDS.jsonl --manifest MANIFEST.json --summary SUMMARY.json` rejects mixed registrations, changed command rows and changed judging-tool identities before applying the existing V2 accounting.

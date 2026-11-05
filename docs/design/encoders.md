# Built-in encoders

Status: The standalone explicit AV1 route and its version 2 summary are implemented for opt-in Unix native builds. Parser-only builds and platform selections still refuse unavailable backends. The small correctness fixture checks real encoding; reference performance figures are separate.

The default output remains H.264 video with copied audio in MP4. AV1 is explicit. [Policy record 0043](encoder-policy.md) adds the platform and licence exceptions to record 0042. The existing [transcode design](transcode.md) continues to describe shipped behavior. This note specifies the new backend contract and its acceptance bars, not a claim that they have passed.

The separate [G1 H.264 encoder design](h264-encoder.md) pre-registers a self-written, full-build-only intra spike. It adds no backend or command-line option to this plan.

## Output and encoder selection

Codec selection and encoder selection are separate. `--video h264|av1` selects the output codec. Omission means H.264 on every platform. `--encoder` accepts `auto`, `x264`, `ffmpeg`, `platform` and `svt-av1`. An explicit encoder never falls back. Selecting an encoder incompatible with the requested codec is an `unsupported_feature` error.

The following table specifies `auto` after integration. A platform backend must be compiled in, available at runtime, and able to honor the request before it is eligible.

| Output | Linux | macOS | Windows |
|---|---|---|---|
| H.264 video | Installed x264, then installed FFmpeg with libx264 | Installed x264, then VideoToolbox, then installed FFmpeg with libx264 | No transcode until the Windows prerequisites land. Later, x264, Media Foundation, then FFmpeg with libx264 |
| AV1 video | Sandboxed SVT-AV1 | Sandboxed SVT-AV1 | Deferred until the Windows sandbox and decoder work |
| Copied audio | Existing packet-copy path | Existing packet-copy path | Subject to the same Windows prerequisites |
| Opus audio | Sandboxed libopus | Sandboxed libopus | Deferred until the Windows sandbox work |
| AAC audio | Installed FFmpeg's AAC encoder | AudioToolbox, then installed FFmpeg's AAC encoder | Deferred. Later, Media Foundation, then FFmpeg's AAC encoder |

AV1 auto does not discover an external FFmpeg as a substitute for a missing built-in backend. Explicit `--video av1 --encoder ffmpeg` is outside phase 1. There is no AV1-to-H.264 or AAC-to-Opus substitution. The audio encoder is chosen separately from the video encoder, so `--encoder platform` selects the video backend, not both tracks. Phase 2 adds a separate `--audio-encoder auto|platform|ffmpeg` choice for AAC. It does not change which source audio streams are eligible for encoding.

Only capability failures discovered before encoding may advance auto to its next candidate. Invalid parameters are refused, not hidden by fallback. Once a backend accepts frames or samples, an initialization, callback, encode, validation, or finish failure ends the command. The temporary output is removed. Retrying a partly completed job through another encoder would hide errors and change its quality.

### Settings have encoder-specific meanings

Argument presence must remain distinguishable from a CLI default. Unset x264 settings retain CRF 23 and preset `medium` when x264 or FFmpeg with libx264 is selected. An explicit x264 preset or CRF is not translated to a platform quality scale. In auto mode that request makes the platform candidate ineligible. An explicit platform selection refuses it.

SVT-AV1 initially accepts 8-bit 4:2:0 SDR frames, a numeric preset from 0 through 13, and its own CRF from 0 through 63. For `--video av1`, unset values mean preset 8 and CRF 32. Both controls require integer tokens, not fractional or exponent notation. AV1 refuses bitrate and thread-count controls. Parallelism level 1 is not a thread limit. Unsupported geometry, pixel format, frame rate, or settings are refused. There is no implicit bit-depth, color-range, or frame-rate conversion. A future 10-bit extension needs its own tests and bars.

The initial VideoToolbox path accepts a target bitrate, H.264 High profile, 8-bit 4:2:0, no frame reordering, and a maximum keyframe interval of two seconds. With no rate option, its target is 2,000,000 bits per second. This is a bitrate default, not a claim of x264 CRF equivalence. AudioToolbox initially accepts AAC-LC, the source channel layout and rate where supported, and a target bitrate of 128,000 bits per second for mono or 192,000 for stereo when unset. Other layouts require an explicit supported bitrate. Unsupported channel layouts or rates are refused, not resampled or downmixed silently.

VideoToolbox auto permits either hardware or software encoding and reports what actually ran. `--require-hardware` requires the VideoToolbox backend and a hardware VideoToolbox session, and fails rather than falling back to software or another program. The backend is not connected in this phase, so the option reports that no hardware encoder is available before reading input or touching output. It is incompatible with a non-platform video encoder. Software availability is a runtime capability, not inferred from an OS version or the presence of the framework.

### Missing encoders and strict-safe mode

The first selection phase preserves legacy missing-encoder diagnostics and their precedence. H.264 requests using only the existing `auto`, `x264` or `ffmpeg` encoder choices and no new selection flag retain their messages, summaries and exit statuses, including when both the input and encoder are unavailable. Early selection refusal applies to `--video av1`, `--encoder platform|svt-av1`, `--audio-encoder` and `--require-hardware`. Moving legacy diagnostics before media reads is a separate later change that requires the release owner's agreement.

For new selection requests, build, OS, executable discovery, and parameter-only capability failures are reported before media reads and before an output is created. Header-known incompatibilities follow the bounded header probe, before decoding or encoding. Decoded-only requirements are checked before the first frame is submitted to the encoder. An unsupported format or size change discovered later terminates with cleanup and no fallback. These refusals use the existing `unsupported_feature` error and transcode's existing error exit status. These are transcode diagnostics, not changes to the separate compatibility command's exit status 4.

Example diagnostics after integration are:

```text
no H.264 encoder on Linux: install x264 or FFmpeg with libx264, or select built-in AV1 with --video av1
no AV1 encoder: this build has no SVT-AV1 backend
no AAC encoder: install FFmpeg or select --audio opus
no platform H.264 encoder is available on this Mac
no hardware H.264 encoder is available: omit --require-hardware to permit software
the selected platform encoder cannot honor --crf; use --bitrate or select x264
strict-safe mode does not permit an in-process platform encoder
```

The AV1 suggestion appears only when that build has an eligible SVT-AV1 backend. Strict-safe mode keeps its current refusal of FFmpeg and of sandboxed encoders on systems without enforceable memory limits. Sandboxed SVT-AV1 therefore remains unavailable in strict-safe mode on macOS. The in-process platform exception does not certify enforceable memory or deadline limits, so strict-safe refuses it too. Selection filters forbidden candidates rather than using a platform backend to bypass strict-safe.

Calling a built-in H.264 encoder does not supply an H.264 decoder. Default-build input limitations remain. The full build adds the existing native decoder independently of encoder selection.

## Summary contract

The `coreav.transcode/1` schema closes its encoder-name enum and rejects additional properties. New metadata must not be inserted into that schema or repurpose `video.codec`, which describes the source. H.264 jobs retain the unchanged version 1 document.

Only completed explicit AV1 jobs use [`coreav.transcode/2`](../schema/transcode-v2.md). Source codec and decoder remain in `video`; `video.encoding` reports the software AV1 backend, version, supplied sandbox launcher, effective settings and completed session counts. Settings cadence is separate from source timestamps. Copied streams, encoded audio, dropped streams and output size share their version 1 meanings. The schema, reference, illustrative example, serializer and tests change together. A successful receipt is not native correctness or independent decoder evidence.

### Later platform metadata

Platform integration must define its own versioned serialization for the following metadata requirements. These proposed fields are not additional keys in the AV1 version 2 schema:

| Field | Meaning |
|---|---|
| `output_codec` | `h264`, `av1`, `aac`, or `opus`, independent of the input codec |
| `encoder` | Stable backend name such as `svt-av1`, `videotoolbox`, `audiotoolbox`, `x264`, `ffmpeg`, or `libopus` |
| `encoder_version` | The linked library version, or the external program version. Null when no reliable version can be obtained |
| `encoder_id` | The actual OS encoder identifier when the framework exposes one, otherwise null |
| `encoder_kind` | `hardware`, `software`, or `unknown`, based on the selected implementation, not the requested preference |
| `encoder_isolation` | `sandboxed`, `in_process`, or `external_process` |
| `encoder_os_build` | OS version and build for a platform encoder, otherwise null |
| `encoder_settings` | Effective rate control, preset when applicable, bitrate, profile, keyframe interval, and thread limits |

The video object and each audio-stream object carry their own metadata. A copied audio stream reports copy, not an encoder or an invented version. OS build and encoder identifier do not become a fake library semantic version. For a platform API with no version query, `encoder_version` is null and the OS build remains available for reproducing measurements. Unknown hardware status is not reported as software. Summaries report the encoder that ran, not the first candidate that was tried.

No `coreav ffprobe` output changes. `coreav ffmpeg -c:v libx264` must continue to select x264 or FFmpeg with libx264, never an OS encoder merely because transcode auto changed. New compatibility vocabulary and any ffprobe change require the compatibility surface owner's agreement and a separate scope. This note authorizes neither.

## Process and validation boundaries

The initial shared video contract in `coreav-transcode::contract` assigns sequential frame identities to checked, owned pictures with strictly increasing presentation times. The submission ledger retains times rather than pixels. The packet ledger bounds sample extents, count and total bytes, then requires one complete sample per submitted identity at drain and derives timing in decode order. Codec-specific readers still validate syntax, random-access status and configuration; accepting a packet into this ledger does not validate its codec. The AV1 path uses these contracts; the legacy H.264 frame owner retains its existing behavior.

SVT-AV1 follows libopus. The default path reexecutes the binary as a sandboxed encoder worker. It receives checked frames and parameters, not a source file or compressed source packets. The parent bounds queued frames, shared buffers, packet sizes, total output, and work against one deadline. EOF drains the encoder. Crash, timeout, premature output completion, or malformed output fails and reaps the child. The in-process comparison uses the same safe backend and validators in a measurement-only executable.

VideoToolbox and AudioToolbox run in-process under record 0043. Their safe wrappers own all framework references and callback state until completion and invalidation. A callback cannot outlive its owner, retain a caller's borrowed frame, or grow an unbounded queue. Framework buffers are copied into bounded Rust-owned storage before release. A deadline check before and after a synchronous OS call cannot interrupt that call, and the summary must not imply otherwise.

Safe output readers validate AV1 OBU lengths and sequence configuration, H.264 parameter sets and access units, or AAC configuration and packetization, as applicable. They also validate packet extents, submitted-frame identity, presentation and decode order, durations, sample counts, and final drain counts. MP4 sample entries come from validated output configuration, not an unchecked framework description. The readers keep fuzz targets and hostile-output tests. Encoded-byte equality is never an encoder correctness criterion.

## Independent implementation tracks

Phases 1 and 2 start from this contract in parallel. Neither depends on the other's code. The phase number gives priority, not a serial dependency. Each track delivers its backend, a standalone test driver, and raw-frame or PCM acceptance evidence. User-facing completion additionally requires the integration below.

| Owner | Files it alone writes | Deliverable |
|---|---|---|
| Phase 1 | `crates/coreav-backend-svt-av1/**`, `docs/design/encoders/svt-av1.md`, `scripts/encoders/svt-av1/**` | Safe SVT-AV1 wrapper, checked AV1 output reader, worker protocol and worker entry implementation local to the backend, static build recipe, conformance and quality driver, sandbox and in-process comparison |
| Phase 2 | `crates/coreav-backend-apple/**`, `docs/design/encoders/macos.md`, `scripts/encoders/macos/**` | Safe VideoToolbox and AudioToolbox wrappers, checked H.264 and AAC output readers, callback lifetime and failure tests, capability and hardware reporting, reference-Mac driver |
| Integration | All existing shared files and the two new workflow files | Workspace registration, shared selection and pipeline, worker dispatch, CLI and schema, packaging, global safety and licence checks, end-to-end acceptance |

The two backend tracks write no shared file other than an agreed contract correction to this note or unsafe-site rows supplied by the integration owner. Each has its own test manifest and lockfile until workspace registration. They may develop and test in isolation, but must not bypass the global unsafe-site or licence checks to merge production code. Actual unsafe files are registered in `docs/unsafe-sites.md` in the same PR that adds them, signed off by the integration owner. Local inventories and the finished source branch go to that owner before the PR opens; the owner checks the rows and posts its sign-off on the PR. The independent reviewer still reviews the whole change. The integration owner owns the SVT-AV1 dependency-scoped licence exception before that backend's production PR opens. The native dependency is checked by `scripts/svt_av1_licenses.py`, separately from Cargo policy; the Rust glue keeps `MIT OR Apache-2.0`. [Native notice checks](../release/notices.md#native-svt-av1-notice-checks) describe the exact scope. Empty speculative site rows are not added in phase 0, and the missing-file guard remains unchanged.

The integration owner is the only writer of root `Cargo.toml` and `Cargo.lock`, crate feature lists, `deny.toml`, `coreav-worker`, `coreav-transcode`, `coreav-cli`, the summary schema, notice inputs, `packages/cli/scripts/codecs.sh`, `platform.sh`, and shared CI, and owns the content of `docs/unsafe-sites.md`. The owner integrates one backend at a time without blocking the other track's development. It adds codec-neutral submitted-frame and encoded-packet contracts once, then connects each backend to them. Phase 1 owns AV1-specific parsing, not an H.264-to-AV1 patch in the shared pipeline.

Phase 1 covers all four supported native Unix release targets, Linux musl and macOS on x86-64 and AArch64, in default and full builds. It does not claim support for every Unix OS or Windows. Phase 2 covers both macOS release architectures. Cross-architecture CI smoke checks do not replace reference-host speed bars. The integration owner coordinates command help, schema publication, packaging and release-plan updates with their owners. Phase 0 changes none of those files.

## Pre-registered measurement protocol

The thresholds below are committed before any encoder acceptance run. The size experiment is separate and makes no encoder performance claim. Before a judged run, commit its candidate, driver, exact input roster with full SHA-256 hashes in order, binary hashes, version pins, effective settings, and host manifest. Missing measurements, oracle errors, absent metrics, and timeouts cannot count as passes. No threshold is weakened after seeing results. A failed arm is final unless a recorded host fault voids it.

### Common method

Use the repository's exact FFmpeg pin from `scripts/oracle-ffmpeg.env`. A measurement-only build may enable the required encoder and metrics, but retains that FFmpeg version and records its configuration and hash. It is never bundled with CoreAV. The same SVT-AV1 version, static archive, preset, rate mode, tune, thread count, lookahead, tile configuration, keyframe interval, pixel format, geometry, color description, and frame rate apply to both SVT arms and FFmpeg. Version or setting mismatches void the comparison rather than become performance evidence.

Video quality uses the same predecoded raw reference frames for both encoders, in presentation order, without implicit scaling, rotation, or color conversion. The initial scope is 8-bit 4:2:0 SDR. Evaluate VMAF with the pinned `vmaf_v0.6.1` model at native resolution without pooling different model variants, all-plane average PSNR, and SSIM `All`. Record the metric tool and model hashes. Decode each encoded result first, require its frame count and order to match, and compare each reconstruction to the same source frame. Quality deltas are CoreAV minus the FFmpeg baseline. Report actual elementary-stream payload sizes as well as MP4 sizes.

Each phase selects ten eligible held-out inputs in each of four height classes, at most 360, 720, 1080, and 2160 pixels, with width at most 3840. Classes are disjoint by their upper bound. Eligible videos have at least 240 and at most 1800 frames, 8-bit 4:2:0 SDR output, and duration from 8 through 30 seconds. Sort by SHA-256 of `coreav-encoders-v1:` followed by the full file hash and take the first ten per class. Commit the selected video stream and exact presented-frame count. A class with fewer than ten inputs makes the planned round incomplete, not a smaller passing set. Training inputs are excluded. No input is replaced after a failure.

For speed, use a fixed host, one warmup and five measured runs per command per input, in alternating AB and BA order. Inputs are in the page cache and outputs on the same RAM filesystem. Time from process start through successful output close and process exit, not a polling interval. A per-input ratio is the median CoreAV time divided by the median baseline time. Report the median of those ratios overall and per class, 95% bootstrap confidence intervals with 10,000 input-level resamples and seed 0, and the share of inputs faster than the baseline. No faster claim is made unless the interval's upper bound is below 1.00.

Integrated comparisons consume the same original input streams, encode and copy the same selected tracks, and apply the same timestamp, rotation, color, and MP4 layout policies. Freeze the exact paired commands, decoder thread controls and encoder settings before the run. No command fallback or per-input search for a different baseline is permitted. The integrated outputs themselves must pass the corresponding conformance, quality and payload-size bars. A raw-driver pass does not cover them. Report unavailable source decoders separately before the roster is frozen, not after a supported pair fails.

An A/A baseline gate runs before and after each host session. Median gate times must differ by no more than 3%, with no swap, throttling, thermal warning, or competing encoder job. The null comparison must contain 1.00 in its 95% interval. A failed gate voids the session and retains its evidence. Thread limits, CPU affinity, memory limits, OS build, and background-load observations are recorded. CI runners provide correctness and build evidence, never timed speed acceptance.

### Phase 1, SVT-AV1

The fixed speed host is a GCP `c4-standard-16`, SMT off, eight pinned physical cores, standard provisioning. Both sides get eight threads and the same affinity. Correctness also runs natively on all four Unix release targets. Presets 6 and 8, CRF 32, two-second maximum keyframe interval, film grain off, and tune PSNR are separate judged cells. No cell is dropped because another preset passes.

The reference is `ffmpeg -c:v libsvtav1` linked to SVT-AV1 4.2.0, the version of the initial size experiment. If implementation needs another stable version, commit that amendment and both rebuilt binaries before the first acceptance run. Other controls are explicitly captured from the effective SVT configuration, not assumed equal because wrapper defaults have similar names.

| Bar | Pass condition, in every preset cell and height class unless stated otherwise |
|---|---|
| A1, conformance | 100% of supported outputs decode completely with pinned dav1d 1.5.3, with no decoder error. Presented-frame count, dimensions, order and color description match the reference. Presentation times agree within half a source tick. A/V start offset differs by at most 1 ms. AV1 configuration and MP4 indexing pass the independent probe |
| A2, quality | Median delta VMAF at least -1.0, PSNR at least -0.10 dB, and SSIM at least -0.001. At least 98% of inputs have all three deltas at least -2.0, -0.50 dB, and -0.005 respectively. Every fixture passes those per-input limits |
| A3, bit budget | Each input's video payload size ratio is between 0.98 and 1.02, so extra bits cannot purchase a quality pass. Different encoded bytes are permitted |
| A4, speed | Median sandboxed CoreAV-to-FFmpeg ratio at most 1.00 overall and per class, for both the raw-frame encoder driver and the integrated transcode command. Report them separately. A raw-frame pass is not an end-to-end pass |
| A5, safety | All malformed-output, child-crash, timeout, early-EOF, queue-limit and cleanup fixtures fail safely, without a panic, orphan, partial final output, or deadline overrun beyond process teardown. Fuzz the AV1 output and worker-message readers in affected-target CI |
| A6, distribution | All four Unix targets build static SVT-AV1 into both native release flavors. Linux has no ELF interpreter or shared dependency. Runtime smoke verifies the selected version and encoder. Required notices are present |

The integration run also preserves copied audio packets and timing under the existing transcode copy bars. Unsupported inputs are listed separately with the exact pre-registered reason. A supported input that fails cannot be relabeled unsupported after the run.

The isolation-cost arm compares the production sandboxed driver with a measurement-only in-process driver on the same raw frames, library, settings, output validation, host and run schedule. Both must pass A1 through A3 before a timing ratio is meaningful. Report sandboxed divided by in-process elapsed time, CPU time, peak resident memory per process and total, process startup, and bytes copied per frame. Include a separate one-frame fixture to expose startup cost. This arm is diagnostic, has no pass threshold, and cannot authorize in-process shipping or rescue an A4 miss.

### Phase 2, VideoToolbox and AudioToolbox

The named reference profile is `ENC-MAC-1`, a dedicated Mac mini M1-M with Apple M1, eight CPU cores and 8 GB unified memory, running macOS 15.8.2, build 24H232. Results apply to this OS profile, not macOS 15.7. This replaces the former Mac mini M4 Pro profile before any run. It is a designated measurement profile, not a claim that a host has been provisioned. Record its exact OS build, framework encoder IDs and hardware status before the run. If it is unavailable, obtain that reference host or commit a replacement profile before measuring. A virtual runner cannot substitute for it. Absolute speed is M1-specific; the judged speed claim is the same-host ratio against `h264_videotoolbox`. The [VideoToolbox plan](../../scripts/encoders/macos/videotoolbox-plan.md) fixes the run and registration gates.

Video compares against the pinned FFmpeg's `h264_videotoolbox` on that same Mac. Hardware and software sessions are separate cells, with hardware selection explicitly required or disabled, not inferred. A software cell may be marked unavailable only if both APIs fail the preflight smoke before the roster is encoded. Such a result supports no software availability or performance claim. The hardware cell is required. No unavailability exclusion applies after a session starts encoding.

Both sides use bitrate control, High profile, 8-bit 4:2:0, no frame reordering, the same two-second keyframe interval and color metadata. Target rates are 1, 3, 6, and 20 Mbit/s for the four height classes. Record requested and effective controls, and refuse a cell whose settings cannot be matched. Do not compare an x264 CRF number to a VideoToolbox quality number. Software-only correctness runs on a hosted Mac, if available, remain distinct from this reference-host run.

| Bar | Pass condition for every available video cell and height class |
|---|---|
| M1, conformance | 100% decode without errors with the pinned FFmpeg H.264 decoder and, within its supported profile, CoreAV's native decoder. Frame count, dimensions, order, times and A/V offset meet A1. MP4 parameter sets and sample descriptions agree with the decoded stream |
| M2, quality | Meet A2's VMAF, PSNR and SSIM bars against `h264_videotoolbox`, each output compared to the common raw source |
| M3, bitrate | Each input's payload size ratio is between 0.95 and 1.05, and each side's average payload bitrate for clips of at least 8 seconds is within 10% of the target |
| M4, speed | Median CoreAV-to-FFmpeg ratio at most 1.00 overall and per class, separately for the raw-frame driver and integrated transcode |
| M5, safety | Every callback lifetime, cancellation, unavailable encoder, oversized output, malformed configuration, bounded queue and cleanup fixture passes. No use-after-free, panic or silently truncated success. Fuzz the output readers |

AAC compares against FFmpeg's `aac_at`, with AAC-LC, constant bitrate, identical input float PCM, channel order, sample rate, bitrate and encoder quality setting. Use mono and stereo at 44.1 and 48 kHz, at 128 and 192 kbit/s respectively, as four cells. In each cell select the first ten held-out eligible audio inputs of 8 through 30 seconds in the same hash order as above, freezing their hashes before the run. No implicit rate conversion is allowed. Use the shared oracle audio input arguments from `scripts/oracle-audio-args.txt` for every audio decode, including metric references.

AAC conformance requires every output to decode cleanly with the pinned FFmpeg AAC decoder. After declared priming and padding are trimmed, sample count differs from the source by at most one sample, with no channel swap and no offset above one sample. Fixtures cover impulse placement, silence, drain, timestamp gaps and priming. Native transcode encodes video too. Its integrated AAC cell therefore uses the same frozen video workload and matched VideoToolbox settings on both sides, and must also pass M1 through M3 for that video. A copied-video AAC command is not added by this phase.

For audio quality, compute full-clip SNR against the common source after that declared trim, with no fitted gain or searched time shift. Per cell, median CoreAV-minus-`aac_at` SNR is at least -0.10 dB and every input is at least -0.50 dB. Silent exact reconstruction is reported as such and passes only if both sides reconstruct silence. No NaN or unhandled infinite score is accepted. Each payload size ratio is between 0.95 and 1.05 and each side's mean bitrate is within 10% of target. These bars compare the wrapper to the same OS encoder, not AAC's perceptual quality against a different codec.

AAC speed uses the same host, warmup, five-run schedule and bootstrap method. The median ratio is at most 1.00 per cell and overall for the PCM driver and integrated video-plus-AAC transcode separately. Report the PCM result as the AAC encoding cost and the integrated result as the combined command, not an isolated AAC speed claim. AudioToolbox must also pass M5's lifetime, queue and malformed-output tests. A video pass does not cover AAC, or the reverse.

## Hosted macOS capability

Software VideoToolbox H.264 on GitHub's macOS runners is not verified by this work. Apple documents disabling hardware acceleration and querying the implementation that ran. GitHub documents hosted Macs as virtual machines, but neither vendor guarantees an available software H.264 encoder in those images. No hosted encoder job was run for this note. Do not turn a framework's presence or a successful session creation into a passing encode smoke test.

Phase 2's capability smoke must submit frames, drain, and independently decode the resulting H.264 stream. It records runner image, OS build, encoder ID, actual hardware status, submitted and decoded frame counts, and errors. If software is absent, hosted CI checks the refusal and resource cleanup. Physical-host conformance and speed bars remain required.

## Documentation sources

- [SVT-AV1 4.2.0 release notes](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/v4.2.0/CHANGELOG.md) and [build guide](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/v4.2.0/Docs/Build-Guide.md).
- [FFmpeg libsvtav1 options](https://ffmpeg.org/ffmpeg-codecs.html#libsvtav1), [VideoToolbox overview](https://trac.ffmpeg.org/wiki/HWAccelIntro), and [AudioToolbox AAC options](https://trac.ffmpeg.org/wiki/Encode/AAC#aac_at). Effective options must also be captured from the pinned test binary's help.
- [Apple's hardware-enable control](https://developer.apple.com/documentation/videotoolbox/kvtvideoencoderspecification_enablehardwareacceleratedvideoencoder), [hardware requirement](https://developer.apple.com/documentation/videotoolbox/kvtvideoencoderspecification_requirehardwareacceleratedvideoencoder), and [selected implementation query](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_usinghardwareacceleratedvideoencoder).
- [GitHub-hosted runner descriptions](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

Research used documentation, licences and release notes. Search results exposed build-file code and a short encoder-code snippet. Those source pages were not opened, the snippets were not used, and no encoder implementation source file was opened. The throwaway experiment compiles upstream releases without examining their source.

## Static-link size experiment

This experiment measures linked code, not encoding speed or the size of a finished backend. It has no pass threshold. The method is committed before the build.

Build the current `coreav-cli` with `release-default`, `--profile dist`, `x86_64-unknown-linux-musl`, fat LTO, one codegen unit, and stripped symbols. Build a pinned SVT-AV1 release as a static musl library with release optimization and no applications or tests. Relink the same CLI with that archive and explicit linker roots for the documented encoder lifecycle, configuration, input, output, and version functions. Do not add an encoder implementation or invoke those functions. Keep section garbage collection enabled.

Record the base and linked executable sizes in bytes, their difference, the static archive size, ELF dependency checks, the SVT-AV1 archive hash, compiler versions, build flags, and retained API symbols before stripping. Also record an all-sections-retained link and a matching base with section garbage collection disabled, so that retained CoreAV sections do not masquerade as an SVT increment. A symbol root must survive into executable code or the experiment fails. Neither result includes the future Rust wrapper, worker protocol, or notices. The final backend must repeat the packaged-size measurement.

### Measured linked-code footprint

The completed throwaway build on 2026-11-02 adds **6,940,608 bytes, or 6.62 MiB**, to the stripped static musl CLI when the encoder API is retained. This is 45.49% over its matched base. It is a linked-code estimate, not the size of a finished encoder feature, its runtime memory, or a compressed release archive. No encoding or quality measurement was performed.

| Link mode | Base executable, bytes | With SVT-AV1, bytes | Increment, bytes |
|---|---:|---:|---:|
| Encoder API roots, section garbage collection enabled | 15,257,312 | 22,197,920 | 6,940,608 |
| Whole archive retained, section garbage collection disabled on both sides | 15,290,112 | 22,399,840 | 7,109,728 |

The static `libSvtAv1Enc.a` is 10,628,794 bytes. Its object-file and LTO representation is not an executable-size estimate. The second pair is a conservative comparison for this build configuration, not an upper bound on future wrapper or worker code.

The source snapshot is `ab6646b0022e093e042e11e73488e7fe79eb7c40`. Only documentation distinguishes that snapshot from its product-code base. The host was Debian 13 x86-64 on a GCP `c4-standard-8`. The CLI used Rust 1.99.1, compiler commit `58cf8f9c35b355199727627d07ae34a4e2846d67`, `release-default`, fat LTO and one codegen unit from the `dist` profile. The C compiler was `musl-gcc` backed by GCC 14.2.0. The existing codec build script supplied static dav1d, libvpx and libopus. This is release-style linkage, not a packaged release build. In particular, the C compiler for ring was musl-gcc rather than the packaging script's zig, and notice generation and path remapping were not run. Both sides use the same choices.

SVT-AV1 was the upstream `v4.2.0` tarball, SHA-256 `c7b13c4a84bd3751aa35fcc72be13e6875467e7c2216879251a486e5b1e4e740`. CMake used `Release`, `BUILD_SHARED_LIBS=OFF`, `BUILD_APPS=OFF`, `BUILD_TESTING=OFF`, `NATIVE=OFF`, `SVT_AV1_LTO=ON`, PGO off, and C release flags `-O3 -DNDEBUG -ffunction-sections -fdata-sections`. The source was compiled without being inspected and removed after installation.

The base command was:

```sh
cargo rustc --locked -p coreav-cli --bin coreav --profile dist \
  --target x86_64-unknown-linux-musl --features release-default
```

The rooted link adds rustc's `-L native=PREFIX/lib -l static=SvtAv1Enc` and one `-C link-arg=-Wl,-u,NAME` per API root. The roots are `svt_av1_enc_init_handle`, `svt_av1_enc_set_parameter`, `svt_av1_enc_init`, `svt_av1_enc_send_picture`, `svt_av1_enc_get_packet`, `svt_av1_enc_release_out_buffer`, `svt_av1_enc_deinit`, `svt_av1_enc_deinit_handle`, and `svt_av1_get_version`. All nine appear as executable text symbols before stripping, including two localized by LTO. No wrapper calls them. The whole-archive control instead uses `-l static:+whole-archive=SvtAv1Enc` and `-C link-arg=-Wl,--no-gc-sections`, with that last flag also on its base.

All four files were stripped with `strip --strip-all`, have no ELF `NEEDED` entry or `INTERP` segment, and successfully print `coreav 0.0.1` with `--version`. Executable SHA-256 hashes are:

| Executable | SHA-256 |
|---|---|
| Base, garbage collection enabled | `60223e6c8ca2f5f09a35df7ada3e0aec8d16675520e57f74583d8d0fe293c714` |
| API-rooted SVT-AV1 | `168e4d63ab16a0906e38df56cdfefd7e2211af89ded56eca0cbfb872b5f2c458` |
| Base, garbage collection disabled | `d5faaeb929a6f01bd1f2dc70491b49fc1d61cf1f3d1b7b5c8dfb9dc2a500f595` |
| Whole-archive SVT-AV1 | `623bc85c851c41daa6bc5026e136e1300885153bd78da85d25428155bbc19fbc` |

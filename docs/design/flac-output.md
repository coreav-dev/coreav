# Native FLAC output

This design adds a lossless FLAC encoder and native `.flac` output to `coreav ffmpeg`. The format reference is [RFC 9639](https://www.rfc-editor.org/rfc/rfc9639.html). MD5 follows [RFC 1321](https://www.rfc-editor.org/rfc/rfc1321.html). Other encoders are executable oracles, never implementation references.

## Ownership and interface

`coreav_audio::flac` already owns FLAC frame decoding, STREAMINFO and the format's CRC functions. Its `encode` submodule owns encoding. A `FlacSink<W: Write>` implements the existing audio `Sink`, taking interleaved signed little-endian 16-bit or packed 24-bit samples. It buffers at most one 4,096-sample block. `finish` consumes the sink, writes its final frame and returns the completed file header for the existing atomic output writer to patch. No seekable writer, whole-file PCM buffer, external encoder or new dependency is needed.

Two complete alternatives were considered. A whole-buffer `encode(&[i32]) -> Vec<u8>` API makes frame testing straightforward but requires the CLI to retain the entire decoded input and duplicates the existing sink protocol. A streaming sink keeps decoding, trimming and publication with their current owners. The streaming sink is selected. Private frame planning remains independent of I/O so the decoder can check every emitted frame.

The sink has explicit new, encoding and failed states. A failed write cannot resume encoding. The decoder's `Limits` bound channels and total input samples. Format validation precedes allocation, and frame scratch is bounded by the fixed block size and eight channels. Arithmetic for prediction and stereo differences uses signed 64-bit integers. A predictor with an unrepresentable residual is discarded, not clipped.

## Encoding decisions

Frames use fixed 4,096-sample blocks and a shorter final block. Each subframe chooses the smallest complete representation among constant, verbatim, fixed predictors of orders zero through four, and linear predictors of orders one through eight. Linear prediction uses autocorrelation and Levinson-Durbin recursion. Quantized coefficients have at most 15 bits and a nonnegative right shift. Floating-point estimation chooses coefficients only; the stored residual is calculated with integer arithmetic and reconstructs the original sample exactly.

Residuals use partitioned Rice coding with four-bit parameters and partition orders zero through four when the block size permits them. The planner charges headers, warm-up samples, coefficient fields and every residual bit. Verbatim coding bounds the chosen representation even for incompressible input. Wasted low bits are removed only when every sample has those zero bits. Stereo chooses the smallest of independent, left-side, side-right and mid-side coding. Other channel counts use independent subframes and the RFC's channel order.

The file starts with `fLaC` and one STREAMINFO block. The completed header carries the sample rate, channels, stored width, fixed block size, actual frame-size bounds, total samples and MD5 of the original interleaved sample bytes. Every header has CRC-8 and every frame has CRC-16. Empty input has no frames and has the MD5 of the empty message. The encoder does not promise the streamable subset for sample rates that require a STREAMINFO reference.

## Compatibility command

`-c:a flac`, `-acodec flac`, `-f flac` and `.flac` select native output, with FLAC as the container default. Existing codec specifiers retain last-match-wins resolution. Explicit maps must resolve to one audio stream. The CLI initially admits the same decoded lossless source boundary as PCM, currently FLAC in MP4 or Matroska, with 16-bit or 24-bit source samples. Lossy sources retain refusal and the existing external fallback. This unit does not change decoder agreement bars.

The adapter keeps the source rate and speaker layout. Resampling, channel conversion, nonzero seeking, other encoder options, stream-to-global metadata mapping, chapter copying and non-bookkeeping tag copying remain unsupported. `-map_metadata -1` permits omitting global tags. Output `-threads` is accepted without changing this single-threaded encoder. Whole-sample duration caps and input/output zero-time semantics stay with the shared PCM adapter. Unsupported input and decoder warnings never publish a staged output.

PNG output was not merged when this step began. PICTURE blocks and the commands that retain a video stream as a cover image are deferred. FLAC planning must refuse a selected video stream rather than silently remove its cover. Audio-only input, an explicit audio map or `-vn` avoids that unsupported operation. The CLI vocabulary and help/site changes were coordinated with the release owner before opening the PR.

## Correctness bars

These criteria precede all comparisons. Generated inputs cover silence, constants, ramps, impulses, alternating extremes, correlated and independent stereo, deterministic noise and a multi-tone signal. Widths are 16 and 24 bits. Channel counts cover one through eight with RFC layouts. Lengths include zero, one, 15, 16, 4,095, 4,096, 4,097 and multiple blocks with a partial tail. Rates include 8,000, 44,100, 48,000, 96,000 and a rate represented through STREAMINFO.

Both the existing native FLAC decoder and the pinned independent FFmpeg decoder must reconstruct every original sample exactly. STREAMINFO's MD5 must match an independent computation. CRCs, frame lengths, sample totals and the complete file must be checked, not only the first decoded frame. Dedicated cases must exercise each representation, a linear predictor, Rice partitioning, stereo decorrelation, wasted bits, uncompressible input and a short final frame.

For supported CLI commands, compare all decoded samples against the pinned FFmpeg output for the same command. Compare ffprobe's `codec_name`, `sample_fmt`, `sample_rate`, `channels`, `channel_layout`, `bits_per_sample`, `bits_per_raw_sample`, `time_base`, `duration_ts` and `duration` where present. Encoder-identification tags, padding, packet boundaries and compressed bytes need not match. A mismatch fails rather than adding a tolerance or an exclusion.

Invalid formats, incomplete sample frames, writes before start, repeated starts, writes after failure, output I/O faults, limits, unsupported commands and an existing destination all have negative tests. CLI refusal tests retain both the destination's bytes and ordinary fallback behavior.

## Size and speed plan

The default-level reference is the pinned FFmpeg FLAC encoder without a compression-level option. The size bar is a total encoded-byte ratio of at most 1.05 over the registered generated set and at most 1.10 for each compressible case. Empty and very short cases remain correctness cases, not compression evidence. Report all per-case bytes, including noise. Headers and metadata count toward size.

The speed bar is a median native/reference elapsed-time ratio at most 1.00 in every registered class and overall. A faster claim additionally requires the upper bound of a paired 95% confidence interval below 1.00. Report the share of cases faster than the reference, all paired samples and the actual host CPU. Both commands perform the same source decoding and file output, use one thread and use the same input and output storage. Alternate paired order, retain failed invocations and validate complete outputs before accepting any timing. A/A pairs and a null run must establish that process and storage overhead do not dominate the measured work.

The generated measurement set has three classes, each with mono and stereo, both widths and the rates 44,100 and 48,000: a sum of deterministic tones, changing piecewise polynomial signals and deterministic full-range noise. Each input has 30 seconds of samples. Use one untimed warm-up and nine alternating-order pairs per case. Confidence intervals resample complete pairs with a fixed seed. The set and generator are frozen before its first judged run. It is synthetic coverage, not evidence about an unseen real-file population.

All size and speed measurements run in GitHub Actions, never on the development machine. Per crew measurement policy, the plan and tooling merge before they run on judged inputs. Pre-merge runs are limited to correctness tests and tooling smoke cases that cannot judge these bars. Results retain a missed bar and require a separate verdict; they cannot be rerun for a narrow miss. Shared-runner timings are diagnostic for their recorded host, not a fixed-host performance claim. No GCP work is authorized by this unit.

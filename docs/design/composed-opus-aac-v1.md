# Composed Opus-to-AAC check, version 1

This plan registers `composed-opus-aac-v1` before its implementation or execution. The machine-readable registration is [composed-opus-aac-v1.json](composed-opus-aac-v1.json). This is a new check version, not a reinterpretation of B9 or its historical results.

## Scope and unchanged judgments

Only a selected Opus stream encoded to AAC enters the composed path. It covers mono and stereo at 48 kHz from MP4/MOV and Matroska/WebM, without resampling or downmixing. Other source codecs, including lossless sources, retain the existing end-to-end exact comparison. Copied audio, strict-safe behavior, unsupported layouts, video checks and other refusals are unchanged.

The legacy `compare_encoded_audio` predicate stays unchanged. Its source hash in the registration is computed from the UTF-8 source lines of that function, from its `FunctionDef.lineno` through `end_lineno`, including the final newline. Unversioned stored evidence belongs only to the legacy rule. Frozen B9 records, registrations, baselines, judging files and historical outcomes are not edited or recomputed as this version.

The new CLI support removes only the implicit Opus-to-AAC refusal after the new check is implemented. No encoder or decoder algorithm is changed.

## Decoder conformance remains the existing bar

The policy owner is [Opus bars](audio-decode.md#opus-bars). The registered policy hash covers the text after `### Opus bars` and before `3. **Speed.**`. The existing RFC-vector gate remains required: all 24 mono/stereo decodes, exact packet range states, and quality strictly above 90.

For the captured transcode PCM, apply that note's real-input agreement rule. Compare equal presented sample lengths against the pinned FFmpeg's `libopus` decoder, with its F32 samples projected to hard-clipped S16. Project the captured F32 using the existing clipping and nearest/ties-even convention of `coreav-audio/src/convert.rs`. Require `opus_compare` to pass. Compile the existing pinned libopus comparator as a black box, delete its source and never inspect it. Do not substitute an RMS/MAX threshold, default FFmpeg decoder SNR, or the diagnostic lag search for this gate.

Compare presented samples without shifting, dropping, padding, realigning or trimming to the shorter stream. Default FFmpeg Opus SNR may be reported, but does not decide conformance. The test fixtures must contain nonperiodic, asymmetric material so lag and channel mistakes are observable.

The inherited MP4 reference is the source-declared presentation model: acquire the complete libopus decode with edits ignored, establish its length from the selected source's packet headers and `dOps` pre-skip, then apply only that source's independently read edit window. The registered runtime owner is `scripts/composed_audio_source.py`; it also owns native encoder command expectations without changing the frozen audio-input policy checker. No candidate length chooses the reference window. Missing media or an unsupported source edit fails, and the ordinary source-to-AAC baseline remains a separate exact properties and presentation obligation.

## Capture the PCM actually delivered

Use an optional parent-side capture hook at the encoder stdin writer. Capture only the bytes each successful stdin write accepted, including partial-write handling. A decoder output or enqueue-only tap does not prove delivery. Do not replace the encoder with a regular-file tee, relax its sandbox, or add a decoder environment variable.

Capture is explicitly activated and off by default. The normal writer path adds no copy or allocation. The enabled path preserves the actual PCM bytes and order, declared sample rate and channel order, encoder arguments, presentation timing, deadlines and sandbox limits. It does not introduce media delays or timing compensation. Parent-side capture I/O is not performance evidence.

Capture data is private and bounded by the registered byte cap. Require nonempty finite F32, complete frames, and `accepted_bytes == samples_per_channel * channels * 4`. Match the decoder's summary and actual selected stream, not the first audio stream or the packet identity of encoded AAC. A writer, capture, flush, deadline or completion error invalidates the operation. A successful child exit alone is insufficient.

Seal a receipt only after writer completion, encoder/output validation and successful transcode completion. Bind it to the version and registration digest, a fresh call identity, input bytes and stream choice, resolved format/rate/channel order, accepted count and PCM digest, actual encoder program/arguments and sandbox, and output bytes. Publish the receipt last. Failed or incomplete operations cannot leave a receipt that claims success. The harness validates these bindings before using the capture.

## Exact encoder comparison and independent timing

Replay the exact captured F32 bytes into the verified pinned FFmpeg with the expected settings derived from the registered command. First verify that the actual encoder invocation matches those settings. Do not derive a permissive reference command from arbitrary captured arguments: a wrong bitrate, rate, format or encoder must not validate itself.

Require exact ordered AAC packet payloads, codec configuration and presented decoded F32 against this PCM-fed reference. There is no encoder tolerance. Independently require the existing exact audio properties and presentation against the normal source-encode baseline. This keeps source start, priming, sample duration and A/V timing checked without pretending a raw PCM input carries the source container's timeline. No sample alignment or timestamp normalization is introduced.

Presentation means the effective sample-to-presentation mapping after priming, edit lists and terminal discard, not only matching start and duration. Derive that mapping from the actual output bytes and packet timestamps. Require it to agree with the ordinary source baseline and captured sample count, rate and start binding. An interior timestamp or edit mutation must fail even when endpoints and decoded sample bytes stay unchanged. Missing timeline/comparator fields or failed reads fail the check. Cases whose mapping cannot be established, including nonzero starts, cannot pass through a weaker endpoint-only test.

## Versioned acquisition, storage and replay

Every new execution explicitly selects this version. CI uses separately named current transcode-compat jobs and passes the version during shard acquisition and merge. This does not report a legacy B9 pass under a new meaning.

Carry the version and registration digest through the run header, each result and arm, capture receipt, exported call rows, manifests, summaries and merged evidence. Validate them before a resume skips a completed call, before summarization judges a row, and before merge accepts shard data. New-version shards must bind the same candidate and oracle identities.

Reject unknown versions, mixed legacy/current data, mismatched registrations, missing capture bindings and relabeled historical data. Selecting this version on the command line cannot convert an old exact failure into a composed pass. Preserve the existing legacy reader and unversioned merge tests.

## Verification before merge

Use trusted generated fixtures only. No corpus, judged or held-out input is authorized before this PR merges. Preserve the existing command roster, with targeted explicit AAC, bitrate and mapped-source cases. Include both supported containers, mono/stereo, distinct mapped Opus sources and 44.1/48-kHz FLAC controls that retain the original exact check.

Require mutations to fail for wrong PCM or invocation/stream binding, shifted/dropped/duplicated samples, swapped channels, wrong rate/count or encoder arguments, truncated capture, missing writer completion, failed child and mixed/relabelled versions. Transport tests use known bytes and partial-write/error stand-ins. Media tests use the registered nonperiodic generated content and never realign it. A tiny F32 change that disappears in the existing S16 projection is not a new decoder failure criterion; a capture-integrity mutation must fail its byte binding instead.

Keep normal-versus-captured output bytes and timing equal in trusted fixture tests, and preserve sandbox-limit tests. Run the existing instruction guard on the normal path. A later judged or held-out run needs its own authorized input registration; fixture success is not a measured coverage or release verdict.

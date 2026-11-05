# Bounded standalone FLAC to PCM

Plan committed before implementation and generated comparisons. Refs coreav-dev/coreav#25.

## Scope

Add standalone native FLAC input to headerless S16/F32 output, using the existing FLAC parser, decoder, strict integrity validator and PCM interleaver. Initially admit 16/24-bit mono/stereo with known nonzero sample totals, unchanged rate/channels and no explicit speaker-mask comment. Native `fLaC` input at offset zero is sufficient; prefixed inputs, other precision, timing, mixing and resampling remain refused. Files, stdout and the existing bounded encoded-stdin replay share the same source qualification. No AAC eligibility changes are included.

## Source ownership and budgets

Expose a typed source descriptor from the existing standalone FLAC metadata walk, without changing report-only probing. Retain its STREAMINFO, exact metadata-end/payload extent and remaining work/retained-memory accounting. Do not scan for frame sync or infer payload offsets from report strings. Preserve current metadata rejection and explicit-mask qualification boundaries.

Load one immutable encoded payload under every existing cap, including the current 1 MiB box-byte allowance, packet bytes and aggregate retained memory. Reserve fallibly and charge actual capacity together with metadata. Release the initial planning report before reparsing/loading, rather than retaining two independently budgeted reports. Larger or over-budget sources remain explicit limit failures; do not increase a limit or reinterpret an I/O/limit error as unsupported media.

Keep strict validation and interleaving in the audio crate. A two-phase `prepare_stream` API validates the complete immutable payload with the existing `validate_stream` before returning an object that consumes the same bytes when writing through the existing interleaver. The prepared object owns no decoded file. Charge bounded packing scratch alongside planar decoder scratch, and charge both validation and emission against the remaining work allowance. The existing conservative per-sample factor is doubled for the two walks; no global allowance changes.

## Publication and verification

Keep complete CRC, numbering/blocking-strategy, total-sample and MD5 validation before stdout or file publication. A zero MD5 does not permit rereading mutable source bytes after validation: retain the exact encoded payload until emission ends. Reuse the shared file/alias/write-access checks, atomic output, stdout flushing and terminal post-publication error rules. Never fall back after any media output has begun.

Sample ranges are part of that validation. RFC 9639 Section 5 identifies decoded values outside the frame bit depth as invalid and leaves their decoding behavior unspecified. The decoder checks fixed-order-zero residuals before wasted-bit restoration and checks every reconstructed stereo sample against the frame depth before narrowing, including 33-bit side storage. CRCs and an absent or matching MD5 do not establish those ranges. Invalid early or late frames fail before publication for both S16 and F32; they do not inherit the pin's behavior on invalid input. Generated neighboring valid boundaries retain complete pinned-byte comparisons.

Generated positive tests compare complete native raw bytes with checksum-verified FFmpeg 9.0.2 for 16/24-bit mono/stereo, S16/F32, extrema and low bits, one/multiple frames, short terminal blocks, files/stdout/stdin and exact lengths. Negative tests corrupt late frames, CRC/sequence/totals/MD5, metadata extents and trailing bytes; they must emit no stdout and preserve destinations. Exercise independent cap boundaries, metadata-plus-payload accounting, bounded packing, unsupported masks/precision and I/O/limit classification. Retain container FLAC, raw PCM and WAV regressions. No frozen records, judged/held-out inputs, thresholds, tolerances or baselines change.

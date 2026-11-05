# AAC compatibility WAV metadata

## Plan

Reuse the existing compatibility WAV header owner for admitted AAC S16 WAV output. The WAVE-source compatibility path already emits the pinned FFmpeg 9.0.2 encoder tag in its RIFF INFO/ISFT chunk; the AAC path currently uses the generic native WAV header and omits it. Reading the current MP4 and Matroska writer code found no shared compatibility encoder-tag emitter to reuse across containers. This change must not introduce another tag literal, consult FFmpeg implementation sources, or change the native audio library's WAV output.

Cover the existing command forms that select default S16 WAV, explicit S16 WAV, a WAV extension, optional unchanged channel/rate settings, or disabled video. Reuse the same header construction and size checks for known-length file output. Preserve the current AAC stdout contract and all decoder admission, bounded preflight, atomic destination, source-integrity and terminal-error behavior. Do not broaden decoder, conversion or metadata admission as part of this correction.

The compatibility header owner will support known file lengths and unknown pipe lengths while retaining its single INFO payload. Only the registered AAC decode-tolerance S16 path will newly use it; F32 WAV and other decoder contracts remain unchanged. Reuse the existing lossless-output metadata predicate for AAC WAV: refuse chapters and unpreserved default format metadata before output, while retaining the explicit metadata-discard option. Keep the WAVE-source admission rules unchanged.

## Verification

First reproduce the complete WAV mismatch with independently generated development inputs and the checksum-verified pinned binaries. Commit this plan before implementation.

Compare complete output files against the pin for exactly decoded silence and compare complete writer output for identical independently supplied PCM. For PNS-containing development inputs, retain the registered energy, deterministic-projection, packing and exact-presentation checks for the entire decoded payload, and compare every non-PCM output byte with the original-command reference. A tag-only assertion, a common-prefix comparison or a whole-noise-waveform identity claim cannot establish compatibility. Exercise each command spelling, file and stdout size fields, existing destinations and failed publication. Retain the existing WAVE-source whole-output tests and overflow tests.

Retain the registered AAC numerical qualification unchanged and strengthen only its S16 WAV wrapper check to compare all non-PCM bytes against the pin. Run the affected complete CLI test targets with the AAC feature and required pinned oracle, then the workspace suite and all-target Clippy. Check public text, commit shape, frozen-file identity and the final-head similarity guard before handoff.

## Not covered

No frozen representative or judged input is opened or executed. No public result, count, baseline, numerical tolerance or decoder registration changes. AAC output remains compatible within its registered decode tolerance, never exact by virtue of this metadata fix. Native concat qualification, other container metadata, new codec admission and new rate/channel conversions are separate work. Refs coreav-dev/coreav#25.

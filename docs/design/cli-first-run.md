# First-run command compatibility

Keep the FFmpeg compatibility boundary separate from native support. A valid option value that the native parser cannot represent is a fallback request, not a usage error. Missing required arguments and values known to be malformed remain usage errors. Fallback receives the original arguments unchanged, including `file:` prefixes.

## Plan

1. Resolve FFmpeg's `file:PATH` protocol as a local path for compatibility inputs and outputs. Preserve ffprobe's displayed input name and avoid treating a literal file named `-` or a protocol-like path as a pipe or URL.
2. Recognize documented log flags before an optional named or numeric level. Keep unsupported numeric expressions and duration forms on the fallback path rather than rejecting valid FFmpeg syntax.
3. Accept `-ignore_unknown` when native stream selection and writers already reject streams they cannot preserve. Keep `-dn` effective through the shared stream selector.
4. Recognize unsupported standalone MP3, FLAC and WAV containers from their signatures, before trying the ISO reader. Do not turn malformed supported containers or I/O errors into fallback requests.
5. Retain the no-section ffprobe fallback because the native report cannot reproduce FFmpeg's default stderr exactly. Add the native section hint to the fallback notice.
6. On WebAssembly, report the inability to launch FFmpeg, not the absence of an executable. Refuse URL inputs with usage status 2, consistently with native commands in a build without networking.
7. Name unsupported MPEG-TS remux input before creating output. Replace checkout-only help references with documentation-site URLs.

## Verification

Add command-level regressions in `crates/coreav-cli/tests` for native execution, unchanged fallback arguments, strict-safe refusals, malformed syntax, output preservation and the no-network build. Update help snapshots with `COREAV_BLESS_DOCS=1`. Compare grammar decisions with the pinned FFmpeg 9.0.2 binaries as black boxes. Leave registered command shapes and measurement inputs unchanged.

Version-query behavior and additions to the measured command set are separate maintainer decisions; neither changes here.

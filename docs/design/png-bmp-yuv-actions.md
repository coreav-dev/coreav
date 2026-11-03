# Registered image acquisition on GitHub Actions

This prerequisite integrates the merged `png-bmp-yuv-v1` registration with the supplemental V2 fixture registration. It changes acquisition provenance and dispatch wiring, not command rows, output predicates, bars, tolerances or earlier records. No acquisition or new coverage count is part of this change. The plan is committed before implementation; a judged run waits until the implementation has merged.

## Complete provenance

The image runner must hash every file in the encode-breadth input owner's `INPUT_FILES`, including declared supplemental dependencies. Require those files before acquisition and compare their hashes again after acquisition. Its method identity must include the supplemental assignment helper, registry and generator, so a missing or changed owner cannot silently alter the input assignment. Summary replay rejects an incomplete input roster or changed method identity.

The original five-fixture traversal still precedes the registered supplemental tuples. The command set, denominator and miss ordering remain unchanged. Capture execution and integrity failures still void acquisition. Both image pixel comparisons and the existing metadata predicate remain exact.

## Explicit dispatch

Extend `codec-coverage` with an explicitly selected `png-bmp-yuv-v1` fixture-only acquisition mode. Preserve the existing release-candidate gate and legacy dispatch behavior for every other mode. The new mode requires a manual dispatch from main, a full forty-character candidate SHA on main and a clean Linux x86-64 source checkout. Candidate and tool sources are the same immutable main commit; they must contain this integration before acquisition can start. Pull requests cannot select or execute this judged image path.

The workflow builds the normal release-full executable and ordinary copy harness, plus the image harness with release-full and input-capture, from that candidate. It produces the required image build record with binary, source, lockfile, feature, target and compiler identities. Caller-supplied executable paths and partial builds are not alternatives to this build contract.

Generate the original and registered supplemental fixtures with the verified pinned FFmpeg 9.0.2 binaries. Verify supplemental reference assignments before starting the image runner. Invoke the runner with an explicit registration and fresh work directory. Retain its records, summary, manifest, build record, host information and complete per-command capture/process evidence, including failed acquisition evidence. The run remains separate from V1 and does not fetch held-out or corpus inputs.

## Verification and later results

Use only mocked acquisition and newly authored synthetic controls before merge. Regressions must reject missing supplemental inputs, missing helper or registry identities, changed input bytes and changed method identities. Execute the workflow's selection guards with pull-request, non-main, malformed-SHA and valid manual-dispatch cases, and prove that only the intended mode builds and invokes the image harness. Keep the source and host refusals covered.

Check the frozen judging roster and command rows remain byte-identical. After this prerequisite merges, select one merged main commit, execute the combined V2 acquisition once and publish its observed counts and exact image diagnostics in a separate results pull request. A tooling fault is not a count or a reason to change acceptance.

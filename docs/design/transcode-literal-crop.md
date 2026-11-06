# Literal crop through `coreav ffmpeg`

## Scope

Accept one literal `-vf crop=W:H:X:Y` (also `-filter:v`) on the existing H.264 encoder adapter. All four fields are one to five ASCII digits and even. Width and height are 2 to 16,384. The rectangle must fit every decoded frame. Only planar 8-bit 4:2:0 is admitted. Expressions, omitted or named fields, filter chains, odd values, and crop beside `-s` or scale remain refused. Repeated filter options retain the existing last-value-wins rule. No new `coreav transcode` flag, encoder, decoder, format conversion or generic filter engine is added.

## Ownership

The adapter parses a rectangle and passes it through the CLI request to an optional `TranscodeOptions` crop. The pipeline refuses crop beside scale, variable-size policy or AV1 before source access. It validates the rectangle and pixel format against decoded geometry before encoder startup, then checks every frame against the first size. Cropping uses the existing `Picture::crop`, including for shared frames that would otherwise bypass CPU preparation. The unchanged no-crop branches keep their direct/shared writes and scale behavior.

The crop allocation is no larger than its decoded source. Existing decode limits, encoder memory accounting, the checks before and after frame preparation, the deadline and atomic publication remain in force. Cropping preserves pixel shape and a valid declared sample aspect ratio, including 1:1; it does not preserve the uncropped display aspect ratio. Absent declarations remain absent. Existing scale normalization is unchanged.

An enum replacing every scale option was rejected because it would rewrite unrelated AV1 and CLI scale paths. The optional rectangle keeps this change within the encoder adapter and its shared frame preparation, with an entry guard for invalid library combinations.

## Verification fixed before implementation

Parser tests cover the accepted literal, the existing scale forms and last-value-wins behavior. Separate refusal tests cover expressions, chains, crop with scale or `-s`, odd width, height and offsets, omitted or extra fields, signs, overflow and out-of-bounds rectangles. Runtime tests cover format refusal, invalid option combinations, source SAR and atomic output preservation.

A fresh test-generated 64x48, two-second, 10 fps source provides development evidence. With the qualified pinned FFmpeg 9.0.2 pair and pinned x264, compare every decoded frame and its timestamp (`framemd5`), frame count, dimensions, pixel format and sample aspect against FFmpeg for `-an -c:v libx264 -f mp4 -vf crop=32:24:0:0 -movflags +faststart -pix_fmt yuv420p`. A second offset, `crop=32:24:16:8`, verifies nonzero coordinates. The fallback log must be empty. Any mismatch is a failure, not grounds to relax the comparator.

Rank 21 of the GitHub shape roster (`1f8b13d8ece40069`) is one registered command on one generated input, weighted by 13 repository-shape pairs. It is not 13 media inputs or 13 independent tests. Development tests never open the retained registered fixture, execute its calls directory or run the registered harness, and make no coverage claim. Historical records and their refused result stay unchanged. A separate registered run and results change follow merge.

No previously native command may move to fallback. The existing legacy encoder comparison, default-path tests and required CI remain gates. C02 retains its committed 1,121,797-instruction baseline and 5% tolerance; its existing CI comparison must pass without changing commands, inputs, generators, judges, baselines or tolerance. Similarity and independent review must identify the final public head. Nothing in this plan authorizes running held-out or judged media before merge.

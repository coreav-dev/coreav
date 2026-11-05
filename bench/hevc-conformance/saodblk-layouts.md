# SAODBLK A/B closed reference-layout investigation

This registration fixed the complete candidate set before another decoded-output hash was computed. It did not change either published digest or admit an alternative reference. The C1 trial retained both misses. The subsequent disposition is narrowly recorded in the [known-exclusion registry](known-exclusions.json): both named streams are excluded by maintainer decision, 2026-11-04. They remain in archive accounting. A miss does not permit another candidate or exclusion to be added.

## Sources and scope

H.265.1 (10/2018) clauses 6.5.2, 6.5.3 and 6.5.5 describe attached reference data, exact sample agreement, Annex C output order and the corresponding hash operation. H.265 (01/2026) clause D.3.20 arranges each coded component in raster sample order, using one byte at depths up to eight and low byte first above eight. D.3.20 defines component SEI hashes, not an external whole-file serialization.

[JCTVC-O1010](https://www.itu.int/wftp3/av-arch/jctvc-site/bitstream_exchange/JCTVC-O1010.zip), section 1.4, requires an MD5 of the complete decoded YUV file, “for entire frames,” and makes checksum SEI optional. It does not specify the exact external packing. [JCTVC-W meeting notes](https://www.itu.int/wftp3/av-arch/jctvc-site/2016_02_W_SanDiego/JCTVC-W_Notes_d7.doc) record the correction of the A/B filenames to `MainConcept_4`. The older official `Divx_3` archives have identical checksum attachments. The checked current and backup listings, H.265.1 edition index, O1004 revision 1, S1004 defect report, and W/X meeting notes supplied no corrected A/B digest. That bounded search does not prove that none exists.

Both acquired streams signal 8-bit 4:2:0, coded and visible geometry 1016×760, and no conformance crop. Each codes and outputs 500 pictures. These facts come from the complete native header receipts, not the filenames. There is no decoded-picture-hash SEI. The existing canonical oracle comparison already disagrees with each published digest; this registration records that prior observation rather than presenting the next comparison as blind.

| Stream | Archive SHA-256 | Checksum attachment SHA-256 | Published whole-output MD5 |
|---|---|---|---|
| SAODBLK_A | `2ca335887ff3262f4a1a2653b5feaa34d2fd313e0f268966ce316549e654e1e0` | `b314dfd9e607f2d5db5fdc65db72a542a945cc0e63c0d153a8b7639df951fc6d` | `b9c98937bcb6be445348fdc89c208f1e` |
| SAODBLK_B | `575165fe6d0a474abdbfb2a628e8c1c2399827fca9f9f47c11aaf71976b51441` | `797a3c21484e2c264d8d5b8a923da8692cc17c186140ab94e743e102854e6418` | `031043495e9b9f4ce120afa316aa5705` |

## Closed candidate set

There is exactly one candidate for each stream:

1. **C1:** MD5 of the complete decoded output in Annex C presentation order. Each picture contains its raster Y plane, then raster Cb, then raster Cr, with one byte per sample and no padding. Each picture is 1,158,240 bytes; the complete file is 579,120,000 bytes. The oracle was the pinned FFmpeg native HEVC decoder, with unaligned cropping enabled and passthrough output timing. The historical trial's exact command, exit status, completed frame count and observed MD5 are retained in each known-exclusion entry's `historical_trial` and copied into later census exclusion records. They are not newly executed comparisons.

This is a packing hypothesis, not a normative claim about undocumented producer packing. It combines the published whole-file scope with the specification's component/raster/sample-byte convention and the signalled format. The completed C1 trial at `30c0009802ce43af0efb0513bb756f832bcbbe88` bound the pinned inputs and driver before the maintainer's exclusion decision. Excluded streams are not decoded or rehashed in later runs.

Decode-order serialization conflicts with H.265.1's required output order. Cropped and uncropped serialization are identical because no crop is signalled. Sixteen-bit serialization has no basis because the signalled depth is unambiguously eight. Per-picture hashes, per-component hashes, and concatenated digest bytes have a different scope from the mandatory complete-YUV-file MD5. Chroma swaps, interleaving, padding, vertical flips and other permutations have no support in the checked specification or attachments. They are not candidates and will not be hashed. No additional layout is justified by the allowed sources.

Every historical C1 result, including a miss, remains in the census evidence. Both trials exited zero, completed progress and produced 500 outputs, but neither matched its published MD5. The retained commands replace only their machine-specific executable and input paths with `{oracle_ffmpeg}` and `{original_bitstream}`. The historical executable SHA-256 and original input digest bind those placeholders. All flags and original progress fields remain unchanged. The trial's generic packing text mentions the high-depth convention, but its actual `pixel_format` and `-pix_fmt` are eight-bit `yuv420p`.

A mismatch remains unresolved: it proves neither a bad published reference nor an oracle defect. It cannot be classified as a harmless diagnostic or replaced by an oracle hash. The historical outputs receive no passing hash or reference-count credit. Only the subsequent named maintainer decision excludes these two streams; it does not authorize another exclusion or a silent gate move. The preregistration at the trial head `30c0009802ce43af0efb0513bb756f832bcbbe88` had SHA-256 `c808cf292008775865b6e1cd3f2bc697c826149d275a80836248b6b98d4794a4`, before the final-disposition text was added. Later edits to this document do not alter that original trial binding.

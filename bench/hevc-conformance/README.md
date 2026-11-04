# HEVC header conformance census

This is milestone 1 of [the HEVC design](../../docs/design/hevc.md). It acquires published data, checks every admitted stream's headers to EOF, and records the fixed 54-file profile/container census. It does not reconstruct pictures with CoreAV, measure speed, or pass later correctness gates.

## Registration

`archives.json` maps all 147 direct ZIP links in the official HEVC_v1 listing to all 147 rows of H.265.1 (10/2018), Table 1, pages 91 through 97. The listing contains 129,093,824 compressed bytes. The largest archive is 9,529,728 bytes. Case differences are recorded. The table's `WP_B_MAIN10_Toshiba_3` maps to `WP_MAIN10_B_Toshiba_3.zip`; wrapped table filenames are joined, including a doubled underscore at the WPP Main10 line break. The mapping does not prove that an archive contains exactly one distinct stream. Acquisition inventories every bitstream and its SHA-256. Duplicate stream hashes remain visible rather than disappearing from the denominator.

The registration pins the listing HTML and the H.265.1 PDF. Archive SHA-256 values are not invented before acquisition. `acquisition.json` records them, and each `inventory.json` records extracted data hashes, published MD5 records, declared sizes, and descriptions. Those receipts must be retained with the census. The full acquisition is required before the full census can pass. A preflight selection never reports complete coverage.

H.265.1 clause 6.5.2 allows archives without decoded pictures or reference hashes and prescribes the reference software to generate them. That software is not an allowed source or oracle for this project. A missing published reference therefore blocks the test. An included published YUV file can supply the reference directly, with its derived MD5 labelled as such. Binary-oracle output is never substituted for missing published data.

`TSUNEQBD_A_MAIN10` is an expected refusal. Clause 6.6.13.3 specifies 10-bit luma and 9-bit chroma, outside the registered equal-depth scope. Extension-data tests are not blanket exclusions. For example, clause 6.6.12.36 requires Main decoders to ignore `EXT_A`'s extension data and decode its three pictures.

## Run

Run the acquisition and full census on Linux CI, not on a personal workstation. The parser example is built by the parent workflow. Install the oracle binaries through the repository's pinned installer first.

```sh
python3 scripts/hevc_conformance.py acquire --dest "$RUNNER_TEMP/hevc-data"
python3 scripts/hevc_conformance.py manifest --data "$RUNNER_TEMP/hevc-data" \
  --out "$RUNNER_TEMP/archives.pinned.json"
python3 scripts/hevc_conformance.py run \
  --registration "$RUNNER_TEMP/archives.pinned.json" \
  --data "$RUNNER_TEMP/hevc-data" --out "$RUNNER_TEMP/hevc-census" \
  --parser "$PWD/target/release/examples/headers" --oracle-bin "$ORACLE_BIN"
```

For a small acquisition preflight, append `--archive IPRED_B_Nokia_3.zip`. It cannot pass the full gate. Do not run the full acquisition or census on a Capy machine.

Retain `archives.pinned.json` as an artifact and copy it into this directory's `archives.json` before completing the PR. The census requires these archive/extracted-data pins, not just the listing's filenames and sizes. Later acquisition checks the pinned archive and extracted hashes. A registration-identity hash binds the original listing/table decisions through this first-acquisition step; adding observed hashes cannot change its membership or scope.

The independent oracle checks every access-unit boundary against the native first-slice offsets, coded geometry against stream metadata, and output count and ordered visible geometry/depth against decoded-frame metadata. Published counts and included reference YUV lengths provide a separate check. The native parser must simulate output decisions, not rename the number of `pic_output_flag` pictures to an output count. `BUMPING_A` has 49 published output pictures in clause 6.6.12.33 despite coding more pictures.

The oracle also hashes its full raw planar output with the MD5 muxer. Agreement with the published whole-YUV-file hash confirms the fixed visible planar packing used for that test. A bitstream `.md5` is a byte-integrity check, not an output reference. A whole-file MD5 is not a per-picture hash or a decoded-picture-hash SEI. This driver does not claim to verify SEI sample hashes before CoreAV reconstruction exists. Unknown hash roles, ambiguous multiple output files, failed hash confirmation and unexplained packing remain explicit blockers. When a published YUV file is included, the driver derives per-picture MD5s from those bytes and identifies G2/G3 intra candidates by depth and slice tools. It checks all independent slices in a picture, not one I-slice flag. These are registered candidates, not proof that a filter or motion tool changes pixels. Missing intra sets remain visible and block completion.

Published hash data include `.md5` files and `*_md5.txt` files. The reader recognizes `HASH [*]filename`, `MD5 (filename) = HASH`, MD5summer comment headers followed by named records, and a single bare 32-digit MD5 with optional line endings. Named records retain distinct bitstream and whole-output-YUV roles. A bare digest in a named bitstream attachment such as `.bit.md5` checks that bitstream. A `.yuv.md5` or `_yuv.md5` attachment names a YUV role. Other bare digests require a single-bitstream H.265.1 clause 6.5.2 package, a matching attachment basename and a short description, or matching archived published YUV bytes. The receipt records that role basis before any oracle comparison. An unrelated or ambiguous bare attachment remains unresolved. Bare digests have no invented target filename or byte length. Each hash file records its digest count, and every text member retains the SHA-256 of its archived raw bytes. A digest count is never a picture count. Multiple bare digests have no declared layout and are refused.

Hash target labels are not filesystem paths. For example, `SAO_H_Parabola_1.md5` names `/cygdrive/d/test.yuv`, but all three ZIP members have safe relative names. The reader retains that producer label without opening it, resolving it against local files, or creating it. Actual unsafe ZIP member paths remain refused, and extracted bitstreams use numeric filenames. AppleDouble `__MACOSX/**/._*` text and hash sidecars retain raw-byte digests as archive metadata, not descriptions or reference records. This classification does not hide bitstream members.

Descriptions and hash text use a BOM or an explicit supported encoding declaration when present, then strict UTF-8. Non-UTF-8 descriptions without a declaration use an explicitly labelled CP1252 fallback, or ISO-8859-1 when CP1252 cannot decode every byte. No replacement characters are inserted. Encoding labels and their detection basis accompany the decoded text, while raw-byte hashes stay unchanged.

Complete acquisition does not establish an independently published output count. A bare output MD5 or a named MD5 without archived YUV bytes supplies no length. Descriptions that count coded pictures do not automatically count output pictures. The table's frame-rate column is not a picture count.

The reference-count priority is an explicitly published output count, fixed-geometry archived YUV length, a registered H.265.1 prose output count, then a hash-validated pinned-oracle count only when none of those sources applies. The last method is not a published count. Before receiving count credit, the complete FFmpeg 9.0.2 native-HEVC decode must match the actual published whole-output MD5 exactly. Its final `-progress` receipt must report completion and the same output-frame count as the complete frame metadata, without duplicated or dropped frames. The native output simulator must independently agree. A partial frame list, wrong hash, missing reference, decode error or incomplete progress fails. The driver never tries alternative packing guesses to obtain a match, substitutes an oracle hash, or excludes a failing stream.

Oracle diagnostics have a closed stream-and-exact-message classifier, not a pattern-based ignore list. Classification requires zero exit, complete output, exact published-MD5 agreement, and matching native and reference counts. Raw stderr lines, transient context addresses, exact messages, stages and proofs remain in the census manifest. An EOS/EOB-only probe packet is not a coded picture; admitting such a packet requires independently checked NAL boundaries and native header records that prove it contains no slice. Unknown lines or streams, nonzero exits, incomplete output and hash disagreements still fail. The [SAODBLK closed layout registration](saodblk-layouts.md) preserves both disagreements rather than classifying them away.

The [synthetic Main10 recipes](synthetic.md) supply additional unfiltered intra references. They use a separate synthetic evidence source, not a fabricated published count or an H.265.1 hash. The fixture verifier regenerates twice, checks committed compressed and raw hashes, compares component hashes with embedded SEI and rejects crossed references. Native header classification happens only after registration. It must find two Main10 intra pictures with both filters off and agree with independent packet/frame metadata before their per-picture references enter the G2 candidate set. This establishes candidates, not reconstructed-sample correctness.

`census.json` retains each stream's chosen `reference_count`, including its method, value and source. `published: false` identifies the hash-validated oracle method. The `reference_count_sources` rows and `reference_count_methods` totals form the generated source audit. Refusals and blocked streams remain in that audit and the archive accounting. For a confirmed one-picture output, the published whole-file MD5 is also that picture's MD5, with its original hash attachment retained as the source. Multi-picture files still require published YUV bytes to derive individual picture hashes. The count fallback does not manufacture picture hashes or tool coverage.

The separate fixed-file census does not decode frames or judge pixels. It reads container and selected-video-stream metadata only. Acquisition is separate from execution so credentials are not present in media subprocess environments.

```sh
python3 scripts/hevc_conformance.py fetch-corpus --dest "$PRIVATE_MEDIA"
python3 scripts/hevc_conformance.py corpus --data "$PRIVATE_MEDIA" \
  --out "$PRIVATE_RESULTS" --oracle-bin "$ORACLE_BIN"
```

On a runner without room for the full roster, use the sequential form instead. It creates its own temporary directory, fetches one file, inspects it with a credential-free subprocess, writes the receipt, and deletes that temporary media before proceeding. It requires free space for the largest file plus 128 MiB, with another capacity check before each download. Existing files outside that temporary directory are not deleted.

```sh
python3 scripts/hevc_conformance.py corpus-online --dest "$RUNNER_TEMP/hevc-corpus" \
  --out "$PRIVATE_RESULTS" --oracle-bin "$ORACLE_BIN"
```

Keep all per-file corpus results and logs private. The fixed roster contains 54 files, 26 development and 28 held-out-partition files, including eight B5 files, totalling 23,305,573,398 bytes. There is no replacement draw, success-based exclusion, or speed run. Metadata inspection alone does not establish profile admission; native active-parameter validation still owns that decision.

## Bounds and data isolation

Each official download has a 16 MiB cap, its registered byte count, a 60-second socket timeout, and a 300-second deadline. ZIPs have at most 512 entries, at most 8 GiB per member, 16 GiB declared expansion per archive, and 64 GiB declared expansion across the census. Bitstreams have a 64 MiB cap and descriptions/hash files a 256 KiB cap. ZIP traversal, links, encrypted members, unsupported compression and duplicate paths are refused. Only bitstreams, YUV reference bytes, short descriptions, MD5 data and bounded text/hash sidecar bytes are read. Other entries, including traces and implementation files, are inventoried by name and size without opening them. Reference YUV is hashed as a stream, not stored. No executable archive member is run.

Each parser or oracle subprocess has a 300-second wall deadline, 300 CPU seconds, a 6 GiB address-space limit, and separate 64 MiB stdout/stderr limits. It receives only PATH, LANG, TMPDIR and LC_ALL. A limit hit is a failure, never an exclusion. The credentialed acquisition has a shared cap of 4,000 read requests and 32 GiB, including manifests. The workflow must retain a timeout and avoid concurrent acquisitions into the same directory.

## Parser example contract

The example accepts one complete Annex B file path. It emits JSON lines and returns zero only after reading to EOF. It is not a default CLI consumer or a decoder fallback.

Every NAL has a `kind: "nal"` record with integer `index`, `offset`, `size`, `nal_type`, `layer_id` and `temporal_id`. Offset names the first NAL-header byte after the start code. Size excludes Annex B trailing zero bytes. Temporal ID is `nuh_temporal_id_plus1 - 1`.

Every VCL record includes boolean `first_slice_segment_in_pic_flag` and `dependent_slice_segment_flag`. A first slice also includes zero-based `picture_index`, `coded_width`, `coded_height`, visible `width`, visible `height`, `bit_depth`, `profile_idc`, and boolean `tiles` and `wpp`. Each independent slice includes `slice_type`, two boolean `sao` flags, and boolean `deblocking_disabled`. The parser retains enough state to validate dependent segments and parameter-set activation. Additional slice/tool metadata can be emitted without changing the required fields.

Every picture released by the output simulator has a `kind: "output"` record with `picture_index`, signed full `poc`, visible `width`, visible `height` and `bit_depth`, in presentation order. The final record is `kind: "summary"`, with `complete: true`, file `bytes`, `nals`, coded `pictures`, true `output_pictures`, and `tools` coverage. Nonzero exit and a final `kind: "refusal"` record with `category: "unsupported"` and `reason: "unequal component depths"` are required for the named out-of-scope test. Other failures must also return nonzero and must not emit a successful completion summary.

Run local unit tests without downloading media:

```sh
python3 -m unittest discover -s scripts/tests -p test_hevc_conformance.py
```

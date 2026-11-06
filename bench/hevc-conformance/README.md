# HEVC conformance checks

Pull requests run the unchanged header tests and a Linux-only comparison of all eight pictures from the four registered synthetic recipes. Full vector and fixed-file census jobs run on main pushes, daily at 03:45 UTC, or by manual dispatch. The [HEVC tier procedure](../../docs/design/ci-checks.md#hevc-tiers) explains exact-head full evidence and failure reporting. The smoke result does not replace G1, G2, G3 or fixed-file census evidence.

The G1 driver of [the HEVC design](../../docs/design/hevc.md) acquires published data, checks every admitted stream's headers to EOF, and records the fixed 54-file profile/container census. The intra driver checks original unfiltered pictures for G2; `--filters` includes filtered pictures for G3 and retains G2 regression results. Neither driver reconstructs inter pictures or measures speed. The historical candidate counts below are not new reconstruction results.

The [complete evidence-source table](evidence-sources.md) records the verified census at `028ca00a6831b64faa226614252edd0290ce642d`: 147 archives, 144 distinct official bitstreams, 144 header/reference checks, two named maintainer exclusions and one named unequal-depth refusal. The 126 hash-validated-oracle count rows represent 123 distinct streams; those counts are not labelled published. The separate four synthetic sources supply eight Main10 G2 candidates. Published candidates supply three 8-bit G2, eleven 8-bit G3 and six Main10 G3 pictures. All four depth/gate sets are nonempty; these are candidates, not reconstruction passes.

The metadata-only census also completed all 54 fixed files. Its public aggregate has 42 Main, 11 Main10 and one RExt profile result with unspecified depth. This is metadata observation, not native-profile admission. Per-file records and container details remain sealed; the public artifact contains only the roster digest, completion flag, aggregate counts and file total.

## Registration

`archives.json` maps all 147 direct ZIP links in the official HEVC_v1 listing to all 147 rows of H.265.1 (10/2018), Table 1, pages 91 through 97. The listing contains 129,093,824 compressed bytes. The largest archive is 9,529,728 bytes. Case differences are recorded. The table's `WP_B_MAIN10_Toshiba_3` maps to `WP_MAIN10_B_Toshiba_3.zip`; wrapped table filenames are joined, including a doubled underscore at the WPP Main10 line break. The mapping does not prove that an archive contains exactly one distinct stream. Acquisition inventories every bitstream and its SHA-256. Duplicate stream hashes remain visible rather than disappearing from the denominator.

The registration pins the listing HTML and the H.265.1 PDF. Archive SHA-256 values are not invented before acquisition. `acquisition.json` records them, and each `inventory.json` records extracted data hashes, published MD5 records, declared sizes, and descriptions. Those receipts must be retained with the census. The full acquisition is required before the full census can pass. A preflight selection never reports complete coverage.

H.265.1 clause 6.5.2 allows archives without decoded pictures or reference hashes and prescribes the reference software to generate them. That software is not an allowed source or oracle for this project. An included published YUV file can supply the reference directly, with its derived MD5 labelled as such. When an archive has only a published whole-output MD5, per-picture oracle hashes are allowed only after the complete pinned-oracle output matches that published MD5 exactly, under the [reference decision of 2026-11-05](../../docs/design/hevc.md). Each derived reference is explicitly oracle-derived and records the original stream identity, coded/output picture indices, anchoring published MD5 and attachment identity. Missing or mismatching whole-output references remain reported blockers, not silently skipped pictures or unanchored oracle substitutions.

`TSUNEQBD_A_MAIN10` is an expected refusal. Clause 6.6.13.3 specifies 10-bit luma and 9-bit chroma, outside the registered equal-depth scope. Extension-data tests are not blanket exclusions. For example, clause 6.6.12.36 requires Main decoders to ignore `EXT_A`'s extension data and decode its three pictures.

## Run

Run the acquisition and full census on Linux CI, not on a personal workstation. The parser example is built by the parent workflow. Install the oracle binaries through the repository's pinned installer first.

```sh
python3 scripts/hevc_conformance.py acquire --dest "$RUNNER_TEMP/hevc-data"
python3 scripts/hevc_conformance.py manifest --data "$RUNNER_TEMP/hevc-data" \
  --out "$RUNNER_TEMP/archives.pinned.json"
python3 -I scripts/hevc_fixture.py verify \
  --ffmpeg "$ORACLE_BIN/ffmpeg" --ffprobe "$ORACLE_BIN/ffprobe" \
  --output "$RUNNER_TEMP/hevc-synthetic"
python3 scripts/hevc_conformance.py run \
  --registration "$RUNNER_TEMP/archives.pinned.json" \
  --data "$RUNNER_TEMP/hevc-data" --out "$RUNNER_TEMP/hevc-census" \
  --parser "$PWD/target/release/examples/headers" --oracle-bin "$ORACLE_BIN" \
  --synthetic-data "$RUNNER_TEMP/hevc-synthetic"
```

For a small acquisition preflight, append `--archive IPRED_B_Nokia_3.zip`. It cannot pass the full gate. Do not run the full acquisition or census on a Capy machine.

### Complete scalar reconstruction

Build the `decode` example with `native-hevc`. After the G1/G3 command below completes, compare every admitted original conformance stream through EOF:

```sh
python3 -I scripts/hevc_decode.py \
  --registration "$RUNNER_TEMP/archives.pinned.json" \
  --census "$RUNNER_TEMP/hevc-intra/g1/census.json" \
  --data "$RUNNER_TEMP/hevc-data" --out "$RUNNER_TEMP/hevc-decode" \
  --parser "$PWD/target/release/examples/headers" \
  --native "$PWD/target/release/examples/decode" --oracle-bin "$ORACLE_BIN" \
  --max-memory-bytes 1073741824 --max-stream-bins 1099511627776 \
  --max-picture-bins 268435456
```

The comparator consumes the complete source-bound census, preserves every archive disposition and requires the published whole-output anchor before comparing individual output pictures. It checks original coded/output identities, geometry, packing, hashes, effective work and budget controls. `--select-test` is diagnostic only and cannot report complete coverage. The `decode` example accepts `INPUT MAX_MEMORY_BYTES MAX_STREAM_BINS MAX_PICTURE_BINS`; `-` reads standard input. Its bounded JSON lines contain each output picture and a final successful-completion summary. Annex B timestamps remain null. This command and its fixture tests are implemented. Original-media C1 and C2 comparisons run in the full tier, not the routine smoke.

### Container packet comparison

The existing Linux-only private census also compares every in-scope fixed-roster input through 120 selected-track packets and every designated EOF case. Build `headers`, `intra` and `packets` with `native-hevc`, then add `--packet-native "$PWD/target/release/examples/packets" --decode-memory-bytes 1073741824 --stream-bins 1099511627776 --picture-bins 268435456` to its `corpus-online` invocation. This reuses the same one-file-at-a-time acquisition and private evidence path; no additional media is registered. Per-file packet/clock/hash records stay private, while the public summary retains only completion and aggregate counts.

CI runs four fixed shards of the same roster. Sorting by descending registered byte size, then SHA-256, and taking indices modulo four assigns each of the 54 identities exactly once. `--shard-count 4 --shard-index N` selects one part; all four checks must pass before claiming roster-wide coverage. Each report labels its `scope`, expected identities and per-part development/EOF requirements. A completed shard is not a completed full roster. Public summaries expose only the scope and aggregate counts; private artifact names carry the shard index.

The `packets` test tool accepts `INPUT CONFIG_ANNEXB STREAM_INDEX PACKET_LIMIT MAX_MEMORY_BYTES MAX_STREAM_BINS MAX_PICTURE_BINS`. Configuration NALs come from an original first-packet copy, not a generated bitstream. Packet limits are 120 or zero for EOF. The caller applies the compatibility video edit-start snap before rounding presentation ticks; the codec preserves them through reordering. Private records retain original edit entries, timescales and the derived clock offset. Native packet identities, oracle packet timestamps, the exact demux time base, per-frame geometry/packing and sample hashes must all agree. After prefix drain, only frames below the minimum timestamp of every unsent packet are compared. A bound hit, failed clock mapping or missing EOF arm fails rather than becoming an exclusion.

The test adapter reports terminal zero padding while matching Annex B's NAL framing; it does not change stored/passed-packet hashes, complete CABAC zero words or entropy validation. The original NAL size is checked before trimming. Full-layer temporal-nesting NAL-type violations and non-coprime Extended_SAR declarations produce typed warnings rather than refusing otherwise decodable pictures; invalid SAR becomes unspecified. Supported ten-bit samples with a Main10 declaration remain admitted when an additional eight-bit-only compatibility claim is inconsistent; `profile_bit_depth` reports the contradiction. Ten-bit samples without a Main10 claim, unsupported depths, safety, reference and entropy failures remain strict refusals.

Milestone 4 requires complete C1, unchanged G3 and exact equality for every completed C2 comparison. Explicit container/tooling blockers outside the decoder remain follow-ups with no pixel credit, preserving all 54 source identities. Decoder failures and pixel mismatches still block. The summary distinguishes `passed` under this acceptance rule from `complete` coverage of all requested comparisons, and publishes only aggregate follow-up reasons. The full-tier verifier requires the current-head dispatch rather than a routine smoke or an earlier revision's success.

### Selected intra reconstruction

Build both `headers` and `intra` examples with `native-hevc`. After the same acquisition, hash binding and synthetic verification above, run G1 with G2/G3 comparisons:

```sh
python3 -I scripts/hevc_intra.py \
  --registration "$RUNNER_TEMP/archives.pinned.json" \
  --data "$RUNNER_TEMP/hevc-data" --out "$RUNNER_TEMP/hevc-intra" \
  --synthetic-data "$RUNNER_TEMP/hevc-synthetic" \
  --parser "$PWD/target/release/examples/headers" \
  --native "$PWD/target/release/examples/intra" --oracle-bin "$ORACLE_BIN" \
  --max-memory-bytes 268435456 --max-bins 268435456 --filters
```

This calls the existing G1 owner once into `hevc-intra/g1`; do not also run the standalone G1 command for the same work. It derives eligible original coded-picture indices from every fresh header record, retaining archive aliases and the four synthetic recipes. An eligible picture missing an output/reference mapping fails instead of disappearing from the denominator. The two authorized non-output OPFLAG_B pictures instead use their original SEI as `coded-picture reference (SEI), not output`, without invented indices. The complete source-bound result is `hevc-intra/result.json`, retained with G1 receipts in the workflow's `hevc-intra` artifact. The labelled gap `unproved at 10-bit: tiles/dependent segments, no official candidate` does not block milestone 3; no replacement bitstreams are generated.

The `intra` example takes `INPUT TARGET_INDEX NEW_OUTPUT_U16LE MAX_MEMORY_BYTES MAX_BINS`. It creates but never overwrites its output, packs coded Y/Cb/Cr as u16le at both depths, and emits one complete JSON record only after header EOF, ordered filtering, hash verification and output flush. Failure removes only its own newly created output. Streaming input is bounded by four GiB overall and the existing per-NAL limit; caller-owned input capacity is reported separately from reconstruction memory. The driver independently checks sample depth, original crop offsets, component geometry and byte count row by row before packing visible 8-bit samples to bytes or retaining 10-bit little-endian pairs. Full coded-component hashes and cropped-picture hashes remain distinct.

The oracle decodes the complete stream to bounded per-frame MD5 output, with exact frame counts, packed sizes, original output-reference order and complete progress. It is not a selected-frame shortcut. Nonzero exit, incomplete output or a reference mismatch fails; diagnostic wording does not. The original NAL sequence always reaches native reconstruction, with only the existing registered oracle-only parameter preparation permitted.

Runtime controls require absent-target, one-byte entropy initialization, zero-memory and zero-bin failures with cleanup, plus preserved no-clobber output. Decoded mode/transform/QP/PCM counters and actual memory/bin use accompany every picture. Tile effects count real boundaries after the initial tile, only for tiled candidates; WPP and dependent effects likewise count only their own candidates. Missing effective reference cases remain explicit coverage gaps, not a claim of serial-feature pixel conformance. Unit/control tests are separate from this media result.

Retain `archives.pinned.json` as an artifact and copy it into this directory's `archives.json` before completing the PR. The census requires these archive/extracted-data pins, not just the listing's filenames and sizes. Later acquisition checks the pinned archive and extracted hashes. A registration-identity hash binds the original listing/table decisions through this first-acquisition step; adding observed hashes cannot change its membership or scope.

The independent oracle checks every access-unit boundary against the native first-slice offsets, coded geometry against stream metadata, and output count and ordered visible geometry/depth against decoded-frame metadata. Published counts and included reference YUV lengths provide a separate check. The native parser must simulate output decisions, not rename the number of `pic_output_flag` pictures to an output count. `BUMPING_A` has 49 published output pictures in clause 6.6.12.33 despite coding more pictures.

Shared census oracle decodes use `-export_side_data +enhancements` to export optional picture-enhancement metadata without applying another decoder's enhancement to the HEVC base reconstruction. Original geometry, planar samples and published whole-output MD5 checks remain mandatory. This does not alter the input bitstream, filtering syntax, reference hashes or candidate denominator.

The oracle also hashes its full raw planar output with the MD5 muxer. Agreement with the published whole-YUV-file hash confirms the fixed visible planar packing used for that test. A bitstream `.md5` is a byte-integrity check, not an output reference. A whole-file MD5 is not a per-picture hash or a decoded-picture-hash SEI. This driver does not claim to verify SEI sample hashes before CoreAV reconstruction exists. Unknown hash roles, ambiguous multiple output files, failed hash confirmation and unexplained packing remain explicit blockers. When a published YUV file is included, the driver derives per-picture MD5s from those bytes and identifies G2/G3 intra candidates by depth and slice tools. It checks all independent slices in a picture, not one I-slice flag. These are registered candidates, not proof that a filter or motion tool changes pixels. Missing intra sets remain visible and block completion.

Published hash data include `.md5` files and `*_md5.txt` files. The reader recognizes `HASH [*]filename`, `MD5 (filename) = HASH`, MD5summer comment headers followed by named records, and a single bare 32-digit MD5 with optional line endings. Named records retain distinct bitstream and whole-output-YUV roles. A bare digest in a named bitstream attachment such as `.bit.md5` checks that bitstream. A `.yuv.md5` or `_yuv.md5` attachment names a YUV role. Other bare digests require a single-bitstream H.265.1 clause 6.5.2 package, a matching attachment basename and a short description, or matching archived published YUV bytes. The receipt records that role basis before any oracle comparison. An unrelated or ambiguous bare attachment remains unresolved. Bare digests have no invented target filename or byte length. Each hash file records its digest count, and every text member retains the SHA-256 of its archived raw bytes. A digest count is never a picture count. Multiple bare digests have no declared layout and are refused.

Hash target labels are not filesystem paths. For example, `SAO_H_Parabola_1.md5` names `/cygdrive/d/test.yuv`, but all three ZIP members have safe relative names. The reader retains that producer label without opening it, resolving it against local files, or creating it. Actual unsafe ZIP member paths remain refused, and extracted bitstreams use numeric filenames. AppleDouble `__MACOSX/**/._*` text and hash sidecars retain raw-byte digests as archive metadata, not descriptions or reference records. This classification does not hide bitstream members.

Descriptions and hash text use a BOM or an explicit supported encoding declaration when present, then strict UTF-8. Non-UTF-8 descriptions without a declaration use an explicitly labelled CP1252 fallback, or ISO-8859-1 when CP1252 cannot decode every byte. No replacement characters are inserted. Encoding labels and their detection basis accompany the decoded text, while raw-byte hashes stay unchanged.

Complete acquisition does not establish an independently published output count. A bare output MD5 or a named MD5 without archived YUV bytes supplies no length. Descriptions that count coded pictures do not automatically count output pictures. The table's frame-rate column is not a picture count.

The reference-count priority is an explicitly published output count, fixed-geometry archived YUV length, a registered H.265.1 prose output count, then a hash-validated pinned-oracle count only when none of those sources applies. The last method is not a published count. Before receiving count credit, the complete FFmpeg 9.0.2 native-HEVC decode must match the actual published whole-output MD5 exactly. Its final `-progress` receipt must report completion and the same output-frame count as the complete frame metadata, without duplicated or dropped frames. The native output simulator must independently agree. A partial frame list, wrong hash, missing reference, decode error or incomplete progress fails every other stream. The driver never tries alternative packing guesses to obtain a match or substitutes an oracle hash. Only the two SHA-bound entries in [known-exclusions.json](known-exclusions.json) are excluded by maintainer decision, 2026-11-04; their digests, exhausted investigation and actual MD5 disagreements remain recorded. Both retain complete native-header receipts and remain in the 147-archive accounting without reference-count/hash credit. Their `exclusion.historical_trial` retains the original C1 command, executable/input identities, exit, progress, counts and mismatching hashes. Only executable and input paths are replaced with explicit placeholders. Later runs validate and copy that historical evidence without decoding or hashing these excluded streams again. No other stream may use this exception.

Diagnostic wording never gates verified output. The [oracle-error rule](../../docs/design/hevc.md) requires zero exit, complete output, exact published-MD5 agreement, and matching native and reference counts. Raw stderr lines, transient context addresses, exact messages, stages and verification proofs remain in the census manifest, including unrecognised messages. An EOS/EOB-only probe packet is not a coded picture; admitting such a packet independently requires checked NAL boundaries and native header records that prove it contains no slice, regardless of stderr. Nonzero exits, incomplete output and hash disagreements still fail. The [SAODBLK closed layout registration](saodblk-layouts.md) preserves both disagreements rather than classifying them away.

The [synthetic Main10 recipes](synthetic.md) supply additional unfiltered intra references. They use a separate synthetic evidence source, not a fabricated published count or an H.265.1 hash. The fixture verifier regenerates twice, checks committed compressed and raw hashes, compares component hashes with embedded SEI and rejects crossed references. Native header classification happens only after registration. It must find two Main10 intra pictures with both filters off and agree with independent packet/frame metadata before their per-picture references enter the G2 candidate set. This establishes candidates, not reconstructed-sample correctness.

[VPSSPSPPS_A's registered oracle preparation](oracle-preparation.md) stably orders only its leading VPS/SPS/PPS units. Every NAL byte, within-type order, VCL position and following byte remains unchanged; CoreAV always reads the original. Receipts and diagnostics retain both input digests. Its reference count is the published description's six pictures, and prepared output must match the published MD5 exactly. The original oracle's incomplete two-picture output is not accepted as harmless. For changing-resolution streams, stream-level coded geometry must match an activated format; each output's visible geometry and depth are compared independently in order.

`census.json` retains each stream's chosen `reference_count`, including its method, value and source. `published: false` identifies the hash-validated oracle method. The `reference_count_sources` rows and `reference_count_methods` totals form the generated source audit. Refusals and blocked streams remain in that audit and the archive accounting. For a confirmed one-picture output, the published whole-file MD5 is also that picture's MD5, with its original hash attachment retained as the source. A multi-picture stream with eligible intra pictures and no published YUV receives complete per-frame oracle hashes only after whole-output confirmation. These carry `source_scope: oracle_picture_anchored_to_published_whole_output`, `stream`, `stream_sha256`, `picture_index`, `output_index` and `anchoring_md5`, together with the published attachment and oracle identities. The count fallback alone does not manufacture picture hashes or tool coverage. New eligible pictures join the appropriate G2/G3 denominator without changing the historical source audit.

The separate fixed-file census defaults to container and selected-video-stream metadata only. With both `--native` and `--parser`, `corpus-online` also packet-copies and validates the first independent IRAP of each of the 26 development files, then compares native pixels with complete pinned-oracle output of that picture. The other 28 files remain metadata-only. This is not full-file decoding or a speed comparison. Acquisition remains outside credential-free media subprocesses.

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

Each parser or oracle subprocess has a 300-second wall deadline, 300 CPU seconds, a 6 GiB address-space limit, and separate 64 MiB stdout/stderr limits. Native C2 prefixes have 1,800 CPU/wall seconds and native EOF arms have 3,600, and each boundary decode of the same arm has the same bound; each shard has a 360-minute job bound, the hosted-runner maximum, because every arm is decoded once by the scalar example and once through the backend. These are operational limits, not speed claims. Each child receives only PATH, LANG, TMPDIR and LC_ALL. A limit hit is a failure, never a container follow-up. Each shard's credentialed acquisition has a cap of 4,000 read requests and 32 GiB, including manifests, with at most three attempts for dropped immutable reads against that same cap. Shards use separate runners and directories.

## Parser example contract

The example accepts one complete Annex B file path. It emits JSON lines and returns zero only after reading to EOF. It is not a default CLI consumer or a decoder fallback.

Every NAL has a `kind: "nal"` record with integer `index`, `offset`, `size`, `nal_type`, `layer_id` and `temporal_id`. Offset names the first NAL-header byte after the start code. Size excludes Annex B trailing zero bytes. Temporal ID is `nuh_temporal_id_plus1 - 1`.

Every VCL record includes boolean `first_slice_segment_in_pic_flag` and `dependent_slice_segment_flag`, and zero-based `picture_index`. All slices belonging to one coded picture carry that picture's index. A first slice also includes `coded_width`, `coded_height`, visible `width`, visible `height`, `bit_depth`, `profile_idc`, `ctb_log2`, the four `conformance_window` offsets in chroma samples, and boolean `tiles` and `wpp`. Each independent slice includes `slice_type`, effective `slice_qp_y` and `chroma_qp_offsets`, two boolean `sao` flags, and boolean `deblocking_disabled`. The parser retains enough state to validate dependent segments and parameter-set activation. Additional slice/tool metadata can be emitted without changing the required fields.

Every picture released by the output simulator has a `kind: "output"` record with `picture_index`, signed full `poc`, visible `width`, visible `height` and `bit_depth`, in presentation order. The final record is `kind: "summary"`, with `complete: true`, file `bytes`, `nals`, coded `pictures`, true `output_pictures`, and `tools` coverage. Nonzero exit and a final `kind: "refusal"` record with `category: "unsupported"` and `reason: "unequal component depths"` are required for the named out-of-scope test. Other failures must also return nonzero and must not emit a successful completion summary.

Run local unit tests without downloading media:

```sh
python3 -m unittest discover -s scripts/tests -p test_hevc_conformance.py
```

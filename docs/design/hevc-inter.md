# HEVC scalar inter reconstruction

Status: milestone 4 implementation in progress, following the first-commit plan in the same pull request. Scalar inter reconstruction and the C1/C2 comparators have local unit coverage; complete original-source C1/C2 execution and final-head CI are not yet complete. Milestone 3 is merged in coreav-dev/coreav#414. Its final proof includes 456 original and separately registered synthetic intra targets, all 371 applicable defined picture hashes, 740 runtime controls and 26 development first-IRAP comparisons. This plan extends the [HEVC plan](hevc.md), [intra reconstruction](hevc-intra.md) and [in-loop filters](hevc-inloop.md).

## Outcome and authority

Decode complete supported Main/Main10 streams with I, P and B slices, not just independently decodable selected intra pictures. Reconstruct references before using them, apply deblocking and SAO before reference publication, verify applicable coded-component hashes, and emit pictures in the existing DPB's presentation order. Retain matching 8-bit or 10-bit 4:2:0 components, original crop and VUI declarations, and terminal errors without concealment or fallback.

Work only from the pinned ITU-T H.265 V11 January 2026 specification and original H.265.1 conformance data. The PDF SHA-256 is `78b093480e178a2b99d8cb7499934240f359c50c6d09568f8be651bc1151d4d2`. Relevant processes are clauses 7.3.6, 7.3.8.4 through 7.3.8.11, 8.3, 8.5, 8.6, 8.7, 9.3 and Annexes C and D. FFmpeg/ffprobe 9.0.2 are black-box binaries only. Do not read HM, another HEVC implementation, implementation test lists, source-derived notes or web-search results that could contain source.

The `native-hevc` feature remains opt-in. Production backend/consumer wiring, default builds and dependencies, encoder work, SIMD, unsafe code, parallel scheduling and timing are outside this milestone. The existing two source exclusions and named profile/depth refusals are unchanged. No new fixture-generation, benchmark registration, freeze or audit machinery is introduced.

## Caller and ownership

The complete-stream facade accepts original complete NALs and an optional caller-supplied presentation timestamp. The caller owns that timestamp's clock and container edit mapping; the codec binds it to a coded picture and preserves it through reordering. Annex B callers may provide no timestamp. The existing selected-intra facade keeps its behavior and uses the same reconstruction owner.

```rust
let mut decoder = PictureDecoder::new(limits);
decoder.read_nal(nal, pts, |picture| sink.picture(picture))?;
let summary = decoder.finish(|picture| sink.picture(picture))?;
```

`DecodeLimits` contains header limits, simultaneous live-memory capacity, a stream-wide CABAC budget and a per-picture CABAC budget. `DecodedPicture<'a>` borrows complete coded planes and carries the original coded index, POC, optional caller timestamp, activated `PictureInfo` and effective work counters. Callbacks cannot retain mutable reference pictures. Successful callbacks are not revoked by a later failure, but successful stream completion always requires `finish`.

Keep `StreamCore` as the only parameter activation, picture identity, inheritance and POC owner, and `Dpb` as the only reference marking, reference-set resolution, bumping and prior-output owner. Extend its fixed sixteen-entry storage with optional owned reconstructed pictures. Header inspection still allocates no samples. Generated unavailable-reference metadata is not fabricated pixel data; a prediction requiring unavailable samples fails.

Full reconstruction distinguishes a coded picture being assembled from a complete filtered picture and a picture eligible for output after its suffix-hash boundary. The shared core owns these transitions. In reconstruction mode it defers the current picture's eager header-only bump until samples and applicable suffix hashes are complete; inspection keeps its existing metadata-only behavior. Before the next first slice, EOS, EOB or final drain can release a picture, its pending finalization is completed. A callback, filter, hash, allocation or syntax failure closes the session and releases its sample state.

Move the existing `Frame`, allocation account and CU/TU traversal into one private reconstruction module rather than copying the intra walker into another decoder. Both facades use that walker, its tile layout, WPP snapshot, dependent restoration, QP map, residual decoder and filter finalization. A separate inter module derives partition motion from the shared state; it does not own another parameter parser or output queue.

Two alternatives were rejected. An independent inter decoder with its own reference pool would duplicate the existing POC/DPB semantics. Per-picture reference-counted heap objects would add allocation and lifetime paths where fixed DPB ownership plus borrowed callbacks suffice. One DPB-owned lifetime and fallibly allocated buffers keep release and memory accounting explicit.

## Retained syntax and motion state

Retain currently discarded slice reference-list syntax, active counts, list modifications, temporal-MVP/collocated selection, `mvd_l1_zero_flag`, merge-candidate limit and weighted-prediction parameters. Dependent segments inherit effective independent syntax through `StreamCore`; they do not reparse or invent defaults. Build reference lists from the existing DPB's resolved short-before, short-after and long-term sets, retaining list ordering and picture identities.

Initialize all CABAC models from the normative I/P/B initialization type, including `cabac_init_flag`, rather than adding only inter-specific models to an I-only snapshot. Tile, independent-slice and WPP transitions copy or reset the same complete context set. Preserve the single picture/stream budget across regular, bypass, termination, PCM restart and entry-point operations.

Decode skip and prediction mode, every admitted symmetric/asymmetric partition, merge indices, inter direction, reference indices, motion-vector differences, AMVP selection, root coded-block flags and inter transform trees. Intra CUs inside inter slices use the existing intra process. Residual scan/context decisions distinguish inter prediction from a fabricated intra mode. Preserve PCM, scaling lists, transform skip and transquant bypass where the profile allows them.

Store resolved motion at the required minimum prediction granularity. Each retained picture has a bounded reference-identity catalog; its motion cells refer to that immutable catalog, not a recycled DPB slot. The catalog keeps POC/long-term facts needed after an old referenced buffer is released. Spatial candidates use already decoded neighbours and the normative partition, slice, tile and parallel-merge restrictions. Temporal candidates use the collocated picture's retained field, reference identity, availability and distance scaling. All reference lookups are checked; no missing reference is replaced by zero samples.

## Scalar numerical work and filtering

Add allocation-free safe kernels for reference-edge extension, luma quarter-sample eight-tap interpolation, chroma eighth-sample four-tap interpolation, unweighted uni/bi prediction and explicit weighted prediction at both depths. Preserve the normative internal precision, offsets, signed shifts, rounding and final clipping; do not clip an intermediate predictor as if it were a final sample. Checked source/destination geometry and bounded scratch prevent partial writes on rejected public inputs.

Keep syntax and candidate derivation out of the kernel crate. The bitstream owner supplies resolved motion, reference views and effective weights. Cover every phase, integer/one-axis/two-axis paths, rectangular partitions, independent strides, picture edges, negative motion, extreme legal samples and weights with independently equation-derived expectations.

Extend the existing boundary metadata with prediction mode, residual presence and resolved reference/motion facts. Derive boundary strength zero, one or two under clause 8.7.2.4, including equivalent list permutations for bi-prediction and the normative motion threshold. Chroma eligibility and luma tC derivation consume the actual strength; they do not assume every edge is intra. Retain CU-local QpY, PCM/bypass protection and slice/tile permissions. Vertical deblocking, horizontal deblocking and charged-source SAO still finalize exactly once before a picture becomes a usable reference.

## Bounds and verification tools

Charge the complete working frame, all retained reference/output planes and motion fields, catalogs, context/substream storage, parameter state, numerical scratch, SAO source copy and allocation overlap before fallible reservation. A released DPB entry releases its owned buffers. Caller-owned input remains separate. Budgets cannot silently evict a needed reference or remove pending output. A stream has both a picture-count limit and an enforced total entropy-work limit, not only a per-picture reset.

Add a streaming complete-decode example and one full-stream comparison path using the existing acquisition, oracle, hashing, privacy and child-process owners. Hash visible output row by row and coded components independently without retaining a whole stream of raw pictures. Keep original coded indices, output order, counts, timestamps, sample packing and reference provenance visible in bounded records. Incomplete streams and partial diagnostic output cannot pass.

C2's test adapter uses the existing container/packet owners to supply original selected-track packets and presentation timestamps; it is not a production backend or CLI registration. Independently reconcile the container clock/edit mapping and oracle time base. The first-120-packet comparison uses the approved unsent-packet timestamp cutoff after drain, not a 120-frame shortcut. The eight designated B5 inputs also run to EOF. Preserve all 54 roster identities and partitions; profile/scope refusals remain explicit and do not follow a decoder mismatch.

Media acquisition, hostile inputs and oracle execution stay on Actions. Local work is limited to builds, bounded self-written unit tests and metadata inspection. Keep the shared HEVC-base oracle option that exports optional enhancement metadata without applying another decoder, and the prescribed explicit shared audio arguments. Private per-file diagnostics remain private; the existing controlled HEVC recipient and separately retained private key are reused. No media or private key is committed.

Shard long correctness work by fixed source identity when needed, retaining one complete denominator and aggregate failure rule. Give every process finite memory, work, capture and wall-time bounds; a bound hit remains a failure, not an exclusion. Do not open a judged speed campaign or introduce new media. Record automatic acquisition caps, including cancelled or incomplete attempts, within the existing infrastructure limit.

## Completion evidence

The first implementation-head run preserves all 456 G3 targets and matches 142 of 144 complete C1 streams. The other two streams match their published whole-output MD5s but expose an accounting error: each contains seven suppressed RASL pictures with suffix hashes and no reconstructed samples. H.265 clause 3.126 describes why such pictures need not be correctly decodable. The shared DPB now exposes target-decoding membership, and the complete summary separately counts their defined and reserved hash messages without checked-sample credit. The comparator requires exact original-index dispositions and the partition of every declared message into checked or unreconstructed counts. It still requires every applicable reconstructed-picture hash, including hashes of decoded non-output pictures; no input, hash, pixel comparison or source denominator is removed. A fresh media run remains required.

The C2 test adapter reuses the existing packet/container owners, supplies original packet NALs and retains caller timestamps through the codec. MP4 timestamps use the packet owner's edit-mapped presentation ticks rounded to nearest with halves away from zero; Matroska uses Segment Ticks. The comparator independently requires equality with the oracle's packet timestamps and demux time base before comparing samples. It scans all unsent metadata for the prefix cutoff. Packet metadata are capped at one million entries and payloads at 64 MiB; oracle prefix output is capped at 152 pictures independently of native output and fails if that bound precedes the cutoff. EOF output is capped at the original packet count plus one, with one coded picture per submitted packet checked by the adapter. Prefix geometry probing stops after the greatest original presentation/decode time of the first 184 packets plus one second, rounded up to a microsecond; every oracle frame below the cutoff still needs matching geometry. Child processes retain 6-GiB-address-space and 64-MiB-capture limits; the native EOF arm has a 1,800-second CPU/wall limit and other calls retain 300 seconds; the combined private census has a 180-minute job bound. No C2 media result is yet claimed.

1. Preserve all G1 accounting, all 456 G3 targets including 17 G2 targets, defined hashes, source references, existing exclusions and negative controls.
2. Complete C1 to original EOF for every admitted conformance stream at both depths. Compare complete published-output layouts and every applicable coded-component MD5/CRC/checksum. Non-output pictures remain counted and hash-checked where defined; reserved hash types get no checked credit.
3. Complete C2 for every in-scope member of the fixed roster by its existing packet-prefix/EOF and clock rules, without dropping failed files or substituting references.
4. Report effective inter motion, P/B/bi prediction, merge/AMVP, temporal prediction, weights, inter boundary strengths, reordering, long-term references, random access and serial-tool coverage by depth. A header flag alone is not effective pixel/hash proof. A missing original case remains explicit and needs a decision before any changed coverage claim; milestone 3's absent ten-bit intra exception is not a blanket G4 exception.
5. Test unavailable references, invalid candidate indices/partitions, truncated entropy, budget exhaustion, reference/output lifetimes, suffix failure before publication, callback failure, EOS/EOB/drain, IRAP prior-output policy and failed-session re-entry. Keep earlier tests intact.
6. Run formatting, strict affected Clippy, affected Rust/script tests, feature-disabled/default-consumer dependency isolation, affected fuzz-target compilation and full final-head CI. Report green CI to the Captain for a fresh independent review; the Captain owns the final similarity guard and queue.

Implementation proceeds in the same PR: retained syntax and shared picture/DPB ownership, numerical kernels and equation tests, inter entropy/motion/residual integration, boundary-strength integration, then complete original-source drivers and failure investigation. No intermediate refusal or unit-only pass is reported as milestone completion.

# HEVC scalar intra reconstruction

Status: milestone 2 is merged in coreav-dev/coreav#393. Its complete G2 result checked all 17 eligible pictures, 17 defined picture hashes and 50 negative/no-clobber controls. This note describes its original unfiltered scalar intra scope; [milestone 3](hevc-inloop.md) extends the same owner with in-loop filters. No production consumer selects it by default.

## Outcome and limits

Reconstruct independently decodable Main intra pictures at 8 bits and Main10 intra pictures at 8 and 10 bits, with deblocking and SAO disabled by the original syntax. Implement scalar CABAC, coding and transform trees, intra prediction, inverse quantization and transforms, scaling lists, transform skip, transquant bypass and PCM. Serial tile, WPP and dependent-segment semantics belong here even though parallel scheduling does not.

Inter prediction, in-loop filters, a `coreav-decode` backend, CLI selection, consumer wiring, default-feature changes, SIMD, unsafe code, threads, an encoder and timed runs are outside this work. Filtered pictures remain unsupported for reconstruction until milestone 3. Never change an official stream's filter flags or compare an intermediate native picture to a filtered reference.

Use the already pinned ITU-T H.265 V11 January 2026 specification and H.265.1 data. HM and every other HEVC implementation's source, test lists and source-derived notes remain unread. FFmpeg/ffprobe 9.0.2 remain binary oracles; no library is linked. The four existing synthetic recipes retain their exact registration and labels. This plan does not authorize a new encoder recipe or a source-study exception.

## Shared stream state

The private `StreamCore` owns parameter activation, full-content identities, dependent-header inheritance, picture boundaries, POC and the header-only DPB. `HeaderParser` delegates to it and reads admitted headers to EOF without reconstructing samples. Its public `NalInfo::Slice` remains an inspection record, not a public decoding configuration.

The SPS/PPS readers retain validated transform bounds, smoothing and PCM declarations, scaling matrices, transform-skip and bypass controls and QP state. Selected slices retain every entry-point length after checking its count and storage budget. Header-only inspection does not allocate entry-point vectors. Re-reading headers in a separate decoder would create conflicting activation, inheritance and lifetime rules.

`IntraPictureDecoder` consumes the same core's validated events. Each NAL has one RBSP/header parse and one parameter-activation decision. Effective inherited syntax and immutable active parameter views reach reconstruction through a borrowed private record, valid for that call. One fixed tile-layout owner serves header address ordering and reconstruction traversal. There is no public parameter-set constructor, second POC implementation or decoder-owned parameter registry.

## Caller contract

The immediate caller is an untimed conformance tool, not a production backend. It requests one original coded-picture index and feeds the original registered NAL sequence without rewriting or concatenating media:

```rust,ignore
let mut decoder = IntraPictureDecoder::new(limits, target_picture_index);
for nal in original_nals {
    decoder.read_nal(nal?, |picture| compare_registered_picture(picture))?;
}
decoder.finish(|picture| compare_registered_picture(picture))?;
```

`IntraLimits` contains the existing header limits plus total reconstruction-memory and picture-wide CABAC-bin budgets. The callback receives an `IntraPicture` borrowing completed coded `u16` Y/Cb/Cr planes, with the original coded index, activated geometry/depth, conformance window and effective counters. It cannot retain the planes beyond the callback without its own allocation. There is no display clock, packet adapter or fallback in this API.

Unselected pictures receive header inspection only, so a target after an inter picture can be located without claiming to reconstruct that inter prefix. The selected picture must have only intra slices, need no earlier samples and have both filters disabled. A selected inter or filtered picture returns `Unsupported`, not a placeholder image. Target absence, missing segments or incomplete termination cannot report success. Header-only prefix validation is never counted as reconstructed-picture coverage or complete C1 decoding.

The target is reconstructed across all its original slice segments. Publish it only after its syntax, required suffix hashes and completion have been checked. A next-picture boundary or EOF can close it; EOF without a complete selected picture fails. The callback is coded-picture evidence, not a new presentation-order output simulator. The existing census maps the original coded index to its registered output reference. Keep cropped-output hashes separate from full coded-component SEI hashes.

Any error, limit hit or callback failure makes the session terminal and discards unpublished samples. No later drain or retry may expose a partial picture. A failure after an earlier callback does not revoke bytes already delivered, but it must make the overall verification fail.

## Ownership and module boundaries

| Owner | State and responsibility |
|---|---|
| Stream core in `coreav-hevc` | Existing NAL/RBSP/header processing, activated parameter identities, inherited slice syntax, picture identity and the single header-only DPB |
| `cabac` in `coreav-hevc` | Bounded arithmetic byte source, range/offset, context transitions, bypass, termination and HEVC-owned initialization tables from clause 9.3 |
| `intra` in `coreav-hevc` | Selected-picture admission, CTU traversal, syntax/context derivation, coefficient decoding, QP state, neighbour availability and publication |
| `coreav-kernels-hevc` | Safe scalar intra prediction, inverse scaling/transform, transform skip and residual addition, with checked block/depth domains and no bitstream or allocation policy |
| Conformance driver | Original source/hash binding, selected-picture/reference mapping, packing, comparisons, coverage and bounded subprocess receipts |

Introduce the kernel crate only with its first real reconstruction caller. It has no dependency on `coreav-hevc`, H.264 or a framework. The optional dependency is activated only by `coreav-hevc/native-hevc`; neither crate enters default consumer graphs. Public kernel inputs use checked depth/block/mode domains; all stored samples fit `u16`, including 8-bit pictures. No architecture dispatch or speculative backend crate is added.

For the design comparison, a separate pixel decoder with its own SPS/PPS tables was rejected because it duplicates activation and temporal invariants. Adding reconstruction switches directly to the public header inspector was also rejected because it changes that API's meaning and makes inspection allocate pixel buffers. The private shared core keeps one semantic owner while the two facades expose distinct, honest capabilities.

## Entropy and serial traversal

1. Decode regular, bypass and termination bins directly from clauses 9.3.2 through 9.3.4. Context initialization, QP clipping, MPS/LPS transitions and renormalization use the specification's tables and equations, not another decoder's constants. Truncation is an error; never synthesize zero bytes past a substream boundary.
2. Derive raster/tile-scan maps from the active PPS and coded dimensions. Use one CTU traversal owner for slice addresses, tile membership, WPP rows and neighbour availability. Validate tile sizes, entry-point counts and exact subset coverage before decoding their payloads.
3. Preserve the distinction between NAL byte offsets and unescaped RBSP offsets. Clause 7.4.7.1 counts emulation-prevention bytes in entry-point subset lengths. Retain the individual offsets and a checked mapping while removing emulation prevention; do not slice RBSP directly with the signalled lengths. Reject overflow, overlapping or out-of-range subsets and malformed alignment/trailing data.
4. Apply independent-slice initialization, tile resets, dependent-segment restoration and WPP storage/synchronization as separate transitions prescribed by clauses 9.3.2.1, 9.3.2.4 and 9.3.2.5. A WPP context snapshot is not a copy of the previous row's arithmetic range/offset. Handle narrow rows, slice starts inside rows and tile edges by the normative availability conditions, rather than unconditional second-CTU copying.
5. Derive intra-neighbour availability from coding order, slice/tile boundaries and constrained-intra rules, not merely whether a plane location has been written. Dependent segments retain the appropriate picture and entropy state without inventing an independent header. PCM alignment/sample reads and arithmetic restart follow their own specified transition.

The scalar path implements serial semantics; it does not add task queues or frame concurrency. A future scheduler must consume these same transition rules, not replace them.

## Reconstruction and bounds

Follow syntax in clauses 7.3.8 and 7.4.9, intra prediction in 8.4.4, quantization/scaling/transforms in 8.6.1 through 8.6.5, and picture construction in 8.6.7. Implement planar, DC and angular prediction, reference substitution/filtering, chroma derivation, coefficient scans, sign hiding, level/Rice decoding, QP derivation, inverse scaling, the specified integer transforms and clipping. Keep normative transform skip, transquant bypass and PCM distinct; none is a shortcut for an unimplemented transform.

Retain custom and default scaling-list values instead of accepting their flags and discarding their content. Derive syntax presence from active block/depth/tool state. Reject inter syntax in the selected picture before accessing a reference plane. Do not silently treat an absent neighbour as available or replace a missing reference with zeros.

Use checked dimensions and stride/plane-size arithmetic, fallible reservations and wide intermediate arithmetic with the specification's intermediate clipping and rounding. Charge the total simultaneous memory before reservation: reconstructed planes, availability maps, CTU metadata, coefficient/transform scratch, parameter payloads, substream maps and any allocation growth overlap. Stride padding is not reference output. The conformance writer packs rows without another full-picture copy.

The defaults are 256 MiB of total reconstruction memory, including scratch and maps, and 268,435,456 decoded bins for the selected picture; a caller may choose different bounds. Header limits remain independent. Input/substream bounds and coding-tree/transform bounds are enforced separately. Caller-owned input storage is separate from the charged RBSP copy. CI records the actual memory/bin budgets and reports exhaustion as failure, never as missing coverage or successful reconstruction. These are runtime safety limits, not a measurement freeze. A valid stream exceeding a caller budget is `Limit`, malformed syntax is `InvalidData`, and unimplemented tools are `Unsupported`; all carry static labels. No unsafe code, unbounded recursion, infallible growth or production panic is introduced.

## Implementation sequence

These are reviewable steps within the milestone work unit, not separately authorized measurement rounds:

1. Preserve G1 while extracting the private stream owner and retaining reconstruction parameters/entry-point mappings. Add table/equation-derived CABAC bin tests, bounded byte-source tests and context checkpoint/restore tests. No picture success is reported yet.
2. Add serial CTU/tree traversal, syntax/context derivation and coefficients, including tiles, WPP, dependent segments and PCM transitions. Intermediate unsupported tools fail explicitly. Trace only closed syntax/counter labels from self-written fixtures; no oracle implementation trace is read.
3. Add the scalar kernel crate and selected-picture reconstruction for both depths together. Integrate QP/scaling lists, transforms, prediction, skip/bypass and PCM under the memory/work budgets. Keep filtered and inter pictures refused.
4. Add the original-picture conformance selection and exact G2 comparisons, complete effective-tool counters, negative controls and affected entropy/picture fuzz targets. Preserve every earlier gate, feature-isolation check and source-bound receipt.

The milestone estimate in the parent plan is not permission to open unrelated PRs. Keep this work's design, implementation and necessary test/check fixes together. No freeze, seal, attestation or performance-registration machinery is added. Any later reference measurement follows the current project rules; it is not authorized by this implementation plan.

## Verification and existing evidence

The completed G1 census already identifies three published 8-bit unfiltered intra pictures (`IPCM_A`, `IPCM_B`, `IPCM_E`) and eight synthetic Main10 pictures from four registered recipes. These are candidate counts, not native reconstruction passes. The published cases do not establish general 8-bit transform coverage, and the synthetic CTU/TU flags do not establish which coefficient, transform-skip or bypass paths are effective. None of these eleven candidates signals tiles, WPP or dependent segments. This is an explicit coverage gap, not permission to defer their serial implementation.

The completing driver derives its exact target list from the full SHA-bound census, retaining archive duplicates and synthetic labels. It checks every G2-eligible picture, not the first passing picture or a changed subset. Newly found eligible pictures join the denominator. Missing candidates, target-selection errors, incomplete oracle output, wrong packing, count/order disagreement or any hash mismatch fail. Keep the two existing SHA-bound SAODBLK exclusions and the named unequal-depth refusal unchanged, with no new exclusion authority.

| Layer | Required evidence |
|---|---|
| CABAC | Specification-derived initialization and bin transitions; regular/bypass/termination extremes; exact substream exhaustion; truncation, bad restart and finite-work failures |
| Serial state | Self-written wire cases for tile boundaries, WPP context storage/restoration, narrow rows, dependent segments, entry-point offsets spanning emulation prevention, unavailable neighbours and PCM restart; later published pixel coverage remains visible |
| Kernels | Both depths, supported transform/prediction sizes and modes, QP extremes, custom/default scaling lists, clipped boundaries and skip/bypass/PCM distinctions; independent equation-derived expected values |
| Picture gate | Every registered G2 candidate's exact reference count, selected coded/output identity, coded/visible geometry and packing; cropped-picture reference MD5 and every applicable coded-component hash |
| Negative controls | Wrong picture, changed sample/component/hash, swapped synthetic references, truncated entropy, overlapping subsets, duplicate/missing segments, selected inter/filtered pictures, exhausted budgets and callback failure must fail |
| Regression and isolation | Complete G1, unchanged header-only behavior, strict Clippy, feature-off/default consumer graphs, full-width 10-bit samples and terminal-session tests |

Validate every defined picture-hash SEI algorithm when present: MD5, CRC and checksum, using clauses D.2.20 and D.3.20. Report absent and reserved hashes separately; a reserved hash is not checked coverage. Compare against published reference bytes/hashes and independently complete FFmpeg 9.0.2 output in the registered layout. When only a published whole-output MD5 is available, the [reference decision of 2026-11-05](hevc.md) permits oracle per-picture hashes only after exact complete-output MD5 agreement. Label each as oracle-derived and retain the original stream, coded/output picture indices, anchoring MD5, published attachment and oracle identities. A missing or mismatching anchor remains blocking. Synthetic references remain separately labelled. Stderr wording does not gate otherwise verified output; preserve the existing closed oracle-error predicate.

Pixel equality and effective syntax coverage are separate facts. Instrument decoded syntax, not just SPS/PPS flags. A tool whose effective reference case is missing stays unproved. Retain filtered tile/WPP/dependent candidates for G3 instead of disabling their filters. Any additional unfiltered reference recipe requires an explicit registration and the applicable authorization before use; none is generated by this plan. Missing coverage blocks the corresponding completion claim rather than being relabelled as a pass.

Run tiny specification-written unit cases and short fixture checks locally. Run all official/synthetic media decoding, corpus acquisition and fuzz campaigns on GitHub Actions, with pinned binary oracles, hard timeouts and credential-free media children. Use only the existing bounded acquisition path. No additional file draw, held-out tuning, timed experiment or remote VM is authorized. Preserve original NAL bytes for native input and the sole existing oracle-only parameter-set preparation where applicable.

## Runtime verification

The `intra` example retains one original coded picture as planar u16le and emits complete metadata only after full input and successful finalization; failure removes its newly created output. `scripts/hevc_intra.py` runs the existing G1 owner once, derives every eligible coded picture from that fresh census, requires its original output/reference mapping, and compares native packed/cropped pixels with the registered reference and complete pinned-oracle frame hashes. It keeps coded-component hashes separate from visible-picture hashes. Controls reject missing targets, truly truncated entropy, exhausted budgets and output clobbering; media-free comparator mutations reject changed samples, components, references and crop origins.

The driver reports finite budgets and decoded work, including mode/transform counts and sign-hidden groups. Missing effective serial-feature reference cases remain coverage gaps even when every available picture matches. Unit/control checks and compilation do not establish G2; the complete source-bound CI result is required. A specification ambiguity or clean-room exception is escalated, never filled from another implementation.

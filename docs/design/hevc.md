# HEVC decoding

Status: Planned (milestone 0). This note designs CoreAV's own HEVC decoder and pre-registers its gates. It adds no decoder, backend, feature or measurement result. The crate boundaries follow [H.264](h264.md) and [VP9](vp9.md), behind [the decode boundary](decode.md).

## What this is

On Linux servers, CoreAV currently decodes HEVC through the sandboxed FFmpeg fallback. An HEVC transcode therefore needs an FFmpeg binary even when its encoder does not. The goal is a specification-written, memory-safe Rust decoder in the calling process, with bit-exact Main and Main10 output and no external decoder binary.

The HEVC class misses transcode bar B5, whose limit is 1.05 times FFmpeg's wall time for each encoder arm. [Transcode round 2](../measurements/transcode.md) reports 1.057 for the `ffmpeg` arm and 1.200 for `x264`. After the huge-page change, [the 10-bit input round](../measurements/transcode-x264-10bit.md) reports 1.056 and 1.146. The H.264 class, using CoreAV's own decoder, reached 1.038 on the `ffmpeg` arm. These are historical FFmpeg 7.0.2 comparisons, not results against today's pinned oracle. All new comparisons below use FFmpeg 9.0.2.

A native decoder removes the fallback worker and its frame copies. It does not remove x264's serial conversion of 10-bit input, which the latter measurement identifies separately. Passing decoder-alone bars does not imply passing B5. Both arms remain gates, and reaching parity does not end performance work.

## Scope

The target is single-layer HEVC Main and Main10, planar 4:2:0, with matching luma and chroma depths of 8 or 10 bits. Main uses 8 bits. Main10 accepts both depths. Main Still Picture and Main10 still-picture-constrained streams within these limits use the same decoding processes. Admission checks the profile compatibility flags as well as `general_profile_idc`, then validates the active parameter sets and tools. A profile name alone is not sufficient.

In scope are I, P and B slices, dependent slice segments, multiple slices, CTUs of 16, 32 and 64 samples, coding and transform quadtrees, asymmetric motion partitions, scaling lists, transform skip and transquant bypass allowed by these profiles, PCM, weighted prediction, merge and advanced motion vector prediction, temporal motion prediction, short- and long-term reference picture sets, tiles, wavefront parallel processing (WPP), temporal sublayers, and IDR, CRA and BLA random access. Tile and WPP combinations obey the profile constraints. Deblocking and sample adaptive offset (SAO), conformance-window cropping, VUI colour, picture order and drain behaviour are part of correctness, not later quality options.

Out of scope, refused with `Unsupported`: range extensions (RExt), screen content coding (SCC), scalable or multilayer coding (SHVC), multiview coding (MV-HEVC), 3D-HEVC, nonzero `nuh_layer_id`, extension tools outside the admitted profiles, monochrome, 4:2:2, 4:4:4, 9-bit or depths above 10, unequal component depths, and field sequences (`field_seq_flag = 1`). This is a bounded software-decoder scope, not a claim of every profile, tier, level or HRD output-timing conformance. Dimensions, DPB size and work remain subject to the caller's limits. Malformed syntax is `InvalidData`; an exhausted resource budget is `Limit`, not an unsupported profile.

Until a milestone implements a tool, that tool is refused, never decoded approximately. Missing references and damaged packets fail explicitly. Concealment is not a way to pass a hash gate. Informational SEI messages with no required pixel effect may be skipped under the specification's rules; skipped metadata is not a claim to implement an extension. HDR display tone mapping and optional display post-processing are outside the reconstructed-sample comparison.

## Specification and the clean room

- The authority is [ITU-T H.265 (V11), January 2026](https://www.itu.int/rec/T-REC-H.265-202601-I/en), aligned with ISO/IEC 23008-2. The English PDF downloaded on 2026-11-04 has SHA-256 `78b093480e178a2b99d8cb7499934240f359c50c6d09568f8be651bc1151d4d2`. Clause references here use that edition: syntax and semantics in clause 7, decoding in clause 8, CABAC in clause 9, profiles in Annex A, DPB operation in Annex C, and decoded-picture-hash SEI in D.2.20 and D.3.20. ISO/IEC 14496-15 and the Matroska codec mappings define the existing container bindings.
- [ITU-T H.265.1 (October 2018)](https://www.itu.int/rec/T-REC-H.265.1-201810-I/en) defines conformance testing. Its public [bitstream listing](https://www.itu.int/wftp3/av-arch/jctvc-site/bitstream_exchange/draft_conformance/) is the source of names, not an implementation repository. On 2026-11-04, direct ZIP links in its six category directories total 317: `HEVC_v1` 147, `RExt` 49, `SCC` 15, `SHVC` 70, `MV-HEVC` 9 and `3D-HEVC` 27. Four additional ZIPs under `SHVC/replaced/` are not counted. The candidate set for this decoder is all **147 archives** directly listed in [HEVC_v1](https://www.itu.int/wftp3/av-arch/jctvc-site/bitstream_exchange/draft_conformance/HEVC_v1/), not the other extensions. This counts archives, not verified distinct streams or verified in-scope profiles. The listing does not identify each stream's profile. Its captured HTML SHA-256 is `a690c5cca2fedf9a8f1a385585bba6ba4e45afc8ebe1336107427213d9dfe4bd`.
- Milestone 1 commits the conformance manifest: archive and extracted bitstream SHA-256, published MD5 data and its layout, the H.265.1 test identity, profile, depth and every scope refusal. It reconciles the listing with H.265.1's tables, rather than assuming one archive equals one test. It accounts for every candidate, including duplicates and replacements. No pixel mismatch may create an exclusion. A missing reference hash must be reported and resolved before that test's gate can pass. The media are fetched and verified, not committed.
- Bitstreams and their reference MD5s are data. Decoded-picture-hash SEI supplies a second, per-component check on the reconstructed coded picture before output cropping. It is not interchangeable with a cropped output frame's MD5. All defined hash types (MD5, CRC and checksum) are checked when present, following D.3.20. Reserved hash types follow the specification's ignore rule and are reported separately, never counted as hash coverage.
- Never read HM, FFmpeg, libde265, OpenHEVC or any other HEVC implementation's source, repository test lists, or source-derived implementation notes. No HEVC source study is authorized by this plan. FFmpeg and ffprobe **9.0.2** are binary oracles only, exactly as pinned by [`scripts/oracle-ffmpeg.env`](../../scripts/oracle-ffmpeg.env), including its archive digest. No FFmpeg library is linked. Profiling, if needed in later work, stays at function level without source lines or disassembly.

## Packaging: opt-in, separately packaged

HEVC decoding can require a patent licence depending on jurisdiction and use. The packaging is the native H.264 decoder's: the future `native-hevc` Cargo feature is opt-in and absent from every default build. CLI, Python, WebAssembly and npm consumers must not acquire it through transitive default features. A build without it refuses an explicit native HEVC request with `unsupported_feature`.

The three planned crates are workspace members for CI, not default artifact dependencies:

| Crate | Responsibility | Safety |
|---|---|---|
| `coreav-hevc` | NAL and parameter parsing, CABAC, CTU traversal, reconstruction control, POC, reference picture sets and DPB | `forbid(unsafe_code)` |
| `coreav-kernels-hevc` | Scalar transforms, prediction, interpolation, deblocking and SAO, followed by measured SIMD kernels | Safe scalar reference first; each future unsafe exception needs its own scoped policy record and `docs/unsafe-sites.md` entries |
| `coreav-backend-hevc` | Packet and frame adaptation to `coreav-decode`, caller limits, output ordering and metadata | `forbid(unsafe_code)` |

The native backend runs in the calling process, as native H.264 does. It emits 8-bit planar samples or low-bit-aligned little-endian 16-bit samples at depth 10 without reducing precision. Pictures retain their activated parameter sets' colour and crop information through reordering. The backend owns no second slice parser. Unsupported configuration may be refused during selection; after a selected session fails, it is terminal, with no packet-time fallback that hides a native failure.

No default selection, release feature set or published package changes in milestone 0. Each future crate's README and API documentation must state the patent position and opt-in status. Inclusion in a full release, publication, or a change to default selection remains a separate maintainer decision. An encoder and OS decoder paths are outside this plan.

### What can be shared with H.264

Share only where both specifications and tests establish the same contract. Do not make HEVC depend on the patented H.264 backend merely to reuse utilities.

| Area | Reusable structure | HEVC-owned semantics |
|---|---|---|
| NAL layer | Bounded Annex B and length-prefixed framing, emulation-prevention removal, checked bit access | The two-byte header, six-bit unit type, layer and temporal identifiers, VPS, access-unit boundaries and HEVC parameter activation. H.264's one-byte header and `nal_ref_idc` do not apply |
| CABAC | The separation of a bounded byte source, arithmetic range and offset, context decisions, bypass and termination | Initialization tables, binarization, context derivation, coefficient coding, slice initialization, dependent-segment state and WPP synchronization. H.264's context indices and residual syntax do not apply |
| DPB | Immutable shared pictures, allocation accounting, separate reference and pending-output lifetimes, ordered output queues | HEVC POC derivation, reference picture sets, list construction, temporal-layer limits, bumping, and IRAP leading-picture rules. H.264's `frame_num`, MMCO and sliding window do not apply |

The present H.264 modules mix these structures with codec rules. Extract a small codec-neutral utility only when HEVC has a real caller and both codecs' gates prove it. A speculative shared decoder framework would make the first HEVC gates harder to isolate. Pixel kernels remain HEVC's: its interpolation, transforms and filter equations are not H.264 kernels under new names.

## The plan, in milestones

Each row is one or more independently reviewed PRs. A milestone's completing PR must pass its gate and all earlier gates. Intermediate PRs have bounded tests and explicit tool refusals. Neither staged correctness nor an instruction-count checkpoint is a public speed claim.

| Milestone | Work | PRs | Gate |
|---|---|---|---|
| 0 | This note and fixed corpus roster | 1 | Review; no decoder code or timed run |
| 1 | Crate boundaries, limits and errors; NAL, VPS, SPS, PPS, VUI, SEI and slice headers; conformance manifest and harness | 2 | G1: account for every one of the 147 candidate archives; parse every in-scope stream's headers to EOF, with independently checked size, depth, picture boundaries and published output counts; assert named out-of-scope refusals and truncated-header errors |
| 2 | Scalar 8- and 10-bit CABAC, CTU and transform trees, intra modes, coefficient decoding, scaling lists, inverse transforms, transform skip, transquant bypass and PCM | 3 | G2: exact reference hashes for every independently decodable intra picture in the manifest with deblocking and SAO disabled; bounded entropy termination tests and malformed-input tests. No claim for filtered pictures yet |
| 3 | Deblocking, then SAO, with slice and tile boundaries, PCM and bypass rules, bit-depth clipping, SAO merge, edge and band offsets | 3 | G3: every independently decodable intra picture in scope equals its published hash and applicable picture-hash SEI, filters enabled or disabled as signalled; first independently decodable IRAP picture of every development file equals the oracle |
| 4 | P and B prediction, merge and motion candidates, interpolation and weights; reference picture sets, POC, output bumping, temporal sublayers, CRA and BLA leading pictures, dependent segments, tiles and WPP entropy-state rules in scalar order | 5 | G4: C1 and C2 for the complete scalar decoder at both depths; streams with reordering, long-term references and random access must each have effective hash coverage |
| B | Native backend, opt-in consumer wiring, drain and terminal-error semantics, keyframe-only thumbnails, colour and crop propagation, boundary fuzz target | 2 | C1, C2 and C5 through the boundary; decode-boundary correctness and limit gates. Verify no default artifact links HEVC and no native-selected test invokes a decoder fallback |
| 5 | AVX2 kernel families first, then NEON; each family a measured round with safe fallback and a scoped exception where needed | 4 to 7 | C3, earlier correctness gates with scalar and automatic dispatch, and each round's pre-registered development bars |
| 6 | Parallel tiles or WPP rows, then frames in flight with post-filter reference progress and bounded memory | 2 to 4 | T1 to T3 below; deterministic output at every thread count |
| R | Reviewed speed driver and run registration, then gated run and separate results | 2 | HEVC-1 to HEVC-5, with complete eligible input accounting |

About **24 to 29 PRs including this note**, versus VP9's estimate of about 20 to 25. The scalar decoder and backend take about 15 implementation PRs before SIMD and threading. This is a planning estimate, not a stage-gate verdict or a prediction of later optimization rounds. H.264 already has conformance and production experience to reuse, but its repeated performance rounds show why correct scalar output is not a speed estimate. HEVC adds larger partition trees, 10-bit arithmetic, SAO, reference picture sets and WPP state dependencies that cannot be borrowed from H.264.

**Both in-loop filters precede inter prediction.** H.265 clause 8.7.1 applies deblocking first and SAO second. Published output hashes and decoded-picture-hash SEI describe the resulting decoded picture, and later inter pictures reference that filtered picture. As with VP9, an unfiltered reconstruction cannot generally be checked against those hashes. G2 therefore checks only independently decodable intra pictures whose signalling disables both filters. G3 checks all intra pictures before G4 depends on them as references. No assumption that FFmpeg exposes an intermediate filter stage is needed, and this plan makes no claim to have tested a filter-skipping option.

Milestone 1 must identify nonempty G2 and G3 coverage sets before reconstruction work starts. If published data lack an effective case for a filter or motion tool, register a deterministic synthetic fixture and its black-box checks before judging it. A fixture must show that the feature changes decoded bytes, with a crossed-hash negative control, not merely count a header flag. Never disable a required filter on an existing judged stream to manufacture agreement. Inter-specific boundary strengths join G4's checks, even though the filter equations land in G3.

After G4, record scalar instruction counts and decoder-only development timings against the pinned binary to size the remaining work. This checkpoint does not open the held-out speed set and does not pause other milestones. Before building an optimization round, measure its removable-work ceiling on a few development streams. Reject a candidate whose ceiling cannot clear its registered bar.

## Bars

Every threshold is committed before its run. Drivers and a round registration must be reviewed and merged before a judged run. Milestone 0 runs no decoder, timing experiment or corpus acquisition. Later registrations add binary hashes, exact commands and host identity, not relaxed thresholds or a newly selected passing subset.

### Correctness

| Bar | Set and comparison | Threshold |
|---|---|---|
| C1 conformance | Every in-scope stream of milestone 1's manifest, complete to EOF, against published reference MD5s and every applicable decoded-picture-hash SEI | 100% of pictures, components and hashes equal, with the reference's count, order, geometry and packing. Hash coverage and absent SEI are reported separately. No known-divergence exception |
| C2 corpus | Every in-scope file of the fixed roster below, against FFmpeg 9.0.2's native `hevc` decoder, first 120 selected-track packets; all eight B5 files also to EOF | 100% equal frame counts, presentation order, integer pts, packed sizes and MD5s. Any decoder failure, incomplete comparison or unregistered exclusion fails |
| C3 kernels | Each optimized kernel against its scalar reference, both depths, all supported block sizes, fractional positions, filter strengths, strides, boundaries and extreme samples | Bit-identical property tests and differential fuzzing, at least 10 minutes per family on each target architecture before accepting the family |
| C4 robustness | Header, entropy, picture, backend and threading fuzz targets | Affected-target CI on each PR; a campaign of at least 24 CPU-hours before the first release containing the decoder, with no crash, hang or out-of-memory result. Checked arithmetic and the workspace's no-panic lints remain mandatory |
| C5 random access | Manifest and corpus random-access points, full decode versus native seek and keyframe-only paths | The same selected picture, bytes and pts as full decode for every admitted start; correct treatment of CRA leading pictures and unavailable references. Refuse an unsafe start rather than return an approximate image |

The conformance harness follows each published MD5's declared layout, including whether it covers a complete YUV file or individual pictures. D.3.20 hashes each component of the coded picture, with low byte first above 8 bits. The corpus harness instead hashes the visible cropped planar output, without stride padding, as `yuv420p` or `yuv420p10le`. No 10-bit-to-8-bit conversion is allowed on either side. A mismatch between published data, SEI and the binary oracle stops the gate for investigation, not a two-against-one vote.

The C2 oracle command is `ffmpeg -copyts -c:v hevc -i FILE -map 0:v:0 -fps_mode passthrough -enc_time_base demux -f framemd5 -`. It must decode the same selected stream. Compare in the container's presentation clock after the same edit mapping. Verify the oracle's `#tb` and any container-to-presentation mapping independently before comparing integer pts. A clock or edit-mapping disagreement fails the method and needs a recorded fix before another run, not a decoder-timestamp exemption.

For a prefix comparison, feed 120 selected-track packets, drain, and scan the remaining packet metadata. Keep only frames with pts strictly below the minimum pts of every unsent packet. With no unsent packet, keep every frame. Bound oracle output independently of native output, and fail if the bound is reached before the cutoff. EOF B5 comparisons have no prefix cutoff. Harness deadlines, memory and capture limits are fixed before acquisition; hitting a limit is a failed comparison, not missing work that can disappear from the denominator.

C1 runs on GitHub-hosted `ubuntu-24.04` x86-64 and `macos-15` arm64, with scalar and automatic kernels. C2 runs on Linux with credentials present only in acquisition steps and no credentials in decoder subprocesses. From milestone 6, C1 and C2 cover automatic kernels at 1, 2 and 8 threads and scalar at 1. C3 runs on both architectures. These are untimed correctness checks, not host classes for wall-clock claims. Hostile media never go to a personal workstation.

### Fixed file sets

[`hevc-native/roster.json`](../measurements/hevc-native/roster.json) fixes **54 distinct files** by full SHA-256: 26 development and 28 held-out-partition files. It is a metadata-only union of existing selections, not a new media draw. It also pins each selection's bytes by SHA-256. Membership does not depend on a historical success status:

- C2 includes the 20 HEVC entries of `transcode/round-2/corpus-selection.json`, the 15 HEVC entries of `speed-dev-2.jsonl`, and the 28 HEVC entries of `speed-2-hevc.jsonl`, deduplicated by SHA-256. Shared entries must agree on partition. The eight B5 entries below add no new file to this union.
- Development work uses only the 26 development files and conformance vectors. HEVC-1, HEVC-2 and HEVC-5 use all 28 held-out-partition files, subject only to pre-registered scope and container eligibility. HEVC-3 and HEVC-4 use exactly the eight HEVC `files` entries of `transcode/speed-selection.json`, not its nine reserve entries. All eight belong to the held-out partition.
- The earlier thumbnail record refused `665dddf09f221cc63cedf9987281c81a226a8eb24bf1231cd32ed337de3686db` with `truncated_structure`. It stays in the roster and accounting. Its previous refusal is not permission to drop it silently or to waive decoder differences.

Every runner verifies the roster and media digests and exact requested membership before work. Milestone 1 records an untimed profile and container census for all 54 files. Out-of-scope files remain explicit refusals, with reasons independent of speed. A missing input, unexpected native refusal or oracle error prevents a passing gate. Fewer than five eligible B5 files prevents a B5 verdict rather than drawing reserves. Report admitted and refused counts by partition, profile and depth.

These files already have fallback measurements, and some held-out-partition results informed previous changes. They are fixed regression and decoder-development validation sets, **not a fresh holdout for a new general speed claim**. Do not tune native HEVC on the 28-file partition. A future claim that CoreAV is faster requires a separately committed fresh-held-out registration before acquisition, with the same bars. Old partition labels do not restore freshness.

### Speed

Ratios are CoreAV over FFmpeg. Policy record 0034 requires medians at most 1.00 overall and per reported class by default. A claim of being faster additionally requires the upper bound of the 95% confidence interval below 1.00 on fresh held-out inputs. Report the share of files below 1.00 beside medians and intervals. The 80% faster-file goal is an aim, not a replacement gate.

| Bar | Measure | Threshold |
|---|---|---|
| HEVC-1 one thread | Decoder alone, one thread: native discard versus FFmpeg `-f null`, and native raw output versus FFmpeg `-f rawvideo`, on the eligible 28-file partition | Median at most **1.00**, both pairings, overall and per resolution class |
| HEVC-2 eight threads | The same workloads, eight threads on both sides | Median at most **1.00**, both pairings, overall and per resolution class |
| HEVC-3 CPU | Boundary decoder harness allocating and exporting each frame without MD5 or an extra packed copy, native default threads versus FFmpeg `hevc` default threads, eight B5 files to EOF | Median user plus system CPU ratio at most **1.00**, overall and for the files at 3840×2160 samples or above |
| HEVC-4 transcode | B5 with native HEVC selected, `--encoder ffmpeg` and `--encoder x264`, the same encoder version, preset and quality on both sides | Median at most **1.05** over the HEVC class, **each arm independently**. Also report 8-bit and 10-bit groups. Other codec classes cannot offset a HEVC miss |
| HEVC-5 thumbnails | S1 to S5 of [thumbnails](thumbnails.md), through native HEVC on the eligible 28-file partition, with image-agreement gates unchanged | Median at most **1.00**, each S bar overall and per resolution class |

HEVC-4's exception above parity is deliberate under policy record 0034. Even after removing the decoder worker, the same external encoder and its process start, raw-frame transfer and pipe remain. B5 bounds that integration cost at 5%; it does not assert that encoding the same pictures is faster. The native decoder may still miss this bar because x264's conversion cost remains. HEVC-1 to HEVC-3 and HEVC-5 get no such exception. A future all-codec B5 run must also meet the overall and every-class bars, not just HEVC.

**Build and oracle.** Use `--profile dist`, automatic kernels and explicit native HEVC selection. HEVC-3 to HEVC-5 use the release recipe's `x86_64-unknown-linux-musl` build with `native-hevc` added deliberately, not a change to release defaults. Record enabled features, native backend receipts, binary hashes, FFmpeg's complete version and matching x264 identity. FFmpeg uses its native `hevc` decoder, never a hardware decoder or another wrapper. No hidden decoder fallback counts as native work.

**Decoder workloads.** For HEVC-1 and HEVC-2, packet-copy each first video stream into Annex B without decoding, preserving required parameter sets, and hash the resulting bytes. Both decoders read that identical elementary stream. Decode the first 1,800 pictures below 921,600 luma samples, 600 through 2,073,600, and 150 above, or EOF if shorter. The registration must fix how each side stops after the same coded-picture prefix, drains it and verifies equal picture counts; `-frames:v` alone is not proof of equal decoding work with reordering. Discard still reconstructs and filters every required picture and updates references. Raw output writes the same planar bytes to the same null sink on both sides.

HEVC-1 has one warm-up per command and five interleaved rounds per file, both sides pinned to core 2. HEVC-2 has one warm-up and ten rounds per file, both sides at eight threads, unpinned within the same eight physical cores. Shuffle commands once per round with seed 20261104, with no repeated command inside a round or minimum-duration adjustment. The file ratio is the median native time divided by the median oracle time. Report the median across files with a 95% bootstrap interval, 2,000 resamples, seed 20261104. Resolution classes use the first coded picture's luma samples: below 921,600, through 2,073,600, and above. A class below five eligible files is joined to the next larger class, or the next smaller for the largest class, and is also reported alone. Report depth groups separately without selecting a favourable one.

**CPU, transcode and thumbnail workloads.** HEVC-3 follows VP-3's allocated-frame method: one complete decode per process, page cache primed, three rotated rounds, CPU from `wait4`. If FFmpeg's wall time is below two seconds, the registration fixes a common repeated-invocation unit for both sides on development data before opening the evaluation set. Sum CPU within that unit, take per-file median ratios, and bootstrap 10,000 times with seed 518. Report the large-file subset even if small. HEVC-4 inherits B5's no-scale `--preset medium --crf 23`, page cache, tmpfs output and three alternating rounds per file, with a 10,000-resample interval. Run both encoder arms against the matching FFmpeg baseline. HEVC-5 inherits the thumbnail note's target at 40% of presented duration, width 320, S1 to S5 commands, same-frame checks, one warm-up, five alternating rounds, page cache, tmpfs outputs and 10,000-resample intervals. Fix seeds and every complete invocation in the reviewed registration before either run.

**Hosts.** Match VP9's host classes: HEVC-1 on the calibrated Compute Engine one-thread class used by H.264, with A/A and null-arm calibration recorded before any timed work. HEVC-2 uses `c4-standard-16`, SMT off, eight physical cores. HEVC-3 to HEVC-5 use `c4-standard-8`, SMT off, four physical cores, native default one thread per physical core. Timed hosts are on-demand, pinned to a recorded CPU class, never spot instances or shared runner pools. Record CPU, kernel, affinity, governor, memory and compiler. Gate the baseline before and after a session and void a host whose gate changes by more than 5%. Hosted CI may count instructions and check correctness but cannot decide a wall-clock bar. No timed work is authorized by milestone 0.

### Threading

- **T1, exactness.** Every conformance and corpus input at 1, 2, 3, 8 and 16 threads gives the same samples, output order and pts as one thread. Vectors run on both architectures. A threading fuzz target compares one and three threads. Scheduling must not expose a reference row before its required deblocking and SAO finish.
- **T2, one-thread cost.** By HEVC-1's method on development files only, one thread against the parent of the threading milestone's first PR: median ratio at most **1.01**, both pairings.
- **T3, scaling.** On `c4-standard-16`, SMT off, development files only, compare both sides at one and eight threads in one session. The median per-file `(native_8 / ffmpeg_8) / (native_1 / ffmpeg_1)` is at most **1.00**, both pairings.

A missed run is final for its arms. Only a recorded host fault permits a rerun, never a narrow miss or a method change discovered from a result. Corrections to a faulty method need their own reviewed pre-registration before new evidence is judged.

## Risks

| Risk | Consequence | Mitigation |
|---|---|---|
| The listing contains duplicates, older revisions or out-of-scope streams | An archive count could masquerade as conformance coverage | Reconcile every candidate with H.265.1, pin extracted data and report admitted tests and refusals separately in milestone 1 |
| In-loop filtering or high-depth arithmetic is wrong | Hash mismatches propagate through later references | Both depths from the first scalar kernels, deblocking and SAO before inter, full coded-picture SEI checks as well as output MD5s |
| CABAC, CTU traversal or reference traffic dominates | SIMD alone cannot reach decoder parity | Scalar checkpoint and removable-work ceiling before each round; no unsafe parser shortcut |
| WPP state or frame progress is published too early | Nondeterministic corruption or a deadlock | Serial entropy-state coverage before parallel scheduling; post-filter progress, bounded queues and T1 |
| 8K Main10 references and scratch exhaust memory | A server loses capacity or the process fails | Charge DPB, queued output, row scratch and simultaneous allocation growth before reservation under the boundary's limits |
| Native decoding clears its own bars but x264 conversion dominates | HEVC-4 still misses on 10-bit sources | Keep both arms as independent gates and attribute the remaining cost without widening this decoder plan into encoder work |
| Reused selections are treated as fresh held-out evidence | An unsupported general speed claim | Preserve partition separation and label regression results honestly; register fresh evidence separately before a faster claim |
| Shared readers and backend selection change concurrently | Integration conflicts or an accidental default dependency | Merge current main before backend work, coordinate that PR, and test both feature graphs |

## Decisions

- **Linux software decoding first.** Removing the external decoder dependency and frame-copy path addresses the current HEVC gap. An HEVC encoder, VideoToolbox and Media Foundation paths are separate work.
- **Main and Main10 together in scalar reconstruction.** A later depth retrofit would repeat every transform, clipping and filter proof. The bounded scope deliberately excludes other chroma formats, extensions and fields.
- **Three crates, limited sharing.** Keep bitstream rules, pixel kernels and the decode boundary separate, with no H.264 semantic dependency. Extract framing utilities only when both callers prove their contract.
- **Deblocking and SAO before inter prediction.** This gives intra output an independent published-hash gate and inter prediction already-verified reference pictures. No oracle intermediate-output switch is assumed.
- **Strict clean room.** Specifications and published conformance data suffice for this plan. HM and all other HEVC source remain unread. An unresolved specification question goes back for a decision rather than silently importing another implementation's answer.
- **Preserve B5, tighten decoder and thumbnail work to parity.** The written external-encoder overhead reason supports HEVC-4's 1.05, not a looser decoder bar. Future speed claims need fresh held-out data and policy record 0034's confidence bound.
- **Plan only.** This note authorizes no publication, default feature change, performance verdict, source-study exception or unsafe implementation. Future milestone PRs supply their own evidence and scoped approvals where required.

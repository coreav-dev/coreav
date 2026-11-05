# Bounded H.264 copy into MPEG-TS

Status: bounded implementation. Preparation checks the serialized transport schedule before publication. Generated command-level controls compare complete packets, stream and program metadata, and decoded frames with pinned FFmpeg 9.0.2. The first commit of this pull request is the plan; later commits implement and qualify it.

## Exact output or refusal

The first unit covers one local ISO input, one selected `avc1` video track, explicit audio exclusion, and MPEG-TS output. Effective video `copy` uses the existing codec-option precedence. The exact output filter `-bsf:v h264_mp4toannexb` and its qualified automatic equivalent share one conversion owner. Other filters, input-scoped filters, extra inputs, encoding, seeking, bounds, concat, and unsupported options retain refusal or original-argument fallback. A `.m2ts` destination refuses even with explicit `-f mpegts`: the pin selects 192-byte Blu-ray transport output and different PIDs from the filename, which this 188-byte profile does not reproduce.

The initial source has one video track and no other tracks, valid local sample tables, one description, no edits, no fragments, and no compressed movie box. Its AVC description has four-byte NAL lengths and one unchanged SPS/PPS pair. Initial qualification covers progressive High-profile, 8-bit 4:2:0 video with one coded slice per sample and the qualified prefix SEI form. Existing AUDs, in-band configuration changes, unknown NAL forms, unsupported metadata, chapters, and external or encrypted data refuse. Audio-bearing inputs with `-an` remain a separate qualification case rather than silently extending this source family.

Strict-safe refusal exits 4 with empty stdout and leaves absent or existing destinations, neighboring files, and staging sentinels unchanged. Ordinary fallback retains the existing argument owner and pinned FFmpeg 9.0.2 behavior. Native MP4 APIs, timing policies, and default output bytes do not change.

## A sealed result owns publication

`coreav-isobmff-write::ts_output` owns this source-specific conversion. Its `prepare(source, stream_index, limits)` returns an opaque `Prepared` containing complete validated TS bytes. `Prepared::write_to` has no source handle and can fail only on output I/O. `coreav-cli::ffmpeg::ts` prepares this result before calling the existing output transaction.

Preparation captures a bounded owned input snapshot and never rereads the live source after sealing it. Parsing, configuration admission, packet conversion, transport serialization, and schedule checks all consume that snapshot. A late unsupported sample therefore cannot produce a successful preparation or touch the destination. Snapshotting does not claim an atomic filesystem version while another process writes during capture. Capture errors and observable inconsistencies refuse.

A public packet-fed muxer in `coreav-mpegts` was considered and rejected. There is no second writer consumer, and that interface would split preflight, clock, allocation, and publication guarantees across callers. The existing transport reader remains unchanged.

The TS branch runs before ordinary MP4 planning and its positive-video-origin normalization. The argument parser retains enough grouped-option information to reject options that existing MP4 parsing otherwise consumes, including unsupported container flags. Non-TS parsing keeps its existing behavior.

## Clocks and AVC bytes

No-edit sources retain their media DTS and CTS. Every timestamp and positive constant duration must be exactly representable in 90 kHz. DTS starts at zero and remains contiguous, and no access unit may have PTS before DTS. Composition times form a complete, unique set of presentation slots at the admitted cadence. Agreement with the progressive SPS nominal timing is a qualified admission rule, not a normative inference when `fixed_frame_rate_flag` is zero. Unknown or inconsistent timing, VFR, incomplete final duration, negative unqualified clocks, and 33-bit wrap refuse rather than round. One- and two-sample sources initially refuse because the reference stream duration differs from the sum of packet durations.

The compatibility mapping adds `126000` ticks to both DTS and CTS. It preserves a positive B-frame composition origin. This offset is a pin behavior, not an H.222.0 requirement. Six independently generated controls cover 24, 30, and 30000/1001 fps with and without B-frames. All 288 packet clocks and durations, including the final duration, support that mapping. Repeated-IDR controls cover four GOPs.

Each admitted access unit starts with `00 00 00 01 09 f0`. Original NAL payloads and order remain unchanged. Configured SPS and PPS precede each IDR slice, after any admitted leading SEI. Within the converted sequence after the AUD, the first NAL uses a four-byte start code and later NALs use three-byte start codes, except that injected SPS and PPS always use four. An IDR following injected parameter sets therefore uses three bytes even when it was the first source NAL. This qualified conversion reproduces every transformed packet byte in the six controls. Other layouts refuse until independently qualified.

Record handling reuses CoreAV's existing AVC parameter-set parser and the `matroska::avc_record` canonical writer, including its `sps_format` helper. That private owner becomes shared within the writer crate, generic over borrowed byte slices, with fallible, bounded allocation and unchanged existing output bytes. The TS entry requires complete equality between the input record and the reconstructed canonical record. SPS and PPS headers separately require a zero forbidden bit and nonzero `nal_ref_idc`. The known High-profile extension tail is validated, not ignored as trailing data. Fields whose meaning is not established by the reviewed code and H.264 refuse. This unit does not introduce another general AVC record parser or enable a native decoder in default builds.

An additive typed result from the existing ISO SPS walker retains profile, constraints, level, chroma, bit depths, progressive mode, timing, and reorder/buffering limits. It requires an SPS of at most 4096 bytes, complete RBSP trailing bits, timing information, known-absent NAL and VCL HRD, and absent picture-structure signaling. The writer checks observed presentation reordering and DTS-to-PTS occupancy against the retained limits. Copying preserves the source's coded pictures; these guards do not claim a full reference-picture conformance check.

## Transport validity is a separate gate

One program uses program number 1, PMT PID `0x1000`, video and PCR PID `0x100`, AVC stream type `0x1b`, and video PES stream ID `0xe0`. These are compatibility choices. Transport packets are 188 bytes. PAT, PMT, and SDT have valid section lengths, reserved bits, continuity, and CRCs. SDT service values match the qualified reference, including `Service01` and `FFmpeg`; they are service descriptor values, not a claim about which executable wrote the file.

One PES carries each access unit with a legal zero video PES length. PTS is present on every access unit, with DTS when it differs. Adaptation fields contain transport stuffing. An adaptation-only packet does not advance payload continuity. Every access unit carries an AUD and the parameter sets needed for AVC carriage.

The delivery schedule must derive arrival times from actual serialized byte positions and PCR values. A fixed decode lead or a successful player decode alone cannot establish T-STD validity. The implementation must check PCR repetition, transport and multiplex buffer occupancy, elementary-stream buffer occupancy, and complete delivery by each decode deadline. Initial and terminal intervals, short EOF cases, repeated tables, and null or PCR-only packets enter that proof. Unproved schedules refuse before `Prepared` exists. Matching the reference's exact PCR positions, file size, or transport bitrate is not required by the copied-packet predicate.

### Separate delivery and multiplex-buffer bounds

The checked SPS must explicitly establish absent NAL and VCL HRD. Its High profile and level select `BR` and `CPB` from H.264 Table A-1. H.222.0 section 2.14.3.1 gives video transport drain `Rx = 1800 * BR`, multiplex leak `Rb = 1200 * BR`, and elementary buffer capacity `E = 1200 * CPB`, all in bits or bits per second as applicable. The no-HRD branch uses the H.222.0 capacity, not the larger inferred H.264 High-profile CPB. Unknown levels, constraints, or HRD facts refuse.

Let `F = 27000000` and `B = 1504`. Busy packet spacing uses at least `ceil(B * F / Rx)` clock ticks. The whole transport rate is not capped at `Rb`: only PES bytes enter the multiplex buffer and only its output uses that leak rate. A global `Rb` cap cannot carry the four longer positive controls before their decode deadlines.

The scheduler is an untrusted producer of a bounded packet plan. Serialization reserves its complete output once. A separate verifier reads PCR fields, byte positions, PIDs, adaptation fields, and PES headers from those serialized bytes. It derives arrival intervals with exact arithmetic under H.222.0 equations 2-4 and 2-5, rather than trusting scheduled timestamps or buffer counters. Its own packet visits consume the work allowance. Mutation tests must reject changed clocks, positions, headers, buffer occupancy, and deadlines.

The verifier separately bounds transport backlog by one packet, multiplex backlog at the `Rb` leak rate, and elementary-buffer reservations by `E - 8` bits. Each complete converted access unit reserves its capacity before first delivery and releases it at DTS. Conservative packet-arrival and transport-delay bounds must establish that every byte reaches the elementary buffer before that DTS. The verifier also proves transport and multiplex buffers empty at least once every second. No access-unit byte may arrive earlier than DTS minus ten seconds. A schedule that cannot establish any bound refuses.

System packets use a separate bound. PAT and PMT starts are separated by at least one busy packet's duration plus `B / 1000000 + B / 80000` seconds. Both system buffers then hold at most one packet. Initial PAT and PMT must finish their bounded system delay before the first PES. SDT does not enter the system transport buffer. Selected repetition policies are at most 500 ms for PAT and PMT, at most two seconds for SDT, and at least 25 ms between repetitions of one sub-table, from ETSI TR 101 290. PCR repetition is at most 100 ms under H.222.0 section 2.7.2.

The first packet carries a PCR and the last media byte precedes a terminal PCR-only packet. PCRs may share video adaptation fields during busy intervals. Idle time advances only between adjacent PCR-only packets, with a gap of at most 100 ms. There is no PES payload between those clock anchors, so an idle rate change cannot stretch media delivery. This keeps output size tied to content instead of filling long waits with high-rate null packets. No extra media or EOS NAL is fabricated. Startup, reservations, buffer emptying, deadlines, repetition, or output limits may make the conservative schedule refuse an otherwise feasible input.

## Fixed resource bounds

The initial defaults are ceilings, not measured capacity claims:

- The input snapshot and serialized output each hold at most 32 MiB.
- ISO tables hold at most 8 MiB and parser-retained metadata at most 1 MiB.
- A source or converted access unit holds at most 1 MiB.
- There are at most 65,536 samples, 64 NALs per access unit, and 262,144 NALs overall.
- Table, sample, NAL, scheduling, and verification work together have a 1,000,000-visit ceiling. Conservative subdivisions may refuse earlier; no pass receives another full allowance unnoticed.
- One monotonic `PlanningBudget` charges at most 128 MiB of backing capacities, including replacement allocations and snapshot, parser, output, index, and temporary storage.

All reservations are fallible and follow checked size arithmetic. The output packet count is checked before each append and cannot exceed the output-byte limit divided by 188. Inserted delimiters, parameter sets, PES headers, repeated tables, adaptation fields, and schedule padding consume that same output allowance. Native limits outside this entry remain unchanged.

## Verification before admission

The existing ordered `github_shapes.packet_rows` and exact `decoded_output.compare` remain unchanged. The TS gate compares every transformed packet hash and rational PTS, DTS, and duration, complete decoded frame timing and pixels, stream metadata, and program descriptors. Container-local size and bitrate differences are accounted for separately, not hidden by weakening the registered predicate. Negative comparator mutations cover interior packets, order, final duration, pixels, PIDs, and SDT fields.

Qualification includes B-frames, multiple GOPs, additional exactly representable rates, one- and two-frame EOF, large access units, transport/adaptation boundaries, continuity rollover, and the schedule's limiting cases. Source configuration, metadata, edit, fragment, timestamp, and unsupported-command controls must refuse. Every budget has boundary and over-limit controls. Late malformed NALs and failing or mutating sources exercise preparation without destination access.

CLI controls check absent and existing destinations and a pre-existing staging sentinel. They prove strict-safe refusal, unchanged directory contents, and complete direct-pin versus fallback equality with the original argument suffix. Existing MP4 copy regressions remain in the affected test run. Affected package tests, Clippy, formatting, pin checks, public-text checks, standalone consumers, exact-head similarity evidence, CI, and independent review precede readiness. No frozen command, input, judging rule, baseline, threshold, or coverage total changes.

## Sources and clean-room record

- [H.222.0, June 2021](https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-H.222.0-202106-S!!PDF-E&type=items), joint with ISO/IEC 13818-1: sections 2.4.2, 2.4.3, 2.4.4, 2.7, 2.14, and Annex A.1 define transport timing, packetization, tables, AVC carriage, and CRCs.
- [H.264, August 2024](https://www.itu.int/rec/dologin_pub.asp?lang=e&id=T-REC-H.264-202408-S!!PDF-E&type=items): sections 7.3 and 7.4 and Annexes A, B, C, and E define AVC syntax, framing, level bounds, timing, and buffering.
- [ETSI EN 300 468 V1.17.1](https://www.etsi.org/deliver/etsi_en/300400_300499/300468/01.17.01_60/en_300468v011701p.pdf), sections 5.2.3 and 6.2.33 and Annex A define SDT and service descriptors.
- [ETSI TR 101 290 V1.4.1](https://www.etsi.org/deliver/etsi_tr/101200_101299/101290/01.04.01_60/tr_101290v010401p.pdf), sections 5.2 and 5.3 provide transport validation guidance.
- Existing record-layout handling is documented in [Matroska remux](mkv-remux.md), [transport parsing](mpegts.md), and [Apple encoder configuration sources](encoders/macos.md). The last note records ISO/IEC 14496-15:2004/Amd.1:2006, section 5.2.4.1, for High-profile extension fields.

An unsolicited search-result snippet displayed third-party Rust PCR-check source from `fishloa/rust-broadcast` during an earlier design attempt. The page was not opened, and no source was downloaded or used. That design was discarded. This plan uses a fresh design context, directly fetched authoritative documents, CoreAV's existing reviewed code, and independently generated black-box controls. No implementation source or retired design was carried into that fresh context.

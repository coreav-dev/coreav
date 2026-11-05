# Bounded H.264 copy into MPEG-TS

Status: implementation plan. Native admission remains closed until the transport schedule, metadata, and complete output comparisons below pass. This plan precedes implementation in the same pull request.

## Exact output or refusal

The first unit covers one local ISO input, one selected `avc1` video track, explicit audio exclusion, and MPEG-TS output. Effective video `copy` uses the existing codec-option precedence. The exact output filter `-bsf:v h264_mp4toannexb` and its qualified automatic equivalent share one conversion owner. Other filters, input-scoped filters, extra inputs, encoding, seeking, bounds, concat, and unsupported options retain refusal or original-argument fallback.

The initial source has one video track and no other tracks, valid local sample tables, one description, no edits, no fragments, and no compressed movie box. Its AVC description has four-byte NAL lengths and one unchanged SPS/PPS pair. Initial qualification covers progressive High-profile, 8-bit 4:2:0 video with one coded slice per sample and the qualified prefix SEI form. Existing AUDs, in-band configuration changes, unknown NAL forms, unsupported metadata, chapters, and external or encrypted data refuse. Audio-bearing inputs with `-an` remain a separate qualification case rather than silently extending this source family.

Strict-safe refusal exits 4 with empty stdout and leaves absent or existing destinations, neighboring files, and staging sentinels unchanged. Ordinary fallback retains the existing argument owner and pinned FFmpeg 9.0.2 behavior. Native MP4 APIs, timing policies, and default output bytes do not change.

## A sealed result owns publication

`coreav-isobmff-write::ts_output` owns this source-specific conversion. Its `prepare(source, stream_index, limits)` returns an opaque `Prepared` containing complete validated TS bytes and a summary. `Prepared::write_to` has no source handle and can fail only on output I/O. `coreav-cli::ffmpeg::ts` prepares this result before calling the existing output transaction.

Preparation captures a bounded owned input snapshot and never rereads the live source after sealing it. Parsing, configuration admission, packet conversion, transport serialization, and schedule checks all consume that snapshot. A late unsupported sample therefore cannot produce a successful preparation or touch the destination. Snapshotting does not claim an atomic filesystem version while another process writes during capture. Capture errors and observable inconsistencies refuse.

A public packet-fed muxer in `coreav-mpegts` was considered and rejected. There is no second writer consumer, and that interface would split preflight, clock, allocation, and publication guarantees across callers. The existing transport reader remains unchanged.

The TS branch runs before ordinary MP4 planning and its positive-video-origin normalization. The argument parser retains enough grouped-option information to reject options that existing MP4 parsing otherwise consumes, including unsupported container flags. Non-TS parsing keeps its existing behavior.

## Clocks and AVC bytes

No-edit sources retain their media DTS and CTS. Every timestamp and positive constant duration must be exactly representable in 90 kHz. DTS starts at zero and remains contiguous. Composition times form a complete, unique set of presentation slots at the admitted cadence. The cadence agrees with the progressive SPS timing. Unknown or inconsistent timing, VFR, incomplete final duration, negative unqualified clocks, and 33-bit wrap refuse rather than round.

The compatibility mapping adds `126000` ticks to both DTS and CTS. It preserves a positive B-frame composition origin. This offset is a pin behavior, not an H.222.0 requirement. Six independently generated controls cover 24, 30, and 30000/1001 fps with and without B-frames. All 288 packet clocks and durations, including the final duration, support that mapping. Repeated-IDR controls cover four GOPs.

Each admitted access unit starts with `00 00 00 01 09 f0`. Original NAL payloads and order remain unchanged. Configured SPS and PPS precede each IDR slice, after any admitted leading SEI. Within the converted sequence after the AUD, the first NAL uses a four-byte start code and later NALs use three-byte start codes, except that injected SPS and PPS always use four. An IDR following injected parameter sets therefore uses three bytes even when it was the first source NAL. This qualified conversion reproduces every transformed packet byte in the six controls. Other layouts refuse until independently qualified.

Record handling reuses CoreAV's existing AVC parameter-set parser and the `matroska::avc_record` canonical writer, including its `sps_format` helper. That private owner becomes shared within the writer crate with fallible, bounded allocation and unchanged existing output bytes. The TS entry requires complete equality between the input record and the reconstructed canonical record. The known High-profile extension tail is validated, not ignored as trailing data. Fields whose meaning is not established by the reviewed code and H.264 refuse. This unit does not introduce another general AVC record parser or enable a native decoder in default builds.

## Transport validity is a separate gate

One program uses program number 1, PMT PID `0x1000`, video and PCR PID `0x100`, AVC stream type `0x1b`, and video PES stream ID `0xe0`. These are compatibility choices. Transport packets are 188 bytes. PAT, PMT, and SDT have valid section lengths, reserved bits, continuity, and CRCs. SDT service values match the qualified reference, including `Service01` and `FFmpeg`; they are service descriptor values, not a claim about which executable wrote the file.

One PES carries each access unit with a legal zero video PES length. PTS is present on every access unit, with DTS when it differs. Adaptation fields contain transport stuffing. An adaptation-only packet does not advance payload continuity. Every access unit carries an AUD and the parameter sets needed for AVC carriage.

The delivery schedule must derive arrival times from actual serialized byte positions and PCR values. A fixed decode lead or a successful player decode alone cannot establish T-STD validity. The implementation must check PCR repetition, transport and multiplex buffer occupancy, elementary-stream buffer occupancy, and complete delivery by each decode deadline. Initial and terminal intervals, short EOF cases, repeated tables, and null or PCR-only packets enter that proof. Unproved schedules refuse before `Prepared` exists. Matching the reference's exact PCR positions, file size, or transport bitrate is not required by the copied-packet predicate.

### A level-derived constant delivery clock

The checked SPS must explicitly establish absent NAL and VCL HRD. Its High profile and level select `BR` and `CPB` from H.264 Table A-1. H.222.0 section 2.14.3.1 gives video transport drain `Rx = 1800 * BR`, multiplex leak `Rb = 1200 * BR`, and elementary buffer capacity `E = 1200 * CPB`, all in bits or bits per second as applicable. The no-HRD branch uses the H.222.0 capacity, not the larger inferred H.264 High-profile CPB. Unknown levels, constraints, or HRD facts refuse.

Let `F = 27000000`, `B = 1504`, and `Q = ceil(B * F / Rb)`. Every transport packet occupies `Q` clock ticks, so the actual transport rate `R = B * F / Q` does not exceed `Rb`. PCR-only packets occupy slots separated by at most `floor(2700000 / Q)` packets, with at least two slots per interval. Their PCR base ends at byte offset 10 and states `(slot + 1) * Q` ticks. Other byte times follow that same constant rate. Null packets fill unused slots, and all arithmetic and timestamps stay in the qualified nonwrapping range.

The video buffer proof treats a complete transport packet as a conservative `B`-bit burst. Since `R < Rx`, transport backlog stays below 188 bytes. Transport delay expands the multiplex input bound to `R * elapsed + 2 * B`. The multiplex capacity must cover `2 * B + 19 * 8` bits, including a maximum PES header. Reserve each complete converted access unit against `E - 8` bits before its first packet and release that reservation at DTS. This one-byte headroom prevents the elementary buffer from blocking the leak. Its last serialized packet must finish strictly more than `B / Rx + 2 * B / Rb` seconds before DTS. No access-unit byte may arrive earlier than DTS minus ten seconds.

System packets have their own spacing. With `Rsys = max(80000, R / 500)`, separate their starts by at least `ceil(1 + (B / 1000000 + B / Rsys) * F / Q)` slots. This bounds each system-buffer occupancy by 188 bytes. Conservatively include SDT in that spacing. PAT and PMT must have completed their bounded system-buffer delay before the first PES. PCR slots take priority; table repetition must remain within the selected 500-ms ceiling without needlessly filling every available system slot.

The bounded scheduler keeps decode-ordered access-unit and reservation cursors, PCR and table counters, and the actual packet positions. It emits due PCRs, due spaced tables, eligible video, or null packets. It does not interleave two PES payloads. At EOF it stops new tables and pads to a terminal PCR after the last bounded video and system drain, with at least two PCRs overall. Startup overhead, reservations, deadlines, repetition, padding, or output limits may make this conservative schedule refuse an otherwise feasible input. The schedule proof is not a claim about H.264 reference-picture conformance, and short-stream demuxed durations still need independent oracle controls.

## Fixed resource bounds

The initial defaults are ceilings, not measured capacity claims:

- The input snapshot and serialized output each hold at most 32 MiB.
- ISO tables hold at most 8 MiB and parser-retained metadata at most 1 MiB.
- A source or converted access unit holds at most 1 MiB.
- There are at most 65,536 samples, 64 NALs per access unit, and 262,144 NALs overall.
- Table, sample, and NAL work together have a 1,000,000-visit ceiling. Conservative subdivisions may refuse earlier; no pass receives another full allowance unnoticed.
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

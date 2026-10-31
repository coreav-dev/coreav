# Upload policy checks of Matroska and WebM files

Status: Accepted. Extends [`policy-check.md`](policy-check.md) and
[`frame-sizes.md`](frame-sizes.md) to the files `coreav probe` reads with
`coreav-mkv` ([`mkv.md`](mkv.md)), whose output is `coreav.probe-matroska/1`.
Code: `coreav-mkv` (`Segment::scan_tracks`), `coreav-isobmff` (the frame
size readers, now public), `coreav-cli` (`coreav check`, the Matroska
document), `coreav-policy` (the evaluator).

## Problem

`coreav check` evaluated ISO base media files only and rejected every
other file as `unreadable`. WebM is what browsers record (MediaRecorder),
so the gate in front of the WebM-to-MP4 remux and of any service that
takes browser uploads must read it, with the same rules and the same
fail-closed behaviour.

## Decisions

1. **One policy, one evaluator, two document shapes.** The policy schema
   stays `coreav.policy/1`; the evaluator reads either probe document and
   maps each rule to the document's field, below. Rejected: a separate
   Matroska policy (operators would maintain two policies for one upload
   path).
2. **Values Matroska does not store come from a scan of the blocks.**
   Matroska stores no per-track duration, byte count or verified picture
   size; the only file duration is the optional `Duration` element, which
   a live recording lacks and which a writer can understate. One pass over
   the packet listing (`Segment::scan_tracks`) gives, per track: the span
   from the first presentation timestamp to the end of the last frame,
   the bytes of every frame, and the bound of the picture sizes the frame
   headers and parameter sets define (the readers of `frame-sizes.md`,
   plus VP8 key frames, RFC 6386 9.1). It reads block and frame headers,
   not coded data, bounded by the Matroska limits. `coreav check` runs it
   when a rule needs one of these values; `coreav probe --scan-frame-sizes`
   prints them. Rejected: trusting `Duration` and `PixelWidth` (the same
   understatement problem as ISO sample entries); estimating from `Cues`
   (an index, written by the muxer, also unverified).
3. **Fail closed, as for ISO.** A value the probe and the scan cannot give
   is `unknown`, so the rule rejects: a live recording without a scan, a
   frame header that does not parse, a track whose frames are encrypted
   (the stored bytes are not the frame), a codec without size information
   (MPEG-1/2/4 Part 2, Theora, MJPEG, FFV1 under the default size source),
   a later frame of a lace whose time is unknown (below), a track whose
   frames `coreav-mkv` does not list (bzlib, lzo or a chain of encodings;
   its duration is unknown, not 0). zlib-compressed frames, which some
   muxers apply to PGS and VobSub subtitles, are listed with their
   inflated sizes (`docs/design/mkv.md`), so their track has a
   duration and a frame count; a frame that does not inflate within the
   limits leaves its track not listed. Their content is opaque, as for
   encrypted frames, so no frame header is read from them. A scan that stops
   on the file's structure or a limit leaves every scanned value `null`,
   with a warning; so does a structure the scan cannot account for, a
   truncated file included (decision 8).
4. **Frame times follow the specification, and the codec only where it
   states durations.** A frame starts at its block's time; a later frame
   of a lace starts after the frames before it, which `DefaultDuration`
   times, and without it the codec (RFC 9559 10.3). CoreAV reads one
   codec's durations: Opus, whose packets state theirs in the TOC byte
   (RFC 6716 3.1), which also ends an Opus frame without `BlockDuration` or
   `DefaultDuration`. Browser recordings and Opus muxers often omit
   `DefaultDuration` and some lace. Every other codec's laced frames keep
   an unknown time (so the stream's duration is `null`), and its frames
   without `BlockDuration` or `DefaultDuration` end where they start: the
   span then runs to the last frame's start, one frame short, which the
   file's `Duration` still covers for the duration rule. Rejected: the
   codec syntax of Vorbis (its frame sizes need the whole setup header),
   AAC, MP3, AC-3 and the rest, for a one-frame difference; estimating a
   lace from the next block's time (frames need not end there).
5. **Header-stripped frames are read as decoders see them.** The scan
   reads a frame through its track's stripped bytes followed by the stored
   ones (`ContentCompAlgo` 3), so stripping does not hide a parameter set
   or a frame header. A ProRes frame is read with or without the 8 bytes
   (frame size, `icpf`) that start it in an ISO base media sample: the
   codec mapping does not say which a muxer stores.
6. **`coreav check` allows every codec CoreAV names.** The Matroska probe's
   default allowlist leaves out codecs outside CoreAV's codec scope (DTS,
   MPEG-2 and others); a check takes them all, so that
   `dropped_streams` holds exactly the `CodecID`s CoreAV does not identify,
   as for ISO files, and the policy's `codecs` lists decide the rest.
7. **Exact values up to 2^64.** A stream bit rate over a span in
   nanoseconds has a large numerator (a 3 GB stream: about 10^19). Exact
   rationals in both probe documents now print numerators up to
   2^64 − 1 (previously 2^63 − 1) before falling back to `null`, so the
   bit rate rules of large files are not `unknown`.
8. **The scan audits the structure before trusting the listing.** The
   packet listing stops or skips where the structure is unclear, while a
   decoder may read on: Clusters after a Segment's declared end, elements
   after an EBML header or after an element whose size runs past the file,
   the second Block of a BlockGroup, the blocks of a track a second
   `Tracks` element defines. A listing that stopped there would give a
   span, a byte count and a picture size that look complete. So the scan
   also walks every element header of the Segment, up to the end of the
   file (with the listing, decision 9), and fails on: bytes after the
   declared end; an EBML header or an element of unknown kind among the
   Segment's children; an element that
   runs past its parent or the file (a truncated file is one); a second
   `Info` or `Tracks`; a BlockGroup with a second Block; a block of a
   track no TrackEntry defines; an element whose length its type does not
   allow (RFC 8794 7: an integer or date longer than 8 octets, a float not
   0, 4 or 8 octets long, a fixed-length binary such as `SeekID` of
   another length), checked below every master element of the Segment,
   metadata included, because that is where demuxers resynchronize and
   find Clusters hidden in the element's data; and a string or binary
   longer than the audit allows (`max_element_bytes`, 1 MiB by default;
   64 MiB for an attached file; `max_packet_bytes` for a block or block
   addition), because RFC 8794 bounds neither but demuxers refuse long
   ones (`ffmpeg` 7.0.2, measured as a black box, at 16 MiB for strings
   and 256 MiB for binaries; other demuxers were not measured, hence the
   wide margins) and resynchronize inside them. The kinds come from the
   Matroska EBML Schema (RFC 9559 section 5); Void and unknown IDs, which
   readers skip by size, are not bounded, and block payloads are not
   read. Rejected: scanning skipped payloads for Cluster IDs (a heuristic
   that large attachments trip by chance). A failed scan makes every stream's
   `frames_listed` `false`, which the `invalid_tables` rule rejects by
   default, and its scanned values `null`. Because a hidden `Tracks`
   element escapes the codec and count rules too, `coreav check` scans
   every Matroska file unless `invalid_tables` is `"ignore"` and no other
   rule needs the scan. Rejected: trusting the listing (the bypasses
   above); resynchronizing on Cluster IDs as some demuxers do (a heuristic
   a crafted file can steer); accepting truncated files with the values of
   their complete blocks (damage in the middle of a file reads the same).
9. **The audit walks with the listing.** The audit used to walk
   the whole file before the listing walked it again, so a scan over a
   network read every Cluster twice: 2.0 times the object for `check`,
   where FFmpeg reads it once. Now the audit is a cursor the listing
   drives (`crates/coreav-mkv/src/audit.rs`): before the listing reads an
   element, the audit walks up to and past the same element (a BlockGroup
   child by child), so both read the same bytes within the reader's
   window; the listing reads a Block's header where the Block is, before
   the elements after its payload, so that its reads move forward too
   (an error there still counts after theirs). The two stay independent:
   each keeps its own work count, as before, and the audit still walks
   from the start of the Segment to the end of the file. When the listing
   stops (an error, or the end of the Segment), the audit walks on alone,
   and an audit error, wherever it is, is returned instead of the
   listing's or the scan's own, so on a source that reads the same each
   time every result is the one the two walks gave. Two things differ: a
   source that fails to read is reported as failing, where the audit,
   walking first, could report a structural error found after the failed
   read; and a file the audit rejects near its end now costs the listing
   too (the listing's and the frame readers' work, within their limits),
   where the two walks stopped after the audit's element headers.
   Rejected: one walk with both sets of checks merged into the
   listing (the audit's verdict would then depend on where the listing
   stops, and its rules would mix with the listing's); auditing only up to
   where the listing stops (an audit error after a listing error would be
   lost, changing results); reading the audit from the Cues (they index
   only some blocks; see an earlier change).

## Rule mapping

Paths are JSON pointers into each probe document; `i` is the stream's
position in `streams`, `j` in `dropped_streams`.

| Policy rule | `coreav.probe/1` (ISO base media) | `coreav.probe-matroska/1` |
|---|---|---|
| `file.max_size` | `/file/size` | `/file/size` |
| `file.max_bit_rate` | size × 8 ÷ `/duration` | size × 8 ÷ `/duration`; without `Duration` (live), ÷ the longest `/streams/i/duration` when every stream has one |
| `container.formats` | `/container/major_brand` → `mp4`, `mov`, `3gp` | `/container/doc_type` → `webm` or `matroska` (new format names) |
| `container.fragmented` | `/container/fragmented` (`mvex` and `moof`) | `/container/live` (a Segment of unknown size, written while recording, the Matroska counterpart of a fragmented file) |
| `duration.max_seconds` | the longest of `/duration` and each `/streams/i/duration` | the longest of `/duration` (`Duration`) and each `/streams/i/duration`, the scanned span; a missing `Duration` (live) does not count when every stream's span is known |
| `streams.max_count` | `streams` + `dropped_streams` | `streams` + `dropped_streams` (attachments are files, not tracks) |
| stream kind | the codec's family, else `/streams/i/kind` (handler) | the codec's family, else `/streams/i/kind` (`TrackType`) |
| `codecs` | `/streams/i/codec` | `/streams/i/codec`; Matroska codec names join the list (`vp8`, `vorbis`, `flac`, `mpeg4`, `ac3`, `subrip`, `webvtt` and the rest `coreav probe` prints) |
| `min_count`, `max_count` | per kind | per kind |
| `max_bit_rate` | `/streams/i/bit_rate` (sample bytes ÷ duration) | `/streams/i/bit_rate`, scanned frame bytes ÷ scanned span |
| `max_width`, `max_height`, `max_pixels` | `/streams/i/video/width` (sample entry) and the verified size | `/streams/i/video/width` (`PixelWidth`, declared) and the verified size: `parameter_set_size` from `CodecPrivate` (`avcC`, `hvcC`, `av1C`) or `scanned_size` from the blocks, as `size_source` says; `PixelCrop*` and `DisplayWidth` describe display, not decoding, and are not read |
| `rotations` | `/streams/i/video/rotation` (`tkhd` matrix) | `/streams/i/video/rotation` (`ProjectionPoseRoll`) |
| `max_channels`, `max_sample_rate` | `/streams/i/audio/channels`, `sample_rate` | `/streams/i/audio/channels`, `sample_rate` (`null`, so `unknown`, for a fractional `SamplingFrequency`) |
| `unidentified` | `/dropped_streams/j/fourcc` | `/dropped_streams/j/codec_id` (codecs CoreAV does not name, decision 6); a list entry matches the sample entry type (ISO) or the `CodecID` (Matroska), so list entries are 1 to 64 printable ASCII characters |
| encrypted streams | dropped by the probe (unidentified) | `/streams/i/encoding` `encrypted`, `zlib` or `unsupported`: rejected under `unidentified` like an unidentified stream, whatever the list says; accepted under `"ignore"` |
| `invalid_tables` | `/streams/i/tables_valid` | `/streams/i/frames_listed`: `false` for a track whose frames cannot be listed (`packets_available` is `false`) and for every track when the scan fails (decision 8); `coreav check` scans when the rule is on |
| `external_data` | `/streams/i/external_data` | not applicable: Matroska has no external data references |
| `multiple_descriptions` | `/streams/i/sample_descriptions` | not applicable: one `CodecPrivate` per track |

## Verification and bars (fixed before the corpus run)

- Unit tests for the mapping of every rule, the scan (spans, bytes, VP8,
  laced frames, Opus durations, header stripping, encryption, ProRes,
  truncation, the work limit) and the new readers.
- Fixtures: the Matroska fixtures of `scripts/gen-fixtures-mkv.sh` through
  `scripts/policy_check.py`, with new files: a WebM whose `PixelWidth`
  understates its VP8/VP9 frames, an H.264 Matroska file whose
  `CodecPrivate` SPS is larger than `PixelWidth`, a live recording, a file
  whose `Duration` understates its blocks.
- The Matroska hostile cases (`crates/coreav-mkv/tests/hostile.rs` and
  `scripts/mkv_craft.py`) through `coreav check`: no crash, a verdict.
- Fuzz: `coreav-mkv`'s fuzz project gains a `scan` target.
- Corpus v1, every Matroska and WebM file (the `mkv` and `webm`
  extensions: 423 files, 209 `real` and 214 `edge`; an earlier count of
  225 `edge` files was a miscount, the set is unchanged), three example
  policies:

| Bar | Set | Threshold |
|---|---|---|
| M1 crashes | every file × policy | 100%: exit 0 or 1 within 10 s and 1 GiB, one valid verdict |
| M2 determinism | every file × policy | 100%: two runs print the same bytes |
| M3 spec | every file × policy | 100%: reasons equal `scripts/policy_ref.py`'s over the probe output |
| M4 precision | every reason | 100%: its value is the probe value at its path or its documented derivation |
| M5 oracle | `real` Matroska and WebM files both CoreAV and `ffprobe` read, × policy | at least 98% agree with the verdict `ffprobe`'s output implies, directly or through a predicate-checked entry of `docs/known-divergences-policy.md` |
| M6 bound | every video stream `ffprobe` decodes | 100%: `scanned_size` is not smaller than `ffprobe`'s `width` × `height` |
| M7 speed | held-out `real` Matroska and WebM files | median ratio of the scanning `coreav check` to `ffprobe -v error -count_packets -show_streams` at most 1.00, overall and for each class (`webm` and `mkv` files) |
| M8 memory | every file | peak resident memory of `coreav check` at most 300 MiB |

### One walk

Fixed before the runs, against `main` at fbf5824 (the two walks):

| Bar | Set | Threshold |
|---|---|---|
| W1 same results | every Matroska and WebM file of corpus v1 (423), the hostile suite's cases, and the bug-finding pass's fuzz corpus of the `coreav-mkv` and `mkv_remux` targets; each file whole and cut in half, with the default `max_block_work` and with 65,536, 40 and 7 | 100%: `scan_tracks` returns the same value, or the same error, as on fbf5824 |
| W2 verdicts | every Matroska and WebM file of corpus v1 × the three example policies | 100%: `coreav check` prints the same bytes and exits with the same status as on fbf5824 |
| W3 reads | every Matroska and WebM file of corpus v1, the scan read through `BufferedSource` (the WebAssembly build's 1 MiB window) | no file reads more bytes than on fbf5824; median of the ratio to fbf5824's bytes at most 0.80 |
| W5 native speed | the held-out `real` Matroska and WebM files of corpus v1 | M7 still met: median ratio of the scanning `coreav check` to `ffprobe -v error -count_packets -show_streams` at most 1.00, overall and per class |

Results, in [`docs/measurements/mkv-one-pass.md`](../measurements/mkv-one-pass.md):
W1, W2 and W5 met; W3 not met. Every result and verdict is the
two walks', and the scan reads 0.63 times fbf5824's bytes (median), but
files the audit rejects near their end now pay the listing too (W3).

## Results

The corpus run is recorded in
[`docs/spikes/policy-matroska/`](../spikes/policy-matroska/README.md),
with its evidence in `docs/spikes/policy-matroska/evidence/`. Every bar is met:
no crash, difference or imprecise reason in 1,269 cases (M1 to M4); 100%
oracle agreement on the 627 `real` cases, 97.13% directly and the rest
through KD-P-MKV-UNSIZED, KD-P-MKV-UNLISTED and KD-P-MKV-UNVERIFIED (M5);
no scanned size below `ffprobe`'s in 279 streams (M6); a median speed
ratio of 0.344 against `ffprobe -count_packets` (95% CI upper bound 0.369;
`webm` 0.338, `mkv` 0.375) (M7); and 3,772 KiB of memory at most (M8).


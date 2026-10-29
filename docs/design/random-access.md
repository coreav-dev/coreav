# Random access in the coded data

Status: Accepted

`coreav-isobmff` reports two views of whether decoding can start at a
frame:

- **The container's**, `Packet::keyframe`: the sync sample table (`stss`)
  as stored, or every sample when the box is absent (ISO/IEC 14496-12
  8.6.2). Unchanged, and what remux writes back; `clip` and `split` cut
  only where it and the coded data agree (`docs/design/cut-points.md`).
- **The coded data's**, `access::FrameAccess`, for H.264 tracks: whether
  the frame holds an IDR picture (`nal_unit_type` 5, ITU-T H.264
  7.4.1.2.4), whether every coded slice is an I or SI slice (`slice_type`
  2, 4, 7 or 9, 7.4.3; `intra`), and the `recovery_frame_cnt` of a recovery
  point SEI message (D.2.8), and whether its slices belong to a reference
  picture (`nal_ref_idc`, `reference`). `FrameAccess::starts_decoding`
  combines them into whether decoding can start at the frame, which `clip`
  and `split` use to choose their cuts (`docs/design/cut-points.md`).
  `Movie::access_format` says whether a track
  has it; `Packets::frame_access` reads it for one packet of a listing,
  `access::read_frame_access` for any frame. `access::read_starts_decoding`
  answers `starts_decoding` alone, reading SEI messages only for a frame
  without an IDR picture. With it,
  `Movie::max_num_ref_frames`: how many frames a P or B frame of the track
  may reference, from its sequence parameter set (7.4.2.1.1).
  `access::annex_b_frame_access` answers the same for a frame stored as a
  byte stream (Annex B), from its bytes rather than through the source,
  since such a frame's units are the stretches between its start codes and
  have no lengths to walk; a Matroska H.264 track written without a
  configuration record carries one (`docs/design/thumbnails.md`, "H.264
  without a configuration record"). A caller may pass a prefix of the
  frame, which gives `None` when it ends before the frame's first coded
  slice.

For AV1 tracks the coded data's view is `access::read_av1_frame_start`:
whether the sample holds a sequence header OBU, and whether its first
frame header is a key frame shown at once (`show_existing_frame` 0,
`frame_type` KEY_FRAME, `show_frame` 1, AV1 specification 5.9.2; every
frame under a reduced still picture header). The frame header is read
with the latest sequence header, in the sample or an earlier one, which
the caller carries from sample to sample as a decoder does (7.5).
`access::read_av1_sync` is the AV1 ISOBMFF binding's sync sample (2.4):
both, which `clip` and `split` cut on (`docs/design/cut-points.md`).
`access::av1_config_reduced` reads the sequence header among an `av1C`
box's configuration OBUs, to start that state before the first sample.

## Why both

The two disagree in real files, and both are facts about the file. In the
corpus, `stss` lists the IDR frames and leaves out non-IDR
I frames (five of the six files examined), and one file has no `stss` at
all although only every thirtieth frame is intra; by the container's rule
every sample of that file is a sync sample. A policy gate or a cutter that trusts `stss` alone
cannot see either case; one that trusts the bitstream alone loses what the
file states. So the container's flag stays as stored and the bitstream's
view is reported beside it.

`ffprobe` flags H.264 packets by the coded data. On 22 corpus files
(228,817 frames: with and without `stss`, fragmented or not, Baseline, Main
and High, with and without B frames and recovery point messages, one to
five reference frames), its flag was set on exactly these frames and no
other:

- an IDR picture, or a frame with a recovery point message; or
- a frame whose slices are all intra, when the sequence parameter set
  allows at most one reference frame (`max_num_ref_frames` 0 or 1).

This is a description of `ffprobe`'s flag, not a guarantee that decoding
can start at such a frame: an intra frame that is not itself a reference
picture (`nal_ref_idc` 0) leaves the previous reference frame in use even
with one reference frame, and a frame of an I field and a P field that
references only that I field is a valid starting point though not all its
slices are intra. Whether a frame is a decodable start is the cutting
question of an earlier change, which `docs/design/cut-points.md` settles with
`FrameAccess::starts_decoding`.

The oracle's isobmff adapter therefore compares that with `ffprobe`'s flag
for H.264 (`tools/oracle/SPEC.md`, "Keyframes"), and the sync sample flag
elsewhere. With that mapping the oracle compares the bitstream view, not
`stss`, for H.264; the sync sample flag stays tested by the crate's
fixture tests (`tests/movie.rs`, `tests/fragmented_fixtures.rs`) and, for
every other codec, by the oracle. Where the two views disagree in a way
that would break a cut (a file without `stss`, cut at a P frame), `clip`
and `split` go by the coded data (`docs/design/cut-points.md`). A reorder
depth (`max_num_reorder_frames`) was tried first and
rejected: three files state a depth of 0 with up to five reference frames,
and `ffprobe` does not flag their non-IDR I frames.

## How it reads

Per frame, through the source: each NAL unit's length prefix and header
byte; the first 32 bytes of each coded slice (enough for
`first_mb_in_slice` and `slice_type`, after emulation prevention is
removed); and at most 4,096 bytes of each SEI NAL unit, whose messages are
walked for a recovery point. Coded data is never read. Each NAL unit
charges one unit of `max_sample_work`, the budget of the packet listing it
belongs to. `annex_b_frame_access` takes no budget: it reads the bytes it
is given once, so the caller bounds it by how many it passes (the
thumbnail passes at most 8,192). A frame whose NAL unit lengths run past its end, or whose slice
header does not parse, gives `None` (unknown); the adapter then falls back
to the sync sample flag. So does a byte stream with no start code in it,
or a prefix of a frame that ends before its first coded slice. A zero NAL unit length ends the walk: zero-filled padding would
otherwise cost one unit per length field. Fuzz target:
`crates/coreav-isobmff/fuzz`, `frame_access`. Cost: on the corpus, reading
the view while listing packets took the largest H.264 file (223 MB,
117,000 frames) from 1.14 to 1.25 s. The view is read only on request:
probe and remux do not pay it, and `clip` and `split` read it for the sync
samples they consider and copy (`docs/design/cut-points.md`, "Cost"). Its
speed bar against FFmpeg comes with the library operations' bars.

## Choices and rejected alternatives

- **Keep `Packet::keyframe` as stored.** Rejected: replacing it with the
  bitstream view. What the container states is what remux writes back
  and what a policy compares against the coded data; overwriting it would
  hide the disagreement.
- **Report facts, map in the adapter.** `FrameAccess` reports what the
  frame holds (IDR, intra, recovery point) and the track its reference
  frame count; the rule that reproduces `ffprobe`'s flag lives in the oracle's
  adapter, because it is `ffprobe`'s heuristic, not a property the
  standard defines. Rejected: one `keyframe` boolean in the bitstream view
  (it would bake one tool's heuristic into the API).
- **The mapping's parts.** IDR alone misses non-IDR I frames `ffprobe`
  flags in single-reference streams (five files); `intra` alone flags
  non-IDR I frames `ffprobe` does not flag in streams of more reference
  frames (sixteen files). A frame with a recovery point message but
  non-intra slices does not occur in the evidence; the adapter flags it,
  unconfirmed.
- **Every slice, not the first.** A frame counts as intra only when all
  its slices are. No frame mixing slice types occurs in the evidence, so
  the stricter rule costs nothing there; an I field with a P field that
  references it is reported as not intra, conservatively.
- **In CoreAV, not as a known divergence.** The difference is CoreAV
  reading less than the file holds, not two valid readings, so it is fixed
  by reading more; a known-divergence predicate would have had to parse
  H.264 in the overlay.
- **AV1 in the adapter: a shown key frame, with or without a sequence
  header in the sample.** `ffprobe` 7.0.2 flags AV1 key frames from the
  coded data, whatever the table says (measured on libaom output, a key
  frame every 25 frames of 100: `ffprobe` flags the same 4 packets with
  the encoder's table, with a table listing every sample and with none).
  It still flags a key frame whose sample has no sequence header: that
  OBU turned into a padding OBU of the same size, the earlier sequence
  header applies. So the adapter maps `read_av1_frame_start`'s key frame,
  with the state carried from the `av1C` configuration and each sample,
  and keeps the table's flag where the frame header cannot be read.
  Rejected: the sync sample of the binding (`read_av1_sync`), which leaves
  that frame unflagged. Not measured, since the pinned encoder cannot
  write them: key frames not shown at once (forward key frames, shown
  later by `show_existing_frame`); the adapter does not flag them or the
  frame that shows them. Where the first bits of a frame header are
  altered (to a key frame not shown, an intra-only, inter or switch
  frame, legal values but a frame header the rest of the bits then do not
  fit), `ffprobe` takes the table's flags from that frame to the end of
  the track, also for later key frames (measured on those files with the
  table rewritten to entries 1 to 4, and to 1, 2, 30 and 52). The adapter
  does not model that: it reads each frame's first header byte as it is.
  Should forward key frames or intra-only frames make `ffprobe` do the
  same in real files, every later frame would differ, which a corpus
  recheck shows; none does in corpus v1 (the 64 MP4 and MOV files with an
  AV1 stream, rechecked against the stage 3 gate results for pull request
  187: identical streams and packets).
- **Not read:** HEVC (IRAP pictures) and H.264 in the byte stream format
  (Annex B, some Matroska files) have no keyframe disagreement in the
  corpus and are left for when one shows; redundant coded pictures are not
  told apart from primary ones (that needs the picture parameter set); SEI
  messages after the first 4,096 bytes of a NAL unit are not seen.

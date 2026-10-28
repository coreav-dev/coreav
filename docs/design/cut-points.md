# Cut points of a stream copy

Status: Accepted

`clip` and `split` start their outputs, and their segments, at a video
sample that a decoder can start from. Until an earlier change they took the container's
word for it: a sync sample (`Packet::keyframe`), which is the sync sample
table (`stss`) as stored, or every sample when the table is absent
(ISO/IEC 14496-12 8.6.2). Real files have no table although most of their
frames are P frames (corpus v1, 4c82d16e5037: one IDR picture every 30
frames), or a table that lists frames that are not random access points;
a cut there gives an output whose first frames do not decode. On the
`clip_no_stss*` and `clip_stss_all` fixtures, a clip from 0.5 s started on a
P frame, and `ffmpeg` reported decoding errors.

## The rule

A sample is a cut point when the container marks it as a sync sample
**and** the coded data agrees (`crates/coreav-isobmff-write/src/cut.rs`):

- **H.264**: the frame starts decoding by itself
  (`coreav_isobmff::access::FrameAccess::starts_decoding`,
  `docs/design/random-access.md`), whether or not a sync sample table
  states the flag:
  - an IDR picture;
  - a recovery point SEI message (ITU-T H.264 D.2.8) with
    `recovery_frame_cnt` 0 and `exact_match_flag` 1 (`broken_link_flag`
    concerns decoding that started at an earlier IDR picture, so it does
    not bear on a cut here);
  - or an intra reference picture in a stream whose sequence parameter set
    keeps at most one reference frame, so that no later frame can
    reference one before it.

  If the frame's NAL units cannot be read, it keeps the container's word
  when a table (or a fragment's sample flags) states it. When nothing
  states it, it is no cut point.
- **HEVC**: the sample starts with an IRAP picture (`nal_unit_type` 16 to
  23, ITU-T H.265 7.4.2.2), with or without a table, as the HLS packager
  already requires (`docs/design/hls.md`).
- **AV1, with or without a table**: the sample is a sync sample as the AV1
  ISOBMFF binding defines one (2.4; `access::read_av1_sync`). It holds a
  sequence header OBU, and its first frame header after it is a key frame
  shown at once (`show_existing_frame` 0, `frame_type` KEY_FRAME,
  `show_frame` 1; AV1 5.9.2). Under a reduced still picture header every
  frame qualifies. Such a frame refreshes every reference frame, so
  decoding starts there. A sample whose OBUs cannot be walked keeps a
  stated flag; with no table it is not a cut point. A file that keeps its
  sequence header only in the `av1C` record has no sync sample by this
  definition, so clip and split refuse it. FFmpeg's decoders cannot
  decode such a file from its key frames either.
- **Other codecs**, and tracks whose configuration record cannot be read,
  keep the container's flag.

Leading pictures (decoded after the cut frame, shown before it) do not
make a cut wrong: the output's edit starts at the cut frame's presentation
time, so they are not shown.

A start between cut points snaps to the latest cut point at or before it,
as for any start between sync samples. When there is none, the clip starts
at the first cut point after the start. A track with no cut point at all
is refused before anything is written. Split cuts at the latest cut point
at or before each target.

## The output's sync flags

The container may mark every sample the command copies as a sync sample:
the clip's run, or the whole track for split, which copies every packet.
That is the claim of a missing table, of a table that lists every sample,
and of track fragment defaults. In that case the output marks as sync
only the samples the rule allows. Otherwise the flags are written as
stored, as remux writes them.

- **Why rewrite that claim.** The coded data contradicts it in the files
  that have it. Copied, it gave outputs whose seek points do not decode.
  `ffmpeg` 7.0.2, which takes an HEVC track's flags from the container,
  also left a packet out: the `clip_hevc_stss_all.mp4` clip from 0.6 s to
  1.6 s listed 41 of its 42 packets and decoded 39 of its 40 frames. With
  its 2 IRAP pictures as the only sync samples, it lists all 42 and
  decodes all 40.
- **Why not every table.** A table that lists some samples is the file's
  own statement about random access, which a stream copy writes back.
  Checking every sample it lists would read the whole run once more
  before the copy (see Cost).
- The oracles see the rewritten flags as a difference from the source's
  packets for HEVC, whose `ffprobe` flags are the container's:
  `KD-C-SYNC-VERIFIED` and `KD-S-SYNC-VERIFIED` cover them when the
  flagged packets are exactly the frames `ffmpeg` decodes as key frames.
  For H.264 and AV1, `ffprobe` flags keyframes from the bitstream and
  agrees directly.

## Cost

Per frame checked, the reads are small:

- H.264: each NAL unit's length prefix and header, the first 32 bytes of
  each slice, and at most 4,096 bytes of each SEI NAL unit
  (`docs/design/random-access.md`);
- HEVC: the length prefix and header of each NAL unit, up to the first
  coded slice;
- AV1: each OBU's header and size, and the first byte of the sequence
  header and of the first frame header.

Over a remote source, what costs is reading frames spread across the
file: a read far from the open responses opens a new one
(`docs/design/remote.md`). So frames are read only near the cut:

- **Clip.** A pass over the sample tables, reading no sample data, keeps
  the latest sync samples at or before the start, as many as fit in 24 KiB
  (1,024 of 24 bytes: the sample number, offset and size needed to read
  each frame; fewer when the limits' table budget is smaller). They are checked
  from the latest back in groups of 1, 2, 4 and so on, each group in file
  order (`cut::latest_accepted`): a remote source serves each group from
  one response, and the requests grow with the logarithm of the distance
  back. Then the listing runs from the chosen sample. Past the start, it
  reads coded data only in a run the container marks sync throughout:
  for the sync samples after the end that decide where the run stops,
  and for every sample of the run (the output's flags). In any other run
  the table is believed for where the listing stops, so no frame away
  from the start is read before the copy. Over a remote source, a clip
  then makes one request more than without the check, for the frame at
  the start.
- **Memory.** The clip's memory bar bounds its growth by its output
  tables plus 64 KiB, the copy pass's buffer. The start search runs before
  the copy pass and frees its candidates first, so 24 KiB of candidates
  and the listing's state stay below the copy pass's peak. Measured with
  the clip spike's counting allocator (`cbench-mem`), on clips whose
  queue is full and whose tables are small, an IDR picture every second
  frame and a start near the end of a 2-minute file: 64,115 bytes of
  growth against a bound of 65,676, with no table and with fragments.
  Keeping the whole packet, 144 bytes each, the same clips grew by about
  145,800 bytes.
- **When no kept candidate is a cut point**, the clip never starts on a
  frame that does not start decoding:
  - if every sync sample before the start was kept, none before the start
    is one, and the clip starts at the first cut point after the start,
    checked as listed;
  - if older ones did not fit, every sync sample is checked as listed from
    the track's start, without a queue, and the latest cut point at or
    before the start is taken. This happens only for a track marked sync
    throughout whose cut points are more than 1,024 frames apart (34 s at
    30 frames per second), and the latest 1,024 are then checked twice;
  - with no cut point at all, the clip is refused ("no video sync sample
    to start from").
- **A track with a single cut point at its start** and no sync sample
  table (one IDR picture, then P frames) has every frame after the start
  checked to the end of the track, since none passes: header reads past
  the clip's end, next to a copy that has to start at the track's first
  sample anyway.
- **Split.** The listing records every candidate cut without reading
  coded data. For each target, candidates are checked from the latest at
  or before it back, in the same groups, each once. Only the frames at the
  cuts are read. Each candidate is decided at most once over all targets,
  and `--every` skips the multiples of its step that reach no undecided
  candidate. The work is therefore bounded by the candidates, which
  `max_table_bytes` bounds, whatever the presentation's length and the
  step: a 4.3e9 s presentation cut every second takes as long as its
  three samples (`tests/split.rs`).
- **A track marked sync throughout** also has every sample of each run
  read before that run is copied. From a remote source, a run longer than
  the reader's cache (8 MiB) is then received twice.

On a local file the cost is CPU. The instruction count guard's `clip`
case (`basic.mp4`, `docs/design/benchmarks.md`) counts about 3% more
instructions on the development machine: one more pass over the sample
tables, and the frame at the start. Two choices keep it there:
- **SEI messages are read only for a frame without an IDR picture**
  (`access::read_starts_decoding`). An IDR picture starts decoding
  whatever they hold, and an encoder's SEI messages can be far longer than
  the slice headers: `basic.mp4`'s first frame carries 684 bytes of them,
  which cost twice the rest of the check;
- **the first sync sample after a chosen start is not read** when it is
  presented before the end: it only ends the search for a start. One
  presented at or after the end also counts towards where the listing
  stops, so it is read as the others are; believing it there let a
  P frame marked sync stop the listing before the leading frame of the next
  cut point, inside the range (`tests/clip.rs`, a reordered stream).

Measured with a local HTTP server that honours byte ranges. The input is
a 67 MB, 20-minute H.264 file with an `stss` of 238 entries, and the
same file without the table. The table
gives the bytes the reader received and the requests it made. The server
also counts what it sent; that includes what abandoned responses had in
flight, which on a loopback connection is several MB each.

| Command | Input | Before an earlier change | This rule |
|---|---|---|---|
| `clip --start 900.3 --end 910` | `stss` | 1.6 MB, 4 requests (7.2 MB sent) | 1.9 MB, 3 requests (10.9 MB sent) |
| same | no `stss` | 1.6 MB, 4 requests (7.4 MB sent), cut on a P frame | 1.9 MB, 7 requests (21.6 MB sent) |
| `split --every 300` | `stss` | 67.0 MB, 7 requests (101 MB sent) | 67.1 MB, 11 requests (118 MB sent) |
| same | no `stss` | 67.0 MB, 7 requests (101 MB sent), cuts on P frames | 100.3 MB, 14 requests (142 MB sent) |

## Choices and rejected alternatives

- **Verify with the bitstream** (chosen). Rejected:
  - *trust the container*: the defect of an earlier change. ISO's rule that a missing
    table makes every sample a sync sample is what writers rely on for
    all-intra codecs, and what H.264 files without a table get wrong;
  - *refuse files without a table*: safe, but it refuses files that cut
    correctly at their IDR pictures, and it does not help a table that
    lists P frames.
- **A stated table is held to the same test as no table.** A non-IDR I
  frame does not empty the decoded picture buffer (ITU-T H.264 8.2.5), so
  in a stream that keeps several reference frames, the P frames after it
  may reference frames before it. `clip_stss_all_flash.mp4` has such
  frames: a flash at 2.0 s coded as a non-IDR I frame, 16 reference
  frames, and a table that lists every sample. A clip from 2.0 s that
  started at that I frame decoded none of its frames. It now starts at the
  IDR picture at 0 s, and all 75 frames decode. Open GOPs still cut at
  their I frames where the encoder marks them with an exact recovery
  point, and at IDR pictures otherwise (`clip_open_gop.mp4` cuts at the
  same samples as before).
- **Unreadable frames.** With a stated flag, the container's word stands:
  the file says it, and nothing contradicts it. Without one, nothing
  vouches for the frame, so it is no cut point.
- **Checking lazily, in growing groups.** Rejected: checking every sync
  sample before the start. With a table every few frames, that reads
  across the file up to the start. A remote reader reads through gaps of
  up to 8 MiB, so that meant most of the file for a clip near its end.
  Also rejected: checking one by one from the latest back. Each step back
  is a new request, and a response's read-ahead is lost each time.
- **The output's flags only for runs marked sync throughout.** Rejected:
  copying every flag, which carries a claim known to be false into the
  output. Also rejected: checking every flag of every run, which reads a
  remote split twice for every file with a table.
- **HLS.** The HLS packager reads the bitstream when every sample so far is
  marked sync (`all_sync`), which covers both a missing table and one that
  lists every sample; a table that lists only some P frames is believed
  there. Moving it onto the same rule is a possible later change; its
  fixtures and the new ones pass either way.

## Evidence

Fixtures (`scripts/gen-fixtures.sh`, `scripts/stss_craft.py`), all run by
the clip and split oracles of CI:

- `clip_no_stss.mp4`: H.264, one reference frame, an IDR picture every 25
  frames, its `stss` removed;
- `clip_no_stss_bframes.mp4`: H.264 with B frames and three reference
  frames, `stss` removed;
- `clip_stss_all.mp4`: the same stream with an `stss` that lists every
  sample;
- `clip_stss_all_flash.mp4`: H.264 with 16 reference frames, one IDR
  picture, non-IDR I frames at flashes, an `stss` that lists every sample;
- `clip_hevc_stss_all.mp4`: HEVC, an IRAP picture every 25 frames, an
  `stss` that lists every sample;
- `clip_av1_stss_all.mp4` and `clip_av1_no_stss.mp4`: `split_av1.mp4`, a
  key frame every 12 frames, with an `stss` that lists every sample and
  with none. The probe oracle's adapter flags AV1 key frames from the
  coded data, as `ffprobe` does (`docs/design/random-access.md`), so they
  pass it like the original. The clip and split round trips
  (`fixtures_round_trip_av1_without_a_sync_table`) also take
  `split_av1.mp4` and turn its `stss` into a `free` box. The clips and segments of that copy must start at
  the same samples as the original's, and mark the same samples as sync
  as the original's table does. Before the AV1 check, a file like it
  (libaom, a key frame every 25 frames, a table that lists every sample)
  clipped from 1.5 s started on a frame that is not a key frame: 13 of its
  26 frames decoded, with 29 decoder errors. With the check, it starts at
  the key frame at 1.0 s and all 38 frames decode.

On the first three, a clip from 0.5 s to 1.5 s now starts at the IDR
picture at 0 s. Its first packet is a keyframe for `ffprobe`, and all 38
frames decode without an error. Before, it started on a P frame and
decoded 13 frames, with 2 decoding errors on the B-frame files. All 261
clips and 110 splits of the fixtures pass their oracles, the
`clip_hevc_stss_all` ones through the two known divergences above.

Unit tests:

- `tests/clip.rs`: a start on a P frame without a table and with a table
  that lists it; an exact recovery point as a start; a track with no cut
  point refused; the output's flags;
- `tests/split.rs`: boundaries at the IDR pictures only, and the segments'
  flags; the round trips of both check every output flag against the
  coded data;
- `cut::latest_accepted`'s group order;
- `FrameAccess::starts_decoding` in `coreav-isobmff`.

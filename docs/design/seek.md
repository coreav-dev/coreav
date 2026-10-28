# Seeking in the sample tables

Status: Accepted

A thumbnail at a time needs the packets around that time, but planning one
used to list every packet of the video track. On a 24-minute file whose
thumbnail took 25 ms in-process, planning was 39% of the instructions,
nearly all of it building one record per packet before the time
(`docs/design/decode.md`, bar 5). This note covers:
- how `coreav-isobmff` finds a position in a track's sample tables without
  listing the samples before it (`crates/coreav-isobmff/src/seek.rs`,
  `Movie::packets_from`, `Cursor::at` in `tables.rs`);
- how `coreav thumbnail` plans from the packets around the time
  (`crates/coreav-thumbnail/src/mp4.rs`, `window`).

Specifications: ISO/IEC 14496-12, 8.6.1.2 (`stts`), 8.6.1.3 (`ctts`), 8.6.2
(`stss`), 8.6.5 (`elst`), 8.7.3 to 8.7.5 (`stsz`, `stsc`, `stco`). No
implementation of another parser was read.

## Lookups (`Seeker`)

`Movie::seeker(track)` reads each `stts` and `ctts` entry once. It keeps,
per `stts` entry with samples, its first sample and its first decode time.
With them:
- **`dts(sample)`**: the decode time of a sample, by a binary search over
  the entries and one multiplication;
- **`samples_through(ticks)`**: how many samples have a decode time at or
  before `ticks`; decode times never decrease, so that is the index of the
  first sample after it;
- **`sync_at_or_before(sample)`** and **`first_sync()`**: a binary search
  in `stss`, which validation leaves strictly increasing. Without `stss`,
  every sample is a sync sample;
- **`composition_offsets()`**: the least and greatest `ctts` offset, read
  signed or not as the table says. They bound how far a sample's
  presentation can be from its decode time.

It covers the sample tables only; a track with external data or tables
that did not validate has none.

## Packets from any sample (`packets_from`)

`Movie::packets_from(track, i)` lists exactly what `Movie::packets(track)`
lists after skipping `i` packets. When that listing would stop with an
error before packet `i`, it lists nothing. The contract is the test
(`crates/coreav-isobmff/tests/seek.rs`), and it covers both APIs:
`packets_from(i)` against `packets().skip(i)`, and `for_each_sample` from
`packets_from(i)` against the same listing.

**Position.** `Cursor::at(tables, i)` leaves the cursor exactly as
`Cursor::new` followed by `i` samples would:
- the `stts`, `ctts` and `stsc` entries and the samples left in each;
- the chunk and the samples left in it;
- the byte position: the chunk's offset plus the sizes of its samples
  before `i`;
- the decode time: the `stts` prefix sum;
- the sync-sample cursor: a binary search in `stss`.

It is arithmetic on the run-length entries. Validation (`SampleTables::validate`) has already shown that:
- `stts` and `ctts` cover every sample;
- `stsc` starts at chunk 1, increases and covers every sample;
- the total duration fits in 64 bits.

So positioning cannot overflow. One subtlety: an entry that ends exactly
at `i` is left with nothing remaining and its index on the next entry,
never past it, so the cursor loads the next entry as iteration would.
Entries without samples, and chunks without samples, are passed over as
iteration passes over them.

**The samples before `i` are checked.** Iteration stops at the first
sample that is over `max_packet_bytes`, lies past the end of the file, or
whose offset overflows. Listing nothing after such a sample is part of
the contract, so `packets_from` checks the samples before `i` chunk by
chunk: each chunk's end within the file and each size within the limit.
- With a constant sample size that is one multiplication per chunk.
- With listed sizes it reads them once, as integers, with no record per
  sample. Reading 40,000 sizes costs about what building a few hundred
  packet records did.

Rejected: not checking them. `packets_from` would then list packets that
the full listing never reaches on a truncated or hostile file, and the
thumbnail of such a file would depend on the path taken.

**Composition times.** Iteration also stops where a sample's composition
time (decode time plus offset) overflows. Movie box tables keep the total
duration within 64 bits, so that needs a decode time within one 32-bit
offset of the largest. `packets_from` checks whether its position is
there. If it is, it lists the samples before `i` one by one, and the
overflow stops it where iteration stops.

**Movie fragments.** Samples of fragments before `i` are skipped run by
run. Each run is checked as iteration checks its samples: sizes, the
file's end, and the composition times at both ends of the run. The fragment
headers have to be read anyway, and `packets_from` stays exact for them.
The thumbnail does not use it there (below).

**Budget.** Positioning charges one visit per table entry read, and one
per listed size it checks. That can be more or less than listing the
samples charges: tables with one entry per sample cost more, run-length
tables less. So either can run out of budget where the other does not.
The budget bounds work; it is not part of the contract. The thumbnail
does not depend on it: any error while building a window, a budget that
runs out included, leaves the plan to the full listing (below).

## Planning a thumbnail from a window

`mp4::plan` lists only the packets around the time when the sample tables
can bound the rest. It does so when all of these hold:
- the movie has no fragments;
- the track has no edit list, or one edit, so every presentation time is
  the composition time plus one shift;
- KD-CTTS-UNSIGNED reads no offset as signed;
- the times are known.

Otherwise it lists every packet, as before. It also lists every packet
when anything fails while building the window, the work budget included.
The full listing then gives the result or the error it always gave.

Let `at` be the time asked for, as a composition time in media ticks, and
`[low, high]` the composition offset range:
- **End of the window.** Every sample from `after = samples_through(at -
  low)` on has a decode time past `at - low`, so none is presented at or
  before `at`. The window ends `AFTER` (32) samples later, to hold the
  first frame presented after `at` past any reordering.
- **Start of the window.** Every sample before `whole =
  samples_through(at - high - 1)` is presented before `at`. The window
  starts at the sync sample at or before the last of those, then one more
  sync sample back. The extra GOP makes keyframe mode's choice later than
  everything left out (below).
- **The bounds on what it leaves out:**
  - before the window, no sample is presented later than `dts(begin) +
    high`;
  - after it, none earlier than `dts(end) + low`.

  Decode times never decrease, which makes both hold.
- **The stream's first keyframe** (its colour header) is read separately
  from `first_sync()`.

### Why the choice is the same

Selection (`select.rs`) runs on the window, then the choice is checked
against the bounds (`window_holds` in `lib.rs`). On any error or failed
check, the plan is made again from every packet and selection runs again.
So the thumbnail is always the one the full listing gives, and errors come
from the full listing too. A read the source fails during selection is the
exception: it fails the plan there, because a second pass over the same
source would take a failed read for a frame that cannot start decoding.
The checks:

- **Keyframe mode** takes the latest keyframe presented at or before `at`
  that starts decoding; ties go to the later packet. Packets after the
  window are presented after `at`, so none is a candidate. Packets before
  it are presented no later than `before`. If `before` is at or before the
  chosen keyframe's time, each of them sorts before it and is never
  reached. The extra GOP at the start is what makes this hold. The
  window's first sync sample is presented at least `low` after its decode
  time, and everything before the previous sync sample at most `high`
  after its own. A GOP longer than the offset range, which every encoder
  writes, puts the chosen keyframe after them.
- **Keyframe mode, falling back to the frame at the time** (no keyframe
  of the window qualifies). A keyframe before the window might, so this
  holds only for a window that starts at the first packet.
- **Exact mode** takes the first frame presented at or after `at`; ties go
  to the earlier packet. Packets before the window are presented before
  `at` when `before < at`. Packets after it are presented no earlier than
  `after`, and lose a tie by coming later, so `time <= after` suffices.
  Decoding starts at the latest keyframe at or before the target in
  decode order that is presented no later than it. The window holds every
  packet from its start to the target, so the search finds the same one,
  or fails and falls back.
- **The end of the video.** Selection first checks that `at` is before
  the end of the presented frames. When the window holds a frame
  presented past `at`, the full listing does too. When it does not, the
  window's check fails and the full listing decides.

Rejected:
- Guessing a window from the frame rate and widening it on failure. The
  bounds make the window's validity a check, not an estimate.
- Re-implementing the edit-list mapping for the window. The shift is read
  from the first keyframe's own presentation time, which the iteration
  computes, so the two cannot drift.

## Bars

Fixed before any measured run.

1. **Same choice.** For every MP4 and MOV file with video, the thumbnail
   from a window decodes the same packets and the same frame as the
   thumbnail from every packet, in both modes, at every time checked; an
   error must match too (`scripts/seek_check.py`, which compares the first
   and the target packet's offsets, the frame's time, the packet count
   and the first keyframe):
   - the fixtures at 64 times spread over each file plus the edges, in CI:
     100%;
   - corpus v1's development-set MP4 and MOV video files at 16 times
     each: 100%.
2. **`packets_from`.** Its property test passes in CI on generated, hostile
   and fragmented tables, and on the fixtures.
3. **Speed.** S1 and S3 of `docs/design/thumbnails.md` on corpus v1's
   development-set MP4 and MOV files, as a development measurement
   before and after this change, same host, same method. Each is reported overall, per
   class, and split by FFmpeg's own time: under 50 ms, 50 to 300 ms,
   over 300 ms. The bars are the thumbnail bars, unchanged. All 611
   such files are timed, for S1 and S3 only (`speed.py --ext mp4,mov
   --bars S1,S3`). No claim that CoreAV is faster than FFmpeg rests on it.
   A fresh held-out draw (corpus v2-heldout draw 4, the thumbnail video
   classes) confirms this change and the conversion change of
   `docs/design/decode.md` (bar 5, item 2) together. That happens once both are
   merged and the bars of that joint run are committed.
4. **Instructions.** On the 24-minute file profiled in
   `docs/design/decode.md` (bar 5), planning's share of the in-process
   thumbnail's instructions is reported before and after.

## Results

`docs/spikes/thumbnail-bench/`, "Planning from a window".

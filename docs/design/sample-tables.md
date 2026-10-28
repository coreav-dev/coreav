# Walking the sample tables

How `coreav-isobmff` turns a track's sample tables (ISO/IEC 14496-12, 8.6
and 8.7) into packets, and why there are three ways to read them. The
tables themselves — how they are parsed, bounded and validated — are not
covered here, nor are
sample runs.

## Three ways to read the samples

A track's samples are always the same samples; what differs is how much a
caller is handed at a time.

- **`Packets` as an iterator**, one `Packet` per sample, with the
  presentation times the edit list gives it. For callers that want one
  sample: `coreav clip`'s cut points, a frame lookup.
- **`Packets::next_run` and `next_runs`**, one `SampleRun` per stretch of
  samples that lie back to back in one chunk or one fragment run and share
  size, decode duration, composition offset and sync flag.
  For callers that need no presentation time and whose work is per run:
  a constant-size PCM track is indexed without visiting each sample, and a
  fragment run that declares billions of samples costs one step.
- **`Packets::for_each_sample`**, every sample as the tables store it,
  handed to the caller's own closure. For callers that visit every sample
  anyway: the packet index (engine A-index) and the
  HLS index's per-sample records.

All three list the same samples with the same checks and the same errors,
and the first two charge the same work for them; the third charges a
movie fragment's samples a run at a time, for the reason below.
`tests/runs.rs` compares them against each other on every generated
table, on seeded mutations of them, under budgets that run out mid-track,
and on real files (`COREAV_RUNS_FILES`).

## Why a third way

Runs are the right shape when the tables repeat themselves. On
constant-rate AAC they do not. Profiled on draw 1 of
[`v2-heldout`](../corpus.md#v2-heldout), which is a
tuning pool, every audiobook in it has:

- `stts`: **one** entry, 1,024 ticks per sample;
- no `ctts` and no `stss`, so every sample is a sync sample at its stored
  decode time;
- `stsz`: listed, with 405 to 693 distinct sizes over half a million to a
  million samples. Consecutive sizes differ almost always: the mean run of
  equal sizes is **1.02 to 1.03 samples**;
- `stsc` and `stco`: 7 to 44 samples per chunk, and up to 97,857 `stsc`
  entries.

So a run is one sample, and a caller that indexes every sample paid for the
run anyway: the walk built a 48-byte record per sample, pushed it into a
vector, and the caller read it back to write its own. A profile of the
packet index on that pool put 60% of the time in the walk and 29% in the
caller's loop over the vector; a hand-written loop that reads the three
tables and writes one 24-byte entry per sample ran at 2.3 to 3.5 ns per
sample against the crate's 14 to 26.

`for_each_sample` removes the record between them. The caller's closure is
inlined into the walk, so nothing per sample reaches memory but the
caller's own output, and the walk itself is reorganized around a **span**:
once the chunk, the decode-duration entry, the composition-offset entry and
the next sync sample are settled, the samples until the next of those share
everything but their size, so the span's sizes are read from one slice of
`stsz` and the byte offset and decode time advance arithmetically. Only a
chunk boundary, an entry boundary or a listed sync sample costs more than a
size.

### What is identical, and how it is kept so

The hard requirement is that nothing a caller can observe changes. Three
things make that checkable rather than hoped for:

1. **The samples.** A span's samples are exactly the runs of `next_run(1)`
   expanded. `tests/runs.rs` asserts it on every generated case.
2. **The errors and their order.** Per sample the span checks, in order,
   the byte offset for overflow, the decode time for overflow, the size
   against `max_packet_bytes`, and the end of the sample against the file —
   the order the general step and `checked_run` produce between them. The
   tests compare the error, as text, as well as the samples.
3. **The work budget**, for the samples of the sample tables. One visit
   per sample, charged before the sample is read, plus one for the step
   that finds the end: exactly what iterating packet by packet charges,
   down to the sample a budget runs out on. The tests compare
   `Budget::used` under budgets of 3, 5, 8, 13 and 21 visits as well as
   the default.

Where a span cannot be taken — the sizes are constant, the table does not
hold the span's sizes, or the general step rejects the tables — the walk
falls back to one run per sample through the general step, which is the
same code the other two ways use.

**Movie fragments are the exception, and deliberately.** A `trun` run
without per-sample fields is computed, not scanned, so a run that
declares as many samples as the file can back costs one step however many
it declares. Expanding such a run sample by sample
would throw that away: the walk would take one step per declared sample,
which `coreav hls index` reads from untrusted input. So a fragment run is
taken whole and its samples are charged before any of them is expanded —
what `hls::list` did before this walk existed, and what
`Packets::charge` documents for a caller that takes maximal runs. The
price is that a budget stops a fragmented track at a run boundary rather
than at a sample, and that such a track is charged the run's own step
beside its samples. `tests/runs.rs` keeps the strict comparison for the
sample tables and, for fragments, checks the prefix, the completion and
the direction of the work; another test holds a `trun` of 300,000 samples
to one step under a budget of 100,000.

Fragments gain less from the walk than the tables do, since the run
machinery was never the cost there, but they do not lose: `coreav hls
index` is 0.5% to 1.3% fewer instructions on the three fragmented
fixtures.

A span was also tried inside `next_runs`, which every other caller of the
tables uses. It cost `coreav hls index` 7.4% of its instructions on the
`hls_h264.mp4` fixture and gained the packet index nothing, since the
packet index no longer goes through runs; it is not in the change.

### The HLS index

`hls::build_index` visits every sample too, so it lists them with
`for_each_sample` as well. Its per-sample record (the segment boundaries,
the sample's flags, size and composition offset, the presentation mapping)
is unchanged; what goes away is the run record between the table and that
work, and the per-batch charge, which becomes the walk's per-sample charge.

### QuickTime compressed audio counted in audio samples

A QuickTime version 1 sound description of a compressed format (QDM2,
IMA 4:1 and others) may come with an `stsz` of 1 and one table sample per
audio sample: `samplesPerPacket` audio samples make one compressed frame
of `bytesPerFrame` bytes (QuickTime File Format, sound sample
descriptions). The tables' one byte a sample then says nothing about
where the bytes are; they are the frames, `ceil(samples / samplesPerPacket)`
of them back to back from each chunk's offset. Checking each run's
nominal bytes against the file rejected valid files whose last chunk has
fewer bytes than samples, as QDM2 files usually have.

So when `Movie::packets` makes the iteration of such a track (without
movie fragments), it walks the chunks once, one visit each charged to the
work budget, and checks that every chunk's frames lie inside the file and
that a frame is within `max_packet_bytes`. If they do, the tables'
nominal ranges are not checked against the file by any of the three ways
or by `packets_from` (the overflow checks stay); if not, the nominal
checks stay, so the track still stops as truncated. The runs keep the
tables' units: a caller that wants the frames, as `coreav ffprobe` does,
groups them from the chunk's offset. Rejected: rewriting the tables into
one sample per frame, as uncompressed audio is rewritten, which changes
the samples every reader sees and needs whole frames in every chunk; and
carrying each run's chunk offset to check the frames run by run, which
widens the run every hot loop moves for a codec the library does not
decode.

### Subtitle samples in fragments

A fragment states each sample's sync flag (`sample_is_non_sync_sample`
in `trun`, `tfhd` or `trex`, ISO/IEC 14496-12 8.8.3.1), and the pinned
`ffmpeg` marks the empty samples between cues of a `mov_text` track as
non-sync. `ffprobe` lists every sample of a subtitle track as a key frame
whatever the flags say (`fragmented_text.mp4`), and a cue does not depend
on the cue before it, so the fragment walk reports every subtitle sample
as a sync sample. Video and audio keep their flags. Rejected: keeping the
flags and recording a known divergence, which would report as needing an
earlier sample a cue that needs none.

## Bars

Fixed here before any measured run (the project's pre-registration rule).

- **Correctness.** Byte-identical output: `coreav hls index` and the
  packet index (the A-index digest of `docs/spikes/safe-probe/`) agree with the
  base commit on every fixture of `scripts/gen-fixtures.sh` and on every
  corpus file measured. The HLS packaging oracle
  (`scripts/hls_check.py`) agrees on every fixture it packages, with no
  new divergence.
- **Speed, against libavformat**, on a fresh held-out draw
  of corpus v2-heldout, over the inputs both engines open: the median of
  the per-input ratios is at most **1.00** overall and in each class with
  at least five inputs. Both engines carry it: the packet index
  (A-index) and the HLS index (A-hls). The group-a class of A-hls is
  expected to miss again; a miss is recorded with its cause and the bar
  does not change (`docs/design/hls.md`).
- **Speed, against the base commit**, paired on the same held-out inputs
  and in the same run, as a fifth and sixth engine: the median of the
  per-input ratios new / base is at most
  - **0.60** for the packet index in the group-a class,
  - **0.85** for the packet index overall,
  - **0.90** for the HLS index in the group-a class,
  - **1.00** for either engine overall and in every class.

  The two binaries differ in code layout, so a class median between 1.00
  and 1.02 is reported as unchanged rather than as a regression; above
  1.02 is a miss. A class with fewer than five inputs is reported without
  a verdict.
- **Share.** Every speed median is reported with the share of held-out
  inputs CoreAV is faster on.
- **CI regression guard.** No case of `scripts/bench_guard.py` above its
  base commit's count by more than the 5% tolerance
  (`docs/design/benchmarks.md`).

## Results

Run `w4-01` on draw 3 of corpus v2-heldout, 46 held-out files, 60
rounds after a warm-up, five engines on one core
(`docs/spikes/w3-308/`). **Every bar is met but one, which was
pre-registered as expected to miss.**

- **Correctness: met.** The packet index's entries and what `coreav hls
  index` writes are identical between the two builds on all 46 files,
  including the files either refuses. The HLS packaging oracle agrees on
  every fixture it packages (64 agree, 3 covered by a known divergence,
  0 fail, 17,136 packets agree), and no case of either CI guard is above
  its base by more than the tolerance: natively `hls index` is −7.6% and
  `hls package` −4.6%, with every other case inside ±1.1%; in the
  WebAssembly module `hls index` passes 27,046 execution points against
  34,017 (−20.5%) and every other case is unchanged to the point.
- **Against libavformat: met.** The packet index is at **0.269** of
  libavformat's open overall (95% interval 0.257 to 0.306) and under
  1.00 in every class — `group-a` 0.352, `group-c` 0.271, `group-g` 0.261 —
  with every interval's upper bound below 1.00, so it is *faster* in
  each. CoreAV is faster on **42 of the 42 inputs** (100%). The same
  engine built from the base commit measured 0.344 overall and 0.850 on
  `group-a`, whose interval reached 1.033: "faster than libavformat" did
  not hold for that class before and does now.
- **Against the base, paired on the same inputs: met.** The packet index
  takes **0.833** of what it took overall (bar 0.85) and **0.412** on
  `group-a` (bar 0.60); it is faster on 35 of the 42 inputs. The HLS index
  takes **0.907** overall (bar 1.00) and **0.805** on `group-a` (bar
  0.90), faster on 30 of 32. One class sits in the band the bar allows
  for the two binaries' code layout: the packet index on `group-g` at 1.003,
  reported as unchanged, where six of eleven inputs are read from a
  segment index and the index is built without walking a sample.
- **The HLS index against libavformat: 0.622 overall, and `group-a`
  misses at 1.547** (the base measured 1.991). The miss was
  pre-registered as expected and is recorded with its cause in
  `docs/design/hls.md`; what is left above 1 is the index's own
  per-sample record, 4.370 times the walk on that class, not the walk.

The gain tracks how much of a request the walk is: `group-a`, whose tables
are one AAC track, 0.412; `group-g`, where six of eleven inputs are read
from a segment index, 1.003. The overall change is the number the
measurement holds least tightly — 0.833 against a bar of 0.85, where
three 15-round runs put it between 0.812 and 0.836, which is why the
reported run is 60 rounds and every run is in the spike's record. The
number the change is about, `group-a`, is clear of its bar by far more
than that spread in every run.

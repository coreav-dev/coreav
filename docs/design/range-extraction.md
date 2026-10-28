# Audio extraction over range reads

Status: Accepted. Scope: `extract_audio` over a remote source, in `coreav extract-audio URL`
(`docs/design/remote.md`).

## What it does

Extracting the AAC track of an object read by byte ranges fetches the
audio samples' bytes and skips the other tracks' samples between them,
where skipping is faster than reading through.

Before, extraction advised the reader of one extent, from its first audio
sample to the end of its last (`Source::will_read`), and the reader's
pipeline read the whole extent: about the whole object for a file that
interleaves its tracks.

## Design

### The copy pass says exactly what it reads

`extract_audio` lists its samples once to build the output's tables
(pass 1) and once more to copy them through one 64 KiB window (pass 2). During pass 1 it now also lists the reads pass 2
will make (`copy::ReadPlan`): the same samples in the same order, through
the same window decisions (`holds` and `refill_range`, shared with
`Window::copy`), without reading. Each refill is one region, a sample
larger than the window is one region, and regions that touch or overlap
are joined. Before pass 2 the source gets the list
(`Source::will_read_regions`, added for the frame-size scan); after pass 2,
an empty list ends the advice, so that whatever reads the source next goes
anywhere.

- **The regions are the reads, not the samples.** A window refill reads
  64 KiB from a sample's start, past the end of a short chunk into the
  next track's bytes. Advising only the samples would make every such
  refill run past its region into a gap, which a reader then serves with
  a request of its own. Advising the windows keeps pass 2's reads, and so
  its output and its local performance, exactly as they were.
- **Only where it holds.** The list is given only to a source that asks
  for it (`wants_regions`: remote sources), only for movies without
  fragments (pass 2 would also read the fragments' boxes), only up to
  65,536 regions (1 MiB; the frame-size scan's bound), and only while each
  region starts at or after the one before (a pass that goes backwards is
  left to the extent). Otherwise the extent advice of before applies.
- Rejected: **coalescing in the writer.** Whether a gap is worth skipping
  depends on the transport (round trip, bandwidth, concurrency), which
  only the reader knows; the writer states what it reads.
- Rejected: **a new advice interface.** `will_read_regions` and
  `wants_regions` already exist for the scan of ProRes and VP8 headers.
- Rejected: **clipping the copy window at a chunk's end** so that the
  regions are the samples exactly. It changes the local read pattern of
  every extraction for a few kilobytes per chunk.

### The readers skip what is worth skipping

The reader plans advised regions in `coreav-range`'s
`Reader::will_read_regions`, which this issue adds for `coreav … URL`.

- Regions closer than `sparse_gap` are joined; larger gaps
  are skipped. Then the spans must fit half the requests left; if not, the
  gap doubles until they do, and a single span that still does not fit
  gives no advice.
- One pipeline requests the spans in order, `parallel` segments in
  flight, within the same memory bounds as a forward pass.
- A response already open at the first span's start (the probe's first
  response) serves the spans as far as its reads reach, and the pipeline
  starts after it, as extent advice already did. The CLI's first response
  asks for 32 MiB and often holds a small object whole; without this rule,
  an 18 MiB tuning file's regions were requested again.
- A read outside the spans while the advice holds asks for its own bytes
  only: one request, exactly the read, never a new forward pipeline.
- A read in a span not requested yet moves the pipeline there, skipping
  the spans in between. A read behind the pipeline's reader takes the
  usual path, so that advice never ended costs a later pass one bounded
  pipeline rather than a request per read.
- In `coreav-range`, a segment the reader has passed without reading
  releases its connection; its missing bytes are requested again only if
  a read goes back for them.

**The gap threshold.** Skipping a gap of `g` bytes saves `g / B` of
transfer, where `B` is the bandwidth of the requests in flight together.
It costs one more request: a round trip `L` on one of `k` connections,
about `L / k` of the pass's time. Skipping pays above `L × B / k`. That
point moves with the transport:
- On this host against R2 (`L` 275 ms, `B` about 60 MB/s, `k` 4) the point
  is about 4 MB. On the tuning files (`bench/range-audio`, "Tuning"), 2 MiB
  lost on a recording whose gaps are 2 to 4 MiB: 32 requests of
  1.6 MB, 1.22 times FFmpeg's time against 0.73 for the extent. 4 MiB kept
  the large file's gain: 4.4% of the object, 0.25 against 0.37.

So the gap is set per session, `sparse_gap` unless
the caller fixes it: `L` is the wait for the session's first response as
the reader measures it, and `B` is the setting `bandwidth`, 60 MB/s. `B` is
an assumed value, not a measurement: a pass measures its bandwidth only
once it reads, after the gap must be chosen. The gap is kept within 1 to
8 MiB. A host that measures no wait (a `coreav-range` host may not) gets
4 MiB. The gap distribution of the tuning files is in
`bench/range-audio/README.md`.

**The router.** Joining every gap gives one span from the first region to
the end of the last: the extent pass of before. So the threshold is the
router between range reads and a whole read, applied per gap: a
low-bitrate file whose gaps are all under it is read straight through;
only gaps worth a request are skipped.
- Rejected: **a separate router** in the style of the adaptive H7 router, predicting
  range-read time against a download per object. It would choose between
  the same two plans with the same inputs, and it would decide once per
  object where the gaps of one file differ.

### Effects beyond extraction

- `coreav`'s URL source now asks for region advice (`wants_regions`), so
  the frame-size scan (`probe --scan-frame-sizes URL`, and `check URL` with
  a policy that needs it) also reads only the headers of ProRes and VP8
  samples natively. The same plan and gap apply.
- The connection fixes of `coreav-range` apply to pipelines over advised
  regions only; forward passes of `clip`, `remux` and the others read as
  before.

### Known limits

- **A sample larger than the window can read 4 KiB past its region.** It
  is copied in 64 KiB pieces; a last piece under 4 KiB goes through
  `BufferedSource`, whose smallest fill is 4 KiB, so the read can run past
  the sample's end, outside the advice, and cost one request. AAC frames
  are far smaller than 64 KiB.
- **The wait includes connection set-up.** The first response's wait
  covers name resolution and the TCP and TLS handshakes of a cold
  connection, so it overstates the round trip, and the gap leans towards
  reading through (within the 8 MiB clamp). That costs part of the gain,
  never more time than the extent.

### Measurement code

`bench/range-audio`: `sets.py` draws the sets, `run.py` measures (with the
helpers of `bench/shared`), `summarize.py`
computes the statistics below.

## Bars (fixed before the held-out run)

**Inputs.** The per-input lists stay private. The population is corpus v1's
`real` objects tagged `c:isobmff` and `a:aac`. The held-out set is drawn
from the population that corpus v1 holds out, files
no development of the readers has used. It is stratified, and within each
stratum ordered by `sha256("coreav-corpus-v1-holdout-issue31:" + sha256)`:
- 24 objects with video of at most 64 MiB;
- 24 objects with video over 64 MiB;
- 12 audio-only objects.

That is 60 objects, 17.0 GiB, 26 of them over 64 MiB, the largest
3.1 GiB. Development used the 10 tuning files (`tuning.json`), all from
the population that corpus v1 does not hold out.

**Method.** All on the development host (8 vCPU, Linux), reading a
presigned URL of the object in R2:
- `coreav extract-audio URL out.m4a` (profile `dist`);
- FFmpeg 7.0.2: `ffmpeg -nostdin -v error -i URL -map 0:<the stream the
  native build extracts> -c copy -f mp4 -y out.m4a`.

Per object, the engines run in blocks, in an order rotated by the
object's index. The CLI's rows may come from two runs over
the same objects (`run.py --engines`), each measuring the final code of its
path, once, with its own FFmpeg block; each run records its commit. Each block is one discarded warm-up run, then three timed
runs. An engine's time is the median of its runs: the wall time from
outside, with process start-up for FFmpeg and the CLI. Each output is checked against the native build's
(`tools/edge-native`) on a SHA-256-verified download. Classes:
- by content: audio-only, and with video;
- by size: at most 64 MiB, and over 64 MiB.

Ratios cover the objects that the native build extracts and on which
FFmpeg succeeds in every run; the others are listed with the reason.
Statistics: medians over objects, with bootstrap 95% intervals over
objects (10,000 resamples).

**Bars.**
- **E-1 equality.** Every CLI output of every run equals the
  native build's.
- **E-3 CLI against FFmpeg.** The same for `coreav extract-audio URL`: the
  median per-object ratio of its time to FFmpeg's is at most 1.00, overall
  and in each of the four classes. "Faster" is said only
  where the upper bound of the 95% interval is below 1.00.

**Reported without a bar.**
- Bytes received as a share of the object, and requests, for the CLI.

## Results

**The final run.** `cli-01` measured a commit before the last change to
`coreav-range` (a read past the open response asks for its own bytes)
and before this branch was rebased onto five days of `main`. The rows
reported against E-1 and E-3 therefore come from one run,
`final-01`, on the rebased branch's final code, three timed runs per
block as above. `cli-01` stays as evidence of the earlier commit; where
the two differ, `final-01` decides E-3.

### `final-01`: the CLI

Run `final-01` measured `c070360`, this branch rebased onto `main`, from
a clean tree, on the 60 held-out objects, in one pass without a stop.
Evidence: `bench/range-audio/evidence/final-01/`, `summary-final-01.json`.

- **E-1 is met.** Every output of the CLI, in every
  run, warm-ups included, equals the native build's. Three objects are
  excluded because the native build refuses their track (they are refused
  by the CLI too).
- **E-3 is met**: the median per object is at most
  1.00 overall and in every class.

| Class | Objects | CLI / FFmpeg, median (95% CI) |
|---|---:|---|
| **All** | 57 | **0.777** (0.618 to 0.829) |
| Audio-only | 9 | 0.814 (0.327 to 1.033) |
| With video | 48 | 0.748 (0.579 to 0.829) |
| Up to 64 MiB | 31 | 0.579 (0.424 to 0.795) |
| Over 64 MiB | 26 | 0.831 (0.703 to 0.910) |

Extraction is faster than FFmpeg reading the same URL overall and in
every class but audio-only, whose interval reaches past 1.00: there it is
on par, for the CLI. Seven objects are slower for the CLI (1.00
to 1.79).

**Bytes and requests.** The CLI received the whole
object at the median (median share 1.00). The CLI made 4 requests at the
median and 198 at most. The CLI received
less than 95% of two objects, the smallest
share 1.3%. As in the tuning, at this host's round trip most files' audio
chunks lie closer together than a request is worth, and the speed comes
mostly from reading through in parallel.

**Against a download** (reported without a bar). The harness downloads
each object before the native check: 16 connections of 8 MiB parts, with
the SHA-256 check, and no extraction. The CLI's time over that download,
medians: 0.42 up to 64 MiB (31 objects), 2.13 over 64 MiB (26).

Up to 64 MiB,
extraction over range reads takes less time than the download alone.
Over 64 MiB, the download is faster, as the extraction reads through the
object on four connections.

### `cli-01`: the CLI at an earlier commit

Kept as evidence; `final-01` decides E-3. Run `cli-01` measured
`measured/range-audio-cli` at `2fa4878`: this branch merged with `main`, from a clean tree, on the 60 held-out
objects. The run stopped once, at the 38th object, when its launching shell
ended; it was resumed into the same output at the same commit, and each
object was measured once. Evidence: `bench/range-audio/evidence/cli-01/`,
`summary-cli-01.json`.

- **E-1 is met.** Every output of every run, warm-ups included, equals the
  native build's. Three objects are excluded because the native build
  refuses their track (the CLI refuses them too).
- **E-3 is met:** the median per object is at most 1.00 overall and in
  every class.

| Class | Objects | CLI / FFmpeg, median (95% CI) |
|---|---:|---|
| **All** | 57 | **0.725** (0.612 to 0.806) |
| Audio-only | 9 | 0.870 (0.444 to 1.072) |
| With video | 48 | 0.683 (0.600 to 0.782) |
| Up to 64 MiB | 31 | 0.607 (0.444 to 0.848) |
| Over 64 MiB | 26 | 0.769 (0.689 to 0.800) |

The CLI is faster than FFmpeg reading the same URL overall and in every
class but audio-only, where the interval reaches 1.072 and it is on par.
Six objects are slower, between 1.03 and 1.19: five of at most 52 MiB,
where one round trip more or less decides, and one of 1.2 GiB at 1.09.

**Bytes.** The CLI received the whole object at the median (median
share 1.00, smallest 1.3%), in 4 requests at the median and 198 at
most. At this host's round trip most files' audio chunks lie closer
together than a request is worth, and the first response's wait, which
includes connection set-up, puts the CLI's gap near its 8 MiB clamp.
The speed against FFmpeg comes mostly from reading through in parallel.


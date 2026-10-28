# The pinned memory bounds

Two of the three pinned limit tests bound allocation, and neither had been
measured before this run: peak memory growth under 32 MiB while a 2.2 GB
fragmented file is demuxed, and a sample count of 2^32 − 1 with a tiny size
table rejected without allocating. The bars, the inputs and the harness are
in [docs/design/limits.md](../design/limits.md), committed at `c120c3e`,
before this run.

**Every bar passed**, with margins from 50× on the widest path to about
35,000× on the narrowest. The widest peak of any path over a 2.2 GB file is
671 KB, 2% of the 32 MiB bar. Every rejecting call allocated under 4.6 KB,
and allocated exactly the same bytes whether the file declared 2^32 − 1
entries or 65,537 — so nothing follows a declared count. One observation
that is not a bar is at the end.

## Setup

- The measured code is the tree of this page's own commit, on
  `task/220-limit-bounds`. The bars were committed at `c120c3e`, before
  the run; the evidence below is re-made from the branch tip whenever the
  tests change, and gave the same numbers every time until the fix for
  an earlier change, whose run is
  under "Closed by an earlier change" with the fifteen numbers it moved.
- Host: 8 vCPU Intel Xeon at 2.90 GHz, Linux 6.18.49 x86_64, `rustc`
  1.99.1. No infrastructure spend.
- The same run on a GitHub `ubuntu-24.04` runner, through the CI step
  below, produced all 33 numbers **byte for byte identical**: none of them
  depends on the host.
- Command, which is also the CI step:

      cargo test --release -p coreav-isobmff -p coreav-isobmff-write \
          --test memory -- --include-ignored --nocapture --test-threads=1

- Metric: requested heap bytes through the test-only counting global
  allocator (`tools/counting-alloc`), per measured region and per thread.
  Peak is the high-water mark of bytes live inside the region, so it is the
  growth the call caused, not the process's resident set. Every region's
  **trough** — its low-water mark — is zero in all 33 measurements, which
  means no call freed memory allocated before it, so no peak below is a
  number that an earlier free offset. Each test asserts that, and that the
  region made at least one allocator call, so a bar cannot pass by
  measuring nothing. A reallocation counts as holding its old and its new
  block at once, so a peak is never below the truth.

## Inputs

| File | Bytes | Shape |
|---|---|---|
| Fragmented ProRes | 2,202,160,587 | 20,000 fragments × 10 intra samples of 11,000 bytes |
| Fragmented H.264 + AAC | 2,318,481,027 | the same video shape with a sync sample every 25, plus 19 AAC samples of 300 bytes a fragment |
| Hostile declarations | under 2,048 each | one table's entry count set to 2^32 − 1 or 65,537, four entries present |

## Bar 1: under 32 MiB on a 2.2 GB fragmented file

| Path | Peak growth | Bar | Margin |
|---|---|---|---|
| `probe` | 7,793 B | 32 MiB | 4,306× |
| `scan_fragments` + 200,000 packets | 960 B | 32 MiB | 34,953× |
| `probe` + scan + full listing | 8,656 B | 32 MiB | 3,876× |
| `extract_audio` (114 MB written) | 74,335 B | 32 MiB | 451× |
| `clip_mp4` of the whole file (2.31 GB written) | 108,239 B | 32 MiB | 310× |
| `split_mp4` into 134 one-minute segments | 671,032 B | 32 MiB | 50× |

**Pass.** No read exceeded 1 MiB (`MAX_READ`) on any path, which each test
asserts.

## Bar 2: memory does not follow the file's size

| Measurement | 20 fragments | 200 | 20,000 |
|---|---|---|---|
| Demux peak growth | 8,656 B | 8,656 B | 8,656 B |

**Pass**, and exactly flat, not merely within the 1 MiB of slack the bar
allows: demuxing holds nothing per fragment.

A stream copy must hold the output's sample tables, which are O(samples) by
the format's definition, so the bar is that it must not follow the file's
*bytes*. With ten times smaller video samples and the same sample count:

| Path | 11,000-byte samples | 1,100-byte samples |
|---|---|---|
| `extract_audio` | 74,335 B | 74,335 B |
| `clip_mp4` | 75,495 B | 75,495 B |
| `split_mp4` | 115,712 B | 115,712 B |

**Pass**, byte for byte, while the source grew from 3.39 MB to 23.19 MB
(200 fragments; the 20,000-fragment files are in bar 1).

## Bar 3: rejected without allocating

Each row is one file whose table declares the count in the heading and holds
four entries. The same file with its real count parses and lists its four
packets, which every row checks first, so a row cannot pass by failing
somewhere earlier. Both patched counts are rejected; the numbers are the
whole `probe` + packet listing call.

| Declared count in | 2^32 − 1 | 65,537 | Equal |
|---|---|---|---|
| `stsz` sample count | 177 B peak, 590 B obtained, 21 calls | 177 B, 590 B, 21 | yes |
| `stco` chunk count | 177 B, 618 B, 23 | 177 B, 618 B, 23 | yes |
| `stsc` entry count | 177 B, 562 B, 19 | 177 B, 562 B, 19 | yes |
| `stts` entry count | 177 B, 490 B, 15 | 177 B, 490 B, 15 | yes |
| `ctts` entry count | 177 B, 514 B, 17 | 177 B, 514 B, 17 | yes |
| `stss` entry count | 177 B, 634 B, 25 | 177 B, 634 B, 25 | yes |
| `trun` sample count | 4,535 B, 5,822 B, 30 | 4,535 B, 5,822 B, 30 | yes |

**Pass.** Identical across a factor of 65,536 in the declared count, and
every peak is under the 64 KiB bar. The smallest table any of these counts
would need is 16 GB, so the margin against an implementation that believed
the count is about 10^8.

Other rejecting inputs, same bar:

| Input | Peak growth |
|---|---|
| 10,000-level nesting | 24 B |
| 10,001 top-level boxes | 24 B |
| a box claiming 4 GiB inside a 1,010-byte file | 28 B |

## Bar 4: hostile fragmented input

A file of 2,000 fragments read under `max_fragments: 8` is refused at the
ninth with 8,656 B of peak growth — the same as a complete demux, so
nothing is held per fragment already passed. **Pass.**

## One observation, not a bar

A `trun` sample count is not checked against the bytes the file holds. A
1,038-byte fragmented file whose one `trun` declares 2^32 − 1 samples is
accepted by `scan_fragments`, which reports what it claims:
`coreav probe --scan-fragments` gives a duration of 171,798,691.8 s (5.45
years) and a stream duration of 2,199,023,255,040 ticks. Listing the
packets then fails at the first one past the end, and the memory bars above
hold throughout (4,535 B of peak growth, identical at 65,537) — this is a
validation gap, not a limits one. FFmpeg 7.0.2 refuses the same file at its
header ("error reading header"), so a fix would narrow a divergence rather
than open one. `a_fragment_may_claim_more_samples_than_the_file_holds` in
`crates/coreav-isobmff/tests/memory.rs` pins today's behaviour so that a
fix changes it on purpose. Filed as
an earlier change; it is outside the
bars this measurement pre-registered.

### Closed by an earlier change

The gap above is closed: a `trun`'s sample count is now checked against the
bytes that can back it — the per-sample fields in its own box, the media
data its sizes span, and, for samples whose only size is a default of zero,
nothing at all, which is refused outright — and the file is refused where
the fragment is read (`crates/coreav-isobmff/src/fragment.rs`, "What backs
a declared sample count"). The pinned test keeps the case under the name
`a_fragment_cannot_claim_more_samples_than_the_file_holds`, where the file
of this page is now `Truncated`: the bytes its run names are not there.

Every bar above was run again on the fixed tree, by the same command on the
same host, and **every one still passes**. Fifteen of the 33 numbers moved,
all for the same two reasons, and the evidence of that run is beside this
page's own:

| Measurement | This run | After an earlier change | Why |
|---|---|---|---|
| `reject/trun-4294967295` and `-65537` | 4,535 B | 4,215 B | the file is refused while the fragment is parsed, before the work the packet listing used to do |
| `demux/fragments-*`, `reject/fragment-flood` | 8,656 B | 8,720 B | each held run carries the offset of its `trun`, for the error, growing `Run` from 88 B to 96 B; a fragment holds two run vectors, each at `Vec`'s smallest capacity of four |
| `scan+packets/fragmented-2.2gb` | 960 B | 1,024 B | the same |
| `extract-audio/*` | 74,335 B | 74,399 B | the same |
| `clip/video-samples-*` | 75,495 B | 75,591 B | the same |
| `split/video-samples-*` | 115,712 B | 115,872 B | the same |

The three widest paths (`clip` and `split` over 2.2 GB, and `probe`) are
unchanged to the byte, and the two counts that must stay equal across a
factor of 65,536 in the declared count are still equal.

## Evidence

- [`measurements.jsonl`](0004-limit-bounds/measurements.jsonl): one line per
  measured region, as the run printed it.
- [`run.txt`](0004-limit-bounds/run.txt): the whole run, with the home
  directory redacted as [`README.md`](README.md) describes.
- [`after-289/measurements.jsonl`](0004-limit-bounds/after-289/measurements.jsonl)
  and [`after-289/run.txt`](0004-limit-bounds/after-289/run.txt): the same
  two files from the run on the fixed tree, made the same way.

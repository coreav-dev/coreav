# The read window and header reads

Status: Accepted

Every command reads a file through `BufferedSource`
(`crates/coreav-isobmff/src/source.rs`): one window of at most 1 MiB in
front of the file, filled with 4 KiB and doubled while reads move
forward, so that the many small reads of box headers, sample tables and
frame headers become few positioned reads of the file. Matroska, WebM and
transport streams read through the same window (`coreav-mkv` and
`coreav-mpegts` re-export it). This note covers how a fill writes the
window, and how the scans of each frame's headers (`frames.rs`,
`access.rs`, the Matroska block headers) read them.

## Problem

A fill used to clear the window and resize it to the fill's size with
zero bytes, then read the file over the zeroes. Each refill paid for
writing the window twice. A scan that reads the first bytes of every
sample (the frame size scan of `coreav check` and `probe
--scan-frame-sizes`) refills the window thousands of times: on a
development fragmented MP4 of corpus v1 (`1734f2047a70…`), 2,954 refills,
and `memset` was 27% of `check`'s instructions (12.3M of 45.9M,
callgrind on the `dist` build). The frame scan also allocated a zeroed
vector for every header it read (`vec![0u8; len]`, 42,626 per check of that
file, 3.2M instructions in `calloc`).

`unsafe` is forbidden in `coreav-isobmff` and `coreav-mkv`, so no read can
go into uninitialised memory except through `std`'s safe ones
(`Read::read_to_end` on a file fills a vector's spare capacity without
zeroing it, which `Source::read_append` uses).

## Decisions

1. **Refill in place.** The window's vector keeps the bytes earlier fills
   read, until a fill longer than it or `Source::release`; `filled` says
   how many of its bytes, from `start`, are the window. A fill no larger than the
   vector reads over the bytes there with one `read_exact_at`: one
   positioned read, nothing zeroed. Only `window[..filled]` is ever served,
   so a short fill (a fill cut by the end of the file, a 4 KiB fill after a
   larger one) never serves bytes of an earlier fill, and a failed fill
   holds nothing (`filled` is 0 until the read succeeds).
   - Rejected: **clear and `read_append` every fill.** Nothing would be
     zeroed either, but a file's `read_append` is a seek and a
     `read_to_end`, which reads 8 KiB first and doubles (a 4 KiB fill: a
     seek and one read; 64 KiB: a seek and two reads; 1 MiB: a seek and
     eight), where a refill now is one positioned read, thousands of times
     in a scan.
   - Rejected: **zero only what a fill adds beyond the vector.** One
     positioned read every time, but the bytes that a growing window adds
     would still be written twice.
2. **Grow by appending.** A fill longer than the vector (the window's
   first fill, and each fill longer than any before it since: a doubling
   while reads move forward, or a read that asks for more) frees the old
   vector, reserves the new size and reads with the inner source's
   `read_append`, into the spare capacity: from a file, the system calls
   of decision 1's rejected alternative, once per growth, instead of
   zeroing the new size. Freeing first keeps the peak at one window. The vector
   keeps its capacity as before this change, so memory is unchanged: one
   window of at most `MAX_READ`.
3. **Inner reads stay the same.** A fill reads the same offsets and sizes
   from the inner source as before; only the method (`read_exact_at` or
   `read_append`) differs, and the two return the same bytes and errors
   (the test of an earlier change). A remote source sees the same requests; from a file,
   a growth costs the extra system calls of decision 2.
4. **Header reads go to the stack.** The frame size scan (`frames.rs`) and
   the random-access walk (`access.rs`: TS keyframes, cut points, thumbnails)
   read each NAL unit length, NAL unit header, slice header, OBU header and
   frame header (at most 64 bytes) into a buffer on the stack, one per
   frame (`read_head`). Larger reads (a parameter set, an SEI message, a
   64 KiB chunk of an Annex B frame) go to one vector per frame, read with
   `read_append`, and the vector is reused for the frame's later ones.
   Through a file, bytes in memory or a `BufferedSource` they are not
   zeroed; through a source with the default `read_append` (Matroska's
   view of a frame behind header stripping, which is how Annex B frames
   are read, and remote sources) they still are, and only the
   allocation per read is saved.
   - Rejected: **a scratch vector held by the caller across frames.** It
     would also save the allocation of the rare larger reads, but changes
     `read_frame`'s public signature for a cost the stack buffer already
     removes from the common path.
5. **Matroska block headers.** `coreav-mkv` reads the first 64 bytes of
   every SimpleBlock and Block (the header, and most lacing tables) into a
   stack buffer, a longer lacing table into a vector with `read_append`;
   element data (`read_data`: cluster timestamps, block durations,
   metadata) is read into reserved capacity, as `coreav-isobmff`'s box
   payloads already were.
6. **Checked, not changed:** the transport stream reader resizes its
   96 KiB chunk only when it grows (the same length every time, so nothing
   is zeroed again), and its large reads go past the window; the remote
   reader (`remote.rs`) zeroes each request's buffer, a cost far below the
   request's round trip; the writers' copy window already refills in place; the reads of `moov`, compressed movie data and iTunes gapless
   data happen once per file.

## Verification

- Unit tests (`source.rs`): growth reads by appending and refills read in
  place over bytes earlier fills read (never a zeroed buffer); a 4 KiB
  refill after a backward seek, in a vector that holds 64 KiB of an earlier
  fill, serves none of those bytes; a fill cut by the end of the file holds
  only the bytes before it, and reads past them are truncated at the end;
  a fill that fails (in place or by appending, from a source that writes
  part and fails) holds nothing; random sequences of reads return the
  bytes of a plain source.
- A fuzz target (`coreav-isobmff/fuzz`, `buffered_reads`): the input is the
  file and seeds a sequence of reads, releases and seeks through
  `read_exact_at` and `read_append`, also from an inner source that claims
  more bytes than it has; every read that succeeds returns the file's
  bytes. With the window served from the whole vector (the bug the
  `filled` count prevents), it fails within seconds.

## Bars (fixed before the measurement run)

The measurement is `docs/spikes/read-window/` (method and evidence there). One
machine, one run: the branch's `coreav` and a control built from `main`
at this branch's base (`81f9ee2`), both `--profile dist`, the same
toolchain. Development files are corpus v1's `real` files that are not
held out; this is a development measurement, not a claim.

| Bar | Set | Threshold |
|---|---|---|
| R1 identity | every corpus v1 file (2,970) | 100%: `check --policy P` for each of the three example policies, `probe --scan-frame-sizes` and `remux FILE OUT` print the same standard output and error and exit with the same code as the control; `remux` writes the same bytes (SHA-256) |
| R2 instructions | development files with an ISO base media, Matroska or WebM extension, for `check --policy web-upload.json` and `probe --scan-frame-sizes`; those up to 200 MB for `remux` | the whole process's instructions (cachegrind, cache simulation off): the total over the set and the median per-file ratio to the control below 1.00 for each command, and no file above 1.01 (Rust's randomly seeded hash maps move counts slightly from run to run) |
| R3 wall time | the sets of R2 | per file, the median of 5 rounds after one warm-up, the order of the binaries alternating, warm page cache; the median per-file ratio to the control at most 1.00 for each command, overall and per class (ISO base media fragmented, not fragmented; Matroska and WebM) |

R1 and R2 misses block the merge. An R3 miss is investigated; it blocks the
merge when the 95% bootstrap interval of the median ratio lies wholly above
1.00 (a measurable slowdown), and is otherwise reported as a miss within
the noise.

Reported, not bars: the reduction of instructions and wall time; for
`check` and `probe --scan-frame-sizes`, the share of development files
on which `coreav` is faster than `ffprobe -v error -count_packets
-show_streams` (the comparator of `frame-sizes.md`, S6), with the control
and with the branch, and the median ratios. The Python `check` and
`probe(scan_frame_sizes=True)` run the same library code and are not
measured apart.

## Results

The run is in [`docs/spikes/read-window/`](../spikes/read-window/README.md),
its evidence in `docs/spikes/read-window/evidence/`: this change's production
code (unchanged since the run; later commits changed tests, documents and
the guard's baseline) against `main` at `81f9ee2`, on one 8-vCPU machine. 886
development files for R2 and R3 (81 fragmented, 665 not, 140 Matroska and
WebM), 730 of them up to 200 MB for `remux`.

| Bar | Result | Status |
|---|---|---|
| R1 identity | 14,850 of 14,850 cases (2,970 files × 5 commands) the same: output, error, exit code and written bytes | Met |
| R2 instructions | `check` 49.30G to 18.16G in total (−63.2%), median per-file ratio 0.533; `probe --scan-frame-sizes` 48.45G to 17.37G (−64.2%), 0.513; `remux` 20.14G to 17.69G (−12.2%), 0.974. Highest file 1.0003, the same file for all three | Met |
| R3 wall time, `check` | median ratio 0.949 (95% CI 0.942 to 0.957); fragmented 0.957, not fragmented 0.957, Matroska 0.920 | Met |
| R3 wall time, `probe --scan-frame-sizes` | 0.923 (0.916 to 0.930); fragmented 0.926, not fragmented 0.929, Matroska 0.904 | Met |
| R3 wall time, `remux` | 0.9999 (0.9972 to 1.0019); fragmented 0.997, Matroska 0.989; not fragmented **1.0018** (0.9996 to 1.0038) | Met overall and in two classes; missed within the noise in one |

On the file of an earlier change (`1734f2047a70…`, fragmented), `check` takes 22.1M
instructions where `main` takes 46.5M, and 3.51 ms where `main` takes
4.36 ms; `probe --scan-frame-sizes` 16.1M and 2.98 ms where `main` takes
39.5M and 3.82 ms.

**The `remux` miss.** It sits in the smallest files: the median ratio of
unfragmented files under 1 MB is 1.0095 (137 files), from 50 MB up 0.998
(70 files). Those calls take about 1.3 ms, and the branch's binary spends
its extra time before it reads anything: `coreav --version` takes 1.9%
longer (11 µs) with the branch's binary than with the control's, with the
same instruction count (the guard's start-up, 499,393 and 499,386), while
a copy of the control against itself measures 0.998. The branch's builds
take about two more minor page faults at start-up (medians 121 to 122
against 119); rebuilt on a later `main`, the gap is 1.2%, against 0.6%
between the two `main` builds. It is code layout in the linked binary,
which cachegrind does not see, not the read path: a growth costs one
`lseek` more than before (`strace`, a 300 KB file: 34 reads and seeks
against 33). It does not block the merge (the interval's lower bound is
below 1.00).

**Against FFmpeg** (development files, not a claim): `check` is faster
than `ffprobe -v error -count_packets -show_streams` on 884 of 886 files
(883 with `main`), median ratio 0.131 (`main` 0.138); `probe
--scan-frame-sizes` on 884 of 886 (884), median 0.123 (0.136).

# Measuring the pinned memory bounds

Status: Accepted

Three limit tests are pinned.
One is a time bound, already pinned in
`crates/coreav-isobmff/tests/hostile.rs`. The other two bound allocation:

- a 2.2 GB fragmented file is demuxed with peak memory growth under 32 MiB
  and no read larger than 1 MiB;
- a sample count of 2^32 − 1 with a tiny size table is rejected without
  allocating.

Neither can be checked by reading the code or by watching process RSS. This
note records the harness that observes allocation, the inputs it runs, the
bars fixed before the measurement run, and where the results live.

## The harness

`coreav-counting-alloc` (`tools/counting-alloc`, `publish = false`) is a
test-only crate with one `GlobalAlloc` implementation. It forwards every
request to `System` and counts what passes through: allocations,
reallocations, bytes obtained, live bytes, and the high-water mark of live
bytes. A test binary installs it with `#[global_allocator]` and wraps the
call it wants to bound in `measure`, which returns the call's value together
with the counters for that region. Only `[dev-dependencies]` name the
crate, so it is compiled into test binaries and into no library, binary or
wheel; its manifest declares that (`[package.metadata.coreav] test-only =
true`) and `scripts/unsafe_sites_check.py` fails if any crate it scans
depends on it outside its `[dev-dependencies]`, resolving a rename through
`package = "..."` or `[workspace.dependencies]` first. Its own tests
(`tools/counting-alloc/tests/self_check.rs`) check that the counters say
what the bars read them to say.

Choices, and what was rejected:

- **Counters are thread-local.** `cargo test` runs each test on its own
  thread and several at a time, so one global counter would mix concurrent
  tests. Forcing `--test-threads=1` was rejected: the bound would then
  depend on a command-line flag the test cannot enforce, and CI would
  silently stop measuring what it believes it measures if the flag were
  dropped. The cost is that a buffer allocated on one thread and freed on
  another is never seen as freed; the measured code is single-threaded, and
  the error is conservative — it can only raise a peak, never lower one.
- **Process-wide regions, for code that starts threads.** A decoder that
  decodes pictures on worker threads allocates most of its memory off the
  measuring thread, where a thread-local count cannot see it.
  `measure_process` counts every thread of the process in atomic counters
  of their own (`src/process.rs`, no `unsafe`), fed by the same allocator
  hooks after each successful call; the accounting is the thread-local
  one's (the peak never below the truth, a trough beside it). Since it
  counts whatever runs beside the body, a test binary that uses it holds
  that one test, which the binary's layout enforces rather than a
  command-line flag. One process-wide region runs at a time.
- **Regions, not process totals.** The memory bound is on the peak memory
  *growth* of one request, so a region starts at zero and the test binary's
  start-up and the fixture built before the region are excluded. Memory a
  region frees that was allocated before it offsets later growth, exactly
  as it does in the process: a call that frees a mebibyte and then takes one
  back never grew. That is the right accounting for the bar and the wrong
  one to trust blindly, so every region reports a **trough** as well — the
  low-water mark of its net live bytes — and every bar asserts it is zero.
  A peak measured next to a trough of zero is growth and nothing that was
  offset against an earlier free.
- **Nesting folds outwards** rather than being rejected, so a helper that
  measures can be called from inside another measurement; the inner region's
  totals are added to the enclosing one when it ends.
- **The metric is requested bytes**, as `Layout::size` gives them: not the
  allocator's rounding, not its own bookkeeping, and not the process's
  resident set. It is the metric the limits are written in. Where it
  cannot tell — a reallocation that may or may not have moved, and so may
  or may not have held both blocks while it copied — it takes the larger
  reading, so a peak is never below the truth.
- **`unsafe` surface**: the four `GlobalAlloc` methods (`alloc`,
  `alloc_zeroed`, `dealloc`, `realloc`), each forwarding its caller's
  pointer, layout and new size unchanged to the same method of `System` and
  returning `System`'s pointer unchanged. No pointer is dereferenced,
  compared except against null, or kept. The counting touches one
  `Cell` of plain integers and allocates nothing, so it cannot re-enter the
  allocator. The crate copies the workspace lint table with `unsafe_code`
  lowered to `deny` and puts `#[allow(unsafe_code)]` on the single module
  that holds the implementation, as the `unsafe` policy requires of an
  exception crate; the files are
  listed in `docs/unsafe-sites.md`.
- Rejected alternatives for observing allocation: `dhat`, `valgrind massif`
  and heaptrack produce a profile to read rather than a bound a test can
  assert, and the last two need a tool outside `cargo test`; process RSS
  cannot see an allocation that was never made, which is exactly what the
  second bar is about. The pinned tests name a counting allocator.

## Inputs

Fragmented inputs come from one generator shared by both crates' tests
(`crates/coreav-isobmff/tests/common/fragments.rs`). It serves a fragmented
movie that is never held in memory: a `ftyp` and a `moov` with empty sample
tables and a `mvex`, then fragments of identical length, each a `moof` with
one `traf` per track and one `mdat`. Fragment bytes are regenerated into one
reused buffer on demand, so the source allocates nothing after the first
fragment it serves and what a measurement sees is the library's memory, not
the fixture's.

- **Demux**: the 2.2 GB file — 20,000
  fragments of 10 ProRes samples of 11,000 bytes. Probe, `scan_fragments`
  and listing every packet.
- **Stream copy**: the same shape with H.264 video (a sync sample every 25
  frames, each sample a slice NAL unit so the cut rule of
  `docs/design/cut-points.md` can read it) and an AAC track of 19 samples of
  300 bytes a fragment. 2.2 GB again, and the copy reads all of it.
- **Rejects**: for every count a file declares that could drive an
  allocation — `stsz` sample count, `stco` chunk count, `stsc`, `stts`,
  `ctts` and `stss` entry counts, and the `trun` sample count inside a
  fragment — a file whose count is huge and whose table holds four entries.
  Plus 10,000-level nesting, 10,001 top-level boxes, and a box claiming
  4 GiB inside a file of a few hundred bytes.

## Bars, fixed before the run

1. **Bounded memory on fragmented files.** On the 2.2 GB fragmented file,
   peak memory growth stays **under 32 MiB** for each of: probe; probe plus
   `scan_fragments` plus listing every packet; `extract_audio`; `clip_mp4`
   of the whole file; `split_mp4` into segments. The largest single read
   stays at or below 1 MiB (`MAX_READ`).
2. **Memory does not follow the file's size.**
   - Demux holds nothing per fragment: peak growth at 20,000 fragments is at
     most peak growth at 200 fragments plus one `BufferedSource` window
     (1 MiB).
   - A stream copy cannot be flat in the number of samples — the output is a
     non-fragmented MP4, whose sample table is O(samples) by the format's
     definition, which is why `max_table_bytes` bounds it. What it must not
     follow is the file's *bytes*: with the same sample count and samples a
     tenth of the size, peak growth stays within 1 MiB of the measurement
     above.
3. **Rejected without allocating.** For every declared count above, two
   files that differ only in that count — 2^32 − 1 and 65,537, both far past
   the four entries actually present, so both are rejected — give **equal**
   allocation counts, equal bytes obtained and equal peak growth. Allocation
   that followed the declared count could not survive a factor of 65,536
   between the two. In addition, every rejecting call stays **under 64 KiB**
   of peak growth, on inputs of at most a few kilobytes.
4. **Hostile fragmented input stays bounded.** A file that runs past
   `max_fragments` is refused under the 32 MiB bar, holding nothing per
   fragment it has already passed.

5. **The decode memory budget bounds the decoder.** The native H.264
   backend, decoding a stream under the tightest `max_memory_bytes` that
   keeps a thread count (`NativeH264::memory_needed`), allocates at most
   that budget on all its threads together, at one thread and at eight,
   with and without discarding, for 720p, sd and 48x48 streams with B
   pictures, a 48x48 stream padded with parameter sets and
   `cabac_zero_words`, and a stream that changes size; a stream that grows, or is
   cropped, past what the budget allows is refused within it
   (`docs/design/h264.md`, "Limits").

A path that misses a bar is a limits bug, to be fixed rather than
renegotiated.

## Where the tests live

`crates/coreav-isobmff/tests/memory.rs` holds the demux and reject bars,
`crates/coreav-isobmff-write/tests/memory.rs` the stream-copy ones,
`crates/coreav-backend-h264/tests/memory.rs` the decoder's, which needs
`ffmpeg` for its fixtures and runs in the fixtures workflow. The
measurements on inputs of a few kilobytes run in the ordinary
`cargo test --workspace`; those on the 2.2 GB file are `#[ignore]`d and run
in the release step CI already uses for the other pinned limit tests. Every
measurement prints one JSON line, so a CI log holds the numbers behind each
bar it checked, not only that it passed.

Every field of each crate's limits struct is also pinned at its boundary in
that crate's `tests/limits.rs` (`coreav-isobmff`, `coreav-mkv`,
`coreav-mpegts`, `coreav-h264`, `coreav-decode`, `coreav-audio`,
`coreav-text`, `coreav-transcode`): an input at the limit is accepted and
one past it is refused with the error that names that limit, or, for a
limit that does not fail the request, gets the documented fallback (a
Matroska frame past `max_inflated_bytes` has no inflated size; cues past
`max_elements` or `max_index_bytes` give way to a cluster scan). The
expected count or byte total comes from the input itself, not from a
search of what the code accepts, and one test per file asserts the
documented defaults. Where an older test elsewhere already pins a field,
the file does not repeat it. These run in the ordinary
`cargo test --workspace`. Four `coreav-decode` limits are enforced in the
backends (`max_decoded_bytes`, `max_pending_bytes`,
`max_reference_frames`, `max_memory_bytes`); their boundary tests there
need `ffmpeg` and run in the fixtures workflow, and `tests/limits.rs`
pins only their defaults.

## Results

[docs/measurements/0004-limit-bounds.md](../measurements/0004-limit-bounds.md).

# Compressed movie boxes (QuickTime `cmov`)

Status: Accepted

QuickTime can store the movie box compressed: the `moov` box then holds
only a compressed movie box (`cmov`), which holds a data compression box
(`dcom`, the algorithm's four-character code) and a compressed movie data
box (`cmvd`: the inflated size in 32 bits, then the compressed data). The
compressed data inflates to a whole `moov` box (Apple's QuickTime File
Format documentation, compressed movie resources). The sample data stays
in the file; chunk offsets in the inflated tables are offsets in the file.

`coreav-isobmff` reads such a movie: `probe` inflates the
data with zlib (RFC 1950 and 1951, through `miniz_oxide`), then reads the
inflated `moov` box exactly as it reads one in the file, under the same
limits and the same counters of boxes, retained bytes and table bytes.
`Movie::compressed` tells the result came from a compressed movie box.

## Limits

Inflating is the one step whose output can be much larger than its input,
so it has its own limits, on top of the ones every box walk has:

- `Limits::max_movie_bytes`, default 16 MiB: the inflated movie box, held
  in memory while it is read, reserved once at the declared size (an
  allocation that fails is this limit, not an abort). The compressed data
  is read from the file in 64 KiB chunks, never whole, and may not be
  longer than zlib's bound for the declared size (`compressBound`).
  Memory while a compressed movie box is read is therefore the declared
  size plus 64 KiB and the inflater's state, besides the tables parsed
  from it, which `max_table_bytes` bounds as for any movie. The bound
  refuses data padded after the zlib stream, or written by an encoder that
  flushes far more often than zlib's; no corpus file does either, and the
  error says the data is longer than its size allows. The WebAssembly module takes
  this and the next limit as `maxMovieBytes` and `maxMovieRatio`.
- `Limits::max_movie_ratio`, default 256: the inflated size per compressed
  byte. zlib's own bound is about 1,032; a movie box of sample tables
  compresses by a few times (1.9 to 3.5 in the corpus files below), so 256
  leaves room for unusual tables and stops a small file from claiming the
  whole size limit.
- The size `cmvd` declares must be the inflated size exactly; inflation
  stops at the declared size (or the limit), so a stream that would run
  past it fails without producing more.

Errors: `LimitExceeded` with `MovieBytes` or `MovieRatio` before anything
is inflated when the declared size breaks a limit; `InvalidBox` when the
data is longer than its size allows, does not inflate, inflates to another
size, or is not one `moov` box; `Unsupported` for an algorithm other than
`zlib`. An error inside the inflated box reports the offset of the `cmvd`
box: its own offsets are offsets in the inflated data, not in the file.

Structure rules: a movie box with a movie header (`mvhd`) is read as
before, any `cmov` in it ignored; one without a header but with a `cmov`
is read through it, and must hold no track (`compressed movie beside
tracks`); an inflated movie box holding another `cmov` is rejected
(`compressed movie inside a compressed movie`), so inflation never
nests.

Evidence: the three compressed movies of corpus v1 (all `zlib`, inflated
sizes 4,780 to 107,094 bytes, ratios 1.9 to 3.5), which CoreAV rejected
before.

## What reads it

Probe, packet listing and every command that builds its output from the
probed movie: `clip`, `extract-audio`, `split`, `hls`. `remux` and
`sanitize` copy boxes from the file's own movie box, which in a compressed
movie holds only the compressed data, so they fail with `Unsupported`
(`compressed movie box (cmov): its boxes cannot be copied`) before writing
anything. Movie-level metadata (`udta`, `meta`) of the inflated box is
read while the box is in memory: `Movie::itunes_gapless` returns what it
holds, so `extract-audio` and `clip` keep gapless trimming. Box depth
starts again inside the inflated box, as inflation never nests.

## Choices and rejected alternatives

- **A vetted inflater, not our own.** `miniz_oxide` is pure Rust and
  `#![forbid(unsafe_code)]`, as is its one dependency (`adler2`), under
  MIT, Zlib or Apache-2.0. Rejected: writing an inflater from RFC 1951
  (more parser code of our own for a rare format); `flate2` (a wrapper
  with optional C backends, more than this needs).
- **Inflate to memory, then walk.** The inflated box is read through the
  same walker over an in-memory source, so every structure rule and limit
  applies unchanged. Rejected: a streaming inflate under the walker
  (box headers seek, which a stream cannot); rewriting the file in memory
  with the inflated box in place (sample offsets would move).
- **Stream copy refuses.** Making `remux` and `sanitize` read the inflated
  boxes would let them write an uncompressed copy, which is useful, but
  their box scan and metadata policy read the file directly; that is a
  separate change. Until then they fail clearly instead of with a box
  count mismatch.
- **16 MiB by default.** The largest inflated movie box in the corpus is
  107 KB; 16 MiB holds the tables of hours of video and keeps one hostile
  upload from claiming much of a process's memory, or of a WebAssembly
  instance's. Rejected: 64 MiB (with the
  compressed data whole in memory, an upload could reach about 128 MiB).
- **The WebAssembly module grows** by 10,970 bytes gzip -9 (176,032 to
  187,002), well under its 1 MiB bar.

Fuzz target: `crates/coreav-isobmff/fuzz`, `compressed_movie` (arbitrary
movie box payloads compressed by the target, and arbitrary `cmov`
payloads).

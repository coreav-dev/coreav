# coreav-backend-h264

CoreAV's native H.264 decoder (`coreav-h264`) behind the decode boundary
(`coreav-decode`), as the `native` backend.

**This backend is opt-in and is never part of a default build.** Decoding
H.264 may require a patent licence depending on where you are and what you
do with it; CoreAV's default H.264 path is unchanged: the sandboxed FFmpeg
fallback (an NVDEC backend is planned but not built yet,
`docs/design/decode.md`, "NVDEC"). The packaging rules are in
`docs/design/h264.md` ("Packaging"), and the same disclaimer is in
[`crates/coreav-h264/README.md`](../coreav-h264/README.md).

Nothing here is legal advice. Deciding whether you may use this decoder is
yours to make.

## What it does

- Decodes the streams `coreav-h264` decodes: High profile, 8-bit 4:2:0
  and monochrome (whose frames are 4:2:0 with neutral chroma), progressive. A stream outside that is refused from its configuration
  record, before a decoder is opened — the parameter sets decide most of
  the scope, not the profile — so the boundary moves on to the next
  backend in the policy's order. What only a slice can decide (data
  partitioning, SP and SI slices) is refused when it arrives.
- Shows what the container says it shows: where an MP4 or QuickTime file
  declares fewer samples than the bitstream codes, and the difference is
  the padding of a macroblock, the frame is the declared size, as it is
  through the FFmpeg fallback.
- Runs in the calling process (`Isolation::InProcess`): no worker, no
  shared memory, no `ffmpeg` binary. Everything that touches the bitstream
  is safe Rust; the pixel kernels are those of `coreav-kernels-h264`, whose
  `unsafe` sites are listed, each with its precondition, in
  `docs/unsafe-sites.md`.
- Keeps to the decode limits: frame size, decoded bytes, bytes not yet
  received, packet size, the deadline, and the memory budget, which the
  decoder checks before the first picture of each coded size: it decodes
  with fewer threads, or refuses the stream with `Limit`.

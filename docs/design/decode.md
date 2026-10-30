# The decode boundary

Status: Accepted

Decoding sits behind a plugin boundary at the compressed-packet level, and the first roadmap step needs it for thumbnails
and frame extraction. This note records the boundary
(`crates/coreav-decode`), the frame model (`crates/coreav-frame`), how a
backend is chosen, the sandboxed worker (`crates/coreav-worker`) and the
backends behind it: the FFmpeg fallback (`crates/coreav-backend-ffmpeg`),
dav1d for AV1 (`crates/coreav-backend-dav1d`), libvpx for VP9
(`crates/coreav-backend-libvpx`) and NVDEC (`crates/coreav-backend-nvdec`). Frame operations and thumbnails are in
`docs/design/thumbnails.md`.

Specifications: ISO/IEC 14496-15 (AVC and HEVC configuration records and
sample formats), the AV1 ISOBMFF binding, the VP codec ISOBMFF binding,
RFC 9559 and the Matroska codec mappings (the Matroska stream written to
the fallback), ITU-T H.273 (colour code points), ITU-T H.264 and H.265
(parameter sets, for selection and NVDEC), the AV1 specification, the VP9
Bitstream and Decoding Process Specification 0.6 (frame headers and
superframes, for the libvpx backend's frame sizes). FFmpeg is
used only as a binary, as the fallback backend and as the oracle, observed
through its documented command-line options; dav1d through its published
Rust binding; libvpx through its public API headers and documentation;
NVDEC through NVIDIA's published Video Codec SDK headers. No
multimedia implementation source was read.

## Crates and what each may do

| Crate | Role | `unsafe` |
|---|---|---|
| `coreav-frame` | Frame model, frame operations | forbidden |
| `coreav-decode` | `Decoder` trait, stream description, codec allowlist, limits, selection | forbidden |
| `coreav-worker` | Sandboxed worker process: control protocol, descriptor validation, shared memory | `deny`, allowed only in `shm.rs` |
| `coreav-backend-ffmpeg` | Drives an `ffmpeg` binary inside the worker | forbidden |
| `coreav-backend-h264` | CoreAV's own H.264 decoder, in the calling process | forbidden |
| `coreav-backend-dav1d` | dav1d through the `dav1d` crate | forbidden (the binding's own `unsafe` is a dependency's) |
| `coreav-backend-libvpx` | libvpx through CoreAV's declarations of its decoder API; the frame sizes of each packet, read in safe Rust | `deny`, allowed only in its FFI module |
| `coreav-backend-nvdec` | NVDEC through FFI | `deny`, allowed only in its FFI module |

The container crates do not depend on any of them, and `coreav-decode`
depends on no container crate: callers translate a container's track into
a `StreamConfig` (section "Streams and packets"). This is the direction
the architecture asks for: "the container layer never depends on a codec
implementation".

## The `Decoder` trait

```rust
pub trait Decoder {
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error>;
    fn send_eof(&mut self) -> Result<(), Error>;
    fn receive_frame(&mut self) -> Result<Option<Frame>, Error>;
    fn info(&self) -> &DecoderInfo;
}
```

- Packets go in in decode order, one container sample (an access unit, a
  temporal unit, a frame) per call. Frames come out in presentation
  order, each carrying the `pts` of the packet whose picture it is.
- Before `send_eof`, `receive_frame` returns the frames that are ready and
  `None` when none is, without waiting for more input. After `send_eof`
  it waits for the next frame and returns `None` once every frame is out.
  A caller that sends many packets interleaves `receive_frame`; frames it
  leaves waiting count against `Limits::max_pending_bytes`, so a caller
  that never receives gets an error rather than unbounded memory or a
  deadlock.
- `info()` names the backend that runs and whether it is the fallback
  (section "Selection").
- Errors: `Error { kind, message }`, with kinds `InvalidData` (the packets
  or the configuration), `Unsupported`, `CodecNotAllowed`, `Limit`,
  `Timeout`, `Backend` (the backend failed: a decoder error, a worker
  that exited or broke the protocol) and `Io`.

Rejected:

- **A callback or iterator API** (`decode(packets) -> impl Iterator`):
  simpler for one-shot use, but a server that feeds packets as it reads
  them needs push and pull separately, as libavcodec and dav1d do.
- **`receive_frame` that waits before end of stream.** A decoder with
  reorder delay may not output anything until more packets arrive; a
  waiting call would deadlock a caller that sends one packet and then
  receives.

## Streams and packets

`StreamConfig` describes a stream: the codec, the codec configuration as
stored (`avcC`, `hvcC`, `av1C` or `vpcC` payload, or Matroska
`CodecPrivate`, tagged with which), the declared coded size, the colour
description from the container or the parameter sets, and the time scale
of the packets' timestamps. A `Packet` is the sample's bytes in the
container's own format (length-prefixed NAL units for H.264 and HEVC, OBUs
with sizes for AV1, frames for VP8, VP9 and ProRes), its `pts` and `dts`
in that time scale, and its sync flag. Each backend converts from this
format to what its decoder takes; callers never convert.

`traits()` reads, in safe Rust, what selection needs from the
configuration record without decoding: bit depth, chroma format and
profile (the `hvcC`, `av1C` and `vpcC` fields; for `avcC`, the profile and,
for the High profiles, the first sequence parameter set up to
`bit_depth_chroma_minus8`, ITU-T H.264 7.3.2.1.1).

## Frame model

`coreav-frame` holds the frame model. A `Frame` is a presentation
timestamp, a colour description and its pixels, which are one of two
first-class variants:

- `Pixels::Cpu(Picture)`: planes in CPU memory.
- `Pixels::Cuda(CudaPicture)`: planes in CUDA device memory, with the
  device, pitches and an owner that releases the memory when the last
  reference drops and copies it to the CPU on request. NVDEC frames stay
  on the GPU; nothing in `coreav-frame` dereferences device memory.

`Picture` validates its geometry when built: every plane's stride covers
its row, every plane holds `stride × (rows − 1) + row bytes`, all with
checked arithmetic, so the operations never index outside a plane.

Pixel formats, v1 (8-bit 4:2:0 required, 10-bit 4:2:0 recommended):

- `Yuv { chroma, depth }`: planar Y'CbCr, chroma 4:2:0, 4:2:2, 4:4:4 or
  none (monochrome), depth 8 to 16 bits; one byte per sample at 8 bits,
  two little-endian bytes holding the value in the low bits otherwise.
  This is what software decoders output.
- `SemiPlanar { depth }`: a Y plane and an interleaved CbCr plane, 4:2:0;
  above 8 bits, 16-bit samples with the value in the high bits. NVDEC's
  output (NV12, P010, P016).
- `Gbr { depth }`: planar G, B, R, for streams coded in RGB (matrix
  coefficients 0, ITU-T H.273).
- `Rgb24` and `Rgba`: packed 8-bit, the output of conversion.

Alpha planes (ProRes 4444) are dropped: no v1 operation uses them, and a
thumbnail of a frame with alpha is of its colour planes.

Colour: matrix coefficients, primaries, transfer characteristics, range
and chroma sample location as H.273 code points, from the backend's frame
when it reports them, else from the stream configuration.

Rejected:

- **One fixed format (I420) everywhere.** Converting 10-bit and 4:2:2
  frames at the boundary loses precision and costs a pass that most
  consumers redo.
- **Opaque per-backend frame types.** Every frame operation would need a
  version per backend.
- **Zero-copy views into worker memory**:
  see "Shared memory" below.

## Codec allowlist and limits

`DecodeAllowlist` names the codecs that may be decoded at all; a stream
outside it fails with `CodecNotAllowed` before any backend sees a byte.
The default is H.264, HEVC, AV1, VP9, VP8 and ProRes, the mainstream
codecs. MPEG-4 Part 2, MPEG-1 and MPEG-2 video, Motion JPEG,
Theora and FFV1 are known to the boundary but are long-tail decoders, the
class that produced PixelSmash: they are decoded only when an operator
adds them.

`Limits`, the decode limits on resolution, reference frames and decoded
bytes. The frame limits (`max_width`, `max_height`, `max_pixels`,
`max_frame_bytes`) and `max_packet_bytes` are checked by `open` on every
packet sent and every frame received, whatever the backend checks: the
decoder it returns wraps the backend's, so no backend can leave them out.
A backend checks them too where it can refuse earlier (a stated picture
size before it allocates) and for a caller that opens it directly. The
other limits are the backend's to enforce:

| Limit | Default | Why |
|---|---|---|
| `max_width`, `max_height` | 16,384 | Above every level of H.264, HEVC and AV1 in use (AV1 level 6.3: 16,384 × 8,704). |
| `max_pixels` | 2^27 (134,217,728) | 16K × 8K; 8K UHD is 33 million. |
| `max_frame_bytes` | 512 MiB | 8K 4:4:4 at 16 bits is 398 MiB. |
| `max_decoded_bytes` | 4 GiB | Per decoder: bytes of every frame it outputs. |
| `max_pending_bytes` | 1 GiB | Frames decoded but not yet received; and, apart, packets sent but not yet taken by the decoder (in the worker, and on their way to FFmpeg). Past it the sandboxed worker waits for the caller to receive frames ("Threads and backpressure"), while the in-process backends (native, dav1d, libvpx) refuse with `Limit`, as they cannot block a caller that is not receiving. Packets waiting past it in the worker, or on their way to FFmpeg, are refused with `Limit`. |
| `max_packet_bytes` | 128 MiB | The container limit. |
| `timeout` | 60 s | Wall clock per decoder, from open to the last frame; the worker is killed when it passes. |
| `max_reference_frames` | 16 | The most H.264 and HEVC allow; NVDEC's DPB. |
| `max_memory_bytes` | 8 GiB | Address space of each process of a sandboxed decoder (the worker, and each process it starts): see "Memory". |

## Selection

`select(config, policy, backends)` picks one backend per stream:

1. The codec must be on the allowlist.
2. The candidates, in the policy's order (default: NVDEC, native,
   dav1d, libvpx, fallback), each say whether they support the stream (codec,
   profile, bit depth, chroma format, size, and for NVDEC what the device
   reports); the first that does and that the policy enables is opened.
   If opening fails (no GPU, no `ffmpeg` binary), the next is tried, and
   the failure is recorded.
3. The result names the backend, whether it is the fallback (`fell_back`:
   FFmpeg's C decoders ran, sandboxed), and every candidate skipped, with
   its reason. `coreav thumbnail` prints it, so operators can see how much
   of their traffic avoids C decoders.

CoreAV's own H.264 decoder is the `native` backend
(`crates/coreav-backend-h264`, `docs/design/h264.md`), dav1d decodes
AV1 (section "dav1d") and libvpx decodes VP9 (section "libvpx"); all
three are opt-in and never in a default build. The other codecs go
through the fallback, except in strict-safe mode, which never runs it
(section "The FFmpeg fallback").

**A caller may order the backends by the work it is about to ask for.**
The order is the policy's, and `coreav thumbnail` sets two of them
(`docs/spikes/h264-native-thumbnail/`, measured on the held-out H.264 files of
corpus v1):

- A thumbnail at a keyframe decodes one picture, and the native decoder
  is the better path for it: 0.437 of the `ffmpeg` command line, and
  0.986 of the sandboxed fallback, which starts a worker and an `ffmpeg`
  and copies the frame back.
- `coreav thumbnail --exact` decodes every picture from the keyframe to
  the frame asked for. The native decoder is single-threaded, so it takes
  5.6 times the fallback's time and 4.6 times FFmpeg's; `--exact`
  therefore puts the fallback first until the decoder decodes on more
  than one thread.
- dav1d decodes on its own threads and is ahead of the fallback in both
  orders: through the fallback, `--exact` on AV1 took 1.186 of FFmpeg's
  time, through dav1d 0.934 (bar 5 below, "Results").

A backend that trims what it returns must trim it the way the others do:
FFmpeg honours a container that declares fewer samples than the bitstream
codes (an MP4 whose sample entry is 854 wide for a stream cropped to
864), so the native backend honours `StreamConfig.width` and
`StreamConfig.height` where they are inside the coded picture and within
one macroblock of it. Two backends of this boundary return the same
picture for the same file.

## The sandboxed worker

Third-party decoders run in a worker process by default. The parent treats the worker as untrusted and possibly
compromised.

**Process.** The worker is a program that calls `coreav_worker::worker_main`
with a check of the backends it hosts (`coreav`'s is strict-safe mode's):
the `coreav` binary itself, run again with a hidden argument, or the
`coreav-worker` binary of the crate for library users. The parent starts
it with `std::process` (no shell), a cleared environment, and pipes for
standard input, output and error. The worker applies resource limits to
itself before it starts any decoder, inherited by any process it starts:
address space, CPU seconds, no core files, no regular-file writes
(`RLIMIT_FSIZE` 0: decoders write only to pipes), and a small file
descriptor count. It sets them through `rustix`'s safe `setrlimit`, a
vetted crate, not through CoreAV-authored `unsafe`.

**Exec.** The same limits serve a program that is not a decoder: the
encoder of `coreav transcode` (`docs/design/transcode.md`, "Sandbox").
`coreav_worker::exec` is a second hidden entry of the same programs
(`__coreav_exec`, in `coreav` and in `coreav-worker`): its arguments are
the limits, then `--` and the program's path and arguments. It sets the
limits on itself with the worker's code (also no core files and
`RLIMIT_FSIZE` 0) and replaces itself with the program through `std`'s
`CommandExt::exec`, with the standard streams it was given and an empty
environment, so the program inherits the limits and no process sits
between it and the parent's pipes. The launcher is in every build with a
decoder backend (the `thumbnail` feature), the native H.264 decoder alone
included. The program must be a path: with no
environment there is no `PATH`, and a search would depend on the
launcher's defaults. A launcher that cannot set the limits or start the
program exits with 125, 126 (not runnable, or refused by the check its
caller gives `sandbox_exec_main`: strict-safe mode's in `coreav`) or 127
(not found), after one line on standard error. The parent side builds the command (its own
process group, empty environment, piped streams), and `exec` also holds
the two process-control calls the keepers need (`kill_group`, and
`has_exited`, Linux's `waitid` with `WNOWAIT`), so `coreav-transcode`
needs neither `rustix` nor anything beyond `std`. The module is
`#![forbid(unsafe_code)]`; it adds no `unsafe` and no C call beyond the
worker's.

**Control.** Length-prefixed binary messages over the pipes. Parent to
worker: `Open` (backend, stream configuration, limits, the shared-memory
layout), `Packet`, `Eof`, `Ack` (a slot is free again), `Close`. Worker to
parent: `Ready`, `Frame` (a descriptor), `Done`, `Error`. The parent reads
at most 4 KiB per worker message; descriptors are under 200 bytes. A
message that does not parse, a sequence number out of order, a slot not
owned by the worker, or anything after `Done` ends the session with a
`Backend` error and kills the worker.

**Shared memory**. The parent creates an anonymous memory file
(`memfd_create`), sizes it for N slots (default 2) of the largest frame
the stream's declared size and the limits allow, and seals it against
shrinking and growing (`F_SEAL_SHRINK`, `F_SEAL_GROW`, then `F_SEAL_SEAL`),
all through `rustix`'s safe API. The worker opens it read-write through
`/proc/<parent>/fd/<n>`; the descriptor itself is close-on-exec, so other
child processes do not inherit it. The parent maps it read-only as a raw
mapping (`memmap2::MmapRaw`, created without `unsafe`, which hands out
pointers, never references), the worker read-write with one `memmap2`
call. The crate's only `unsafe` is in `shm.rs`: the worker's mapping call,
and the parent's copy out of its mapping (below).

The seals settle what a compromised worker could do to the backing file: it cannot truncate the file, so the parent's mapping cannot fault (`SIGBUS`); it can
only write bytes into it, which is what it is there for.

**Descriptors.** A frame descriptor names the slot, the sequence number,
the pts, the width, height and pixel format, and each plane's offset,
stride and length. The parent validates it before it touches the mapping
(`descriptor.rs`, fuzzed): the slot is one the worker holds; the format is
one the frame model knows; width and height are within the limits; for
each plane, the stride covers a row, the length is exactly
`stride × (rows − 1) + row bytes`, and `offset + length` lies inside the
slot, all with checked arithmetic. Only then does it take the plane's
bytes.

**Copy, not views.** The parent copies each validated plane out of the
mapping into memory it owns, once, and acknowledges the slot; consumers
get owned frames. The copy is by raw pointer
(`ParentMemory::copy_out`, bounds-checked first): the parent never forms
a Rust reference (`&[u8]`) to memory another process may change, which
the language's aliasing rules would not allow. A worker that writes while
the parent copies can only change pixel values, which it controls anyway;
no value of any byte affects the parent's control flow, since the
descriptor was read from the pipe, not the mapping. Views into the mapping
handed to consumers would let a compromised worker change a frame while
it is being used and would hold slots for as long as a consumer keeps
a frame. The copy is one `memcpy` per plane, small next to decoding the
frame.

**Threads and backpressure.** In the parent, one thread reads the
worker's messages, validates and copies frames, and hands them to
`receive_frame` through a queue of at most 4 frames (and
`Limits::max_pending_bytes`); it acknowledges a slot only after the frame
is handed over. In the worker, one thread reads the parent's messages
(acknowledgements free slots, packets go to the backend) and the backend
runs on another. The worker hands on the frames its backend has ready
after each packet, at the end of input, and whenever no message has come
for 2 ms: FFmpeg decodes on threads of its own, so its frames come while
no packet does, and a caller that waits for frames before it sends more
(the transcode's pacing, `docs/design/transcode.md`) would otherwise wait
for a message that never comes. Rejected: a wake-up from the
backend when a frame is ready, which needs a hook in the `Decoder` trait
for every backend; a longer or growing interval, which adds its length to
every wait of such a caller; and draining only when the parent asks,
which adds a message each way to every frame. Waking every 2 ms costs a
few microseconds a time, and only until the end of input. Because frames
are handed over only as slots free up, a slow consumer slows the worker,
which slows FFmpeg, instead of growing memory; packets the consumer sends meanwhile
queue in the worker, so `send_packet` never blocks on frames. An earlier
version of this design returned an error once frames waiting passed the
limit; the corpus showed that a consumer hashing 8K frames fell behind
FFmpeg within 120 frames, which is ordinary use, not abuse. Packets
waiting, unlike frames, are capped with an error: past
`Limits::max_pending_bytes` of them in the worker (or on their way to
FFmpeg in the backend) the decoder fails with `Limit`. Blocking instead
does not work with this trait. Acknowledgements reach the worker on the
pipe behind packets, so a worker that stopped reading packets would stop
seeing them, and a parent blocked in `send_packet` cannot take the frames
that would let the worker go on. Either way the session would stall until
the deadline. A thumbnail sends one group of pictures, far below the
cap. The known limit: a caller that streams more than the cap ahead of a
decoder slower than its reads (a long ProRes file through the fallback,
sent without waiting) fails with `Limit` and must pace its packets. The
fix is backpressure in the trait (`send_packet` saying "receive first",
as libavcodec's does), which comes with frame extraction, the first
operation that streams whole files through a decoder.

**When the memory file cannot be opened** (a sandbox without `/proc`, as
in some container runtimes), the worker sends every frame inline in its
message instead; the parent accepts both in any session.

**Failures.** A worker that exits or breaks the protocol ends the session
with a `Backend` error that carries the worker's exit status and its last
error message, read even after a failed acknowledgement (a worker that
stopped reading has usually reported why). The worker's resource limits
never exceed one it inherited: a process cannot raise its hard limit.

**Time.** Every wait in the parent ends at the decoder's deadline
(`Limits::timeout`); past it the worker is killed and the call fails
with `Timeout`. A keeper thread owns the worker process: it kills it at
the deadline, when the decoder fails, or when the decoder is dropped,
which also makes a write blocked on a worker that stopped reading fail.
Writes go through a writer thread and the caller waits for each at most
until the deadline, so even a worker that hands its input to a process
outside its group (which the kill does not reach) holds only that
thread and what it holds: the packet it was writing (up to
`max_packet_bytes`), and, through the reader thread, the shared memory,
for as long as that process lives. Acknowledgements waiting for the
writer never outnumber the slots: a frame in a slot the parent still
holds ends the session. The worker runs in a process group of its own
and the keeper kills the group, so the backend's own children (FFmpeg)
end with it; it reaps the worker only after the kill, so the group's id
cannot have been reused. (How the worker ended is reported on Linux only,
where `waitid` can look without reaping. Without process groups, on
systems other than unix, only the worker is killed.) Dropping a decoder
returns at once. The worker's CPU limit bounds a
decoder that spins without doing I/O, even if the parent has gone.

**Memory.** Each process of a sandboxed decoder has a memory budget,
`Limits::max_memory_bytes` (default 8 GiB).
- The worker sets it, within the worker options' cap and any lower limit
  it inherited (soft or hard), as its own address-space limit
  (`RLIMIT_AS`), through `rustix`'s safe `setrlimit`, before it starts
  any process. Every process it starts inherits it, and its backend fits
  itself to the limit in force.
- The budget is per process: a worker and its FFmpeg may each use it.
  The parent has no address-space limit; the frames and packets it holds
  are bounded by the frame and pending limits above.
- The limit is enforced on Linux. On other systems `RLIMIT_AS` may not be
  enforced, and outside Unix there is none; there the budget only fits the
  threads.

A decoder that runs into the limit fails with `Limit`. FFmpeg exits by
itself with an error code after an allocation was refused or a thread
could not start ("Cannot allocate memory", "Resource temporarily
unavailable"). The fallback notes such a message whenever FFmpeg prints
it, and reports the exit as `Limit`, also when it shows first as a short
frame on its output. A signal is a crash, reported as such. EAGAIN for a
thread can also come from another resource limit (the system's process
count), reported the same way: a resource limit.

The worker's own frame and message buffers are reserved fallibly and fail
with `Limit`, also when the main thread is waiting for a free slot. An
allocation of the worker outside those still aborts it, reported as the
worker's exit.

Within the budget, the fallback fits FFmpeg's decoding threads before it
starts, and only ever lowers them. Its filter graph (`showinfo`) and
output (raw video) run on one thread each (`-filter_threads 1`, output
`-threads 1`): they only pass frames on, and their thread pools otherwise
grow with the host's CPUs. With both pinned, the large file below peaks
at 2.2 GiB on one CPU and 2.5 GiB on eight, with one decoding thread
(3.6 GiB on eight without). It estimates FFmpeg's address space as:
- 1 GiB, plus 96 MiB per thread;
- plus six frames and two more per thread;
- plus, for codecs with reference frames (all but ProRes),
  `max_reference_frames` frames.

A frame is the declared size at the format the configuration record gives
(two bytes per sample above 8 bits; ProRes 4444 may carry alpha), or 8
bytes per pixel when it gives none.

It keeps the threads asked for, or FFmpeg's own choice (measured as one
per CPU and one more; taken at most 16), while the estimate fits, and
lowers them until it does, down to one. It passes the count explicitly:
the CPUs it counts honour a container's CPU quota, which FFmpeg's own
count may not see.

It refuses a stream with `Limit` before FFmpeg starts only when its frames
and one thread do not fit (`frame × (8 + references) + 96 MiB`). The fixed
1 GiB of the estimate is mostly address space reserved rather than used,
and FFmpeg with one thread decodes small streams within much less. This
refusal trusts the configuration record and the declared size, which a
file can understate, so it is advisory: the address-space limit is what
holds.

The estimate is an upper bound on these measurements, all of ProRes,
which is intra-only (the reference-frame term covers the other codecs and
is not measured here): the peak address space (`VmPeak`) of the pinned
FFmpeg 7.0.2, by the thumbnail spike's memory script. The large file is 8809×6314
ProRes 4444 at 12 bits with alpha, corpus file `ed60cdd24ba9`, about
445 MB a frame.

| Input | Threads | Measured | Estimate |
|---|---:|---:|---:|
| Large file, read by `ffmpeg -i` | 1 / 2 / 4 | 3.4 / 4.4 / 5.9 GiB | 4.4 / 5.3 / 7.2 GiB |
| Large file, the fallback's command (before output and filter threads were pinned) fed Matroska, output read at once | 1 / 2 / 4 | 3.6 / 4.6 / 6.4 GiB | as above |
| 1080p ProRes 4444, 10 bits with alpha | 1 / FFmpeg's choice (8 CPUs) | 1.1 / 1.7 GiB | 1.2 / 2.1 GiB |

The estimate holds while FFmpeg's output is read as it comes, which is
what a thumbnail does. A consumer that falls behind lets FFmpeg's
internal queues fill: with the output stalled for 30 seconds, the large
file reached 13.4 GiB with one thread and 16.4 GiB with four. FFmpeg's
command line has no bound for those queues, so there the address-space
limit is what holds, and the request fails with `Limit`.

In corpus v1's run, the large file ran out of memory, killed by the
kernel on a 16 GB host. Two processes did: the reference FFmpeg (15.0
GiB) and the fallback's FFmpeg, then with automatic threads and the old
16 GiB cap (10.0 GiB). Now the fallback's FFmpeg stays under the budget,
at 6.6 GiB resident with four threads. When the decode oracle sends all
120 packets ahead of a slow consumer, the request ends with `Limit` and
nothing is killed. A thumbnail of the file, with the default budget,
stays under 1 GiB resident.

Re-run on a 64 GiB host with the decode oracle's budget raised to 32 GiB
(a test-tool setting), all 96 of its frames agree with FFmpeg's `framemd5`
(`docs/spikes/thumbnail-bench/`).

A decoder backend in the calling process (no sandbox) has no address-space
limit of its own; there the budget only fits its threads.

What the sandbox does not do yet: system-call filtering (seccomp),
file-system and network confinement (Landlock or namespaces), and a
separate user. A compromised worker runs with the parent's user and can
do what that user can, outside CoreAV's memory. These are the
server-hardening item of stage 5 (seccomp profiles); until then the
worker gives crash isolation, resource limits and a validated, narrow
interface, not confinement.

Rejected:

- **Pipes for frames.** No `unsafe` at all, and the stage 1 measurement
  found no difference at 1080p, but shared memory is the choice for large
  frames; pipe transfer stays in the tests as the differential
  reference.
- **A plain file in `/dev/shm`** (the spike's): the worker could truncate
  it under the parent's mapping.
- **Passing the memory file by inheritance.** Simpler for the worker, but
  any process that the parent starts at the same time on another thread
  would inherit it too.

## The FFmpeg fallback

A separately packaged backend, never in the default build:
`coreav-backend-ffmpeg`, compiled into the worker and the CLI only with
the `ffmpeg-fallback` feature. It runs an `ffmpeg` binary, the system's or
one the operator names, as a child of the worker, so FFmpeg's C code never
runs in the parent process. FFmpeg's libraries are never linked: binding
them would mean reading their headers, which are FFmpeg source.

**Strict-safe mode** (`coreav --strict-safe`, `COREAV_STRICT_SAFE`;
`compat-cli.md`, "Strict-safe mode") never runs it. The CLI's backend list
(`coreav-cli`, `strict_safe::Decoders`) then holds a stand-in in its place:
the same `supports`, and an `open` that fails with `unsupported`, so
selection goes on to the next backend in the policy's order as for any
backend that fails to open. A stream no other backend opens is refused with
exit status 4, naming the build that decodes it without FFmpeg; a stream
another backend opens decodes there, and the summary lists the fallback as
skipped (`not run in strict-safe mode`). The order is not changed, so
`--exact` still tries the fallback first and, in strict-safe mode, decodes
H.264 with the native decoder. The worker and the launcher are started
with `--strict-safe` after their entry argument in the mode
(`StrictSafe::entry`), since every launch clears the environment, or read
`COREAV_STRICT_SAFE` when run directly; the worker then refuses an `Open` of the fallback
(`StrictSafe::worker`, through `worker_main`'s check), and the launcher a
program named `ffmpeg` or `ffprobe` (`StrictSafe::program`).

CoreAV demuxes; FFmpeg only decodes. The worker writes the packets to
FFmpeg's standard input as a Matroska stream that it writes itself (RFC
9559): one video track with the codec's `CodecID` and `CodecPrivate`, one
Cluster per packet. FFmpeg's container probing does not run on the
untrusted file, and the decoder is chosen by name, so a hostile file
cannot steer FFmpeg to a decoder that is not allowlisted. The command is
fixed:

    ffmpeg -nostdin -hide_banner -nostats -loglevel info -max_alloc <bytes>
      [-threads <n>] [-strict 1]
      -flags unaligned -protocol_whitelist pipe -nofind_stream_info -copyts
      -f matroska -c:v <decoder> -i pipe:0
      -map 0:v:0 -fps_mode passthrough -noautoscale
      -vf showinfo=checksum=0 -flush_packets 1 -f rawvideo pipe:1

- Frames come back as raw video on standard output, in the decoder's own
  pixel format, so frames are compared with `framemd5` without a
  conversion in between. `showinfo` writes each frame's timestamp, pixel
  format and size to standard error; the worker reads both, in order, and
  rejects a pixel format the frame model does not know.
- Timestamps: each block carries the packet's own pts plus 2^40, in a
  Matroska time scale of one tick per nanosecond, so the number FFmpeg
  reports back is the packet's pts exactly; frames are matched to packets
  by it, so a frame the decoder drops does not shift the others.
- Decoders by codec: `h264`, `hevc`, `vp9`, `vp8`, `prores`, `libdav1d`
  (else `libaom-av1`) for AV1, and for the long-tail codecs, once allowed,
  `mpeg4`, `mpeg1video`, `mpeg2video`, `mjpeg`, `theora`, `ffv1`. A decoder
  the binary lacks makes the backend unsupported for that stream.
- `-nofind_stream_info` skips FFmpeg's probing decode, which would decode
  the first frame twice. Without it, FFmpeg's H.264 decoder learns the
  reorder depth only as frames arrive, unless the sequence parameter set
  states it, and drops a frame each time it grows the depth ("Increasing
  reorder buffer"), which a preliminary run of the frame oracle found on
  H.264 files of the corpus. `-strict 1`, for H.264 only, makes it assume the
  largest depth instead; frames then leave the decoder later, which costs
  nothing when the caller sends the end of the stream after the frame it
  wants. HEVC parameter sets always state the depth; the other codecs do
  not reorder.
- `-flags unaligned`: the decoder crops exactly as the bitstream says
  (clause 7.4.2.1.1 for H.264, the conformance window for HEVC). By
  default FFmpeg 7 keeps the planes aligned and skips a left crop: an
  H.264 stream coded 1280 wide with `frame_crop_left_offset` 24 came back
  as the uncropped 1280-wide picture, where the native backend and FFmpeg
  9 return columns 48 to 1279; an H.264 4:4:4 10-bit stream with any left
  crop failed to decode. With the flag the frames are FFmpeg 9's, and
  streams without a left crop decode byte for byte as before. The frame
  oracle (`scripts/thumbnail_oracle.py`) passes the same flag. Rejected:
  cropping in the parent from the coded size, which the worker's frames
  do not report.
- `-noautoscale`: frames keep their own size when a stream changes it;
  FFmpeg otherwise scales every frame to the first one's.
- `-copyts` keeps the timestamps as written, so that each reported pts is
  the packet's own; `-flush_packets 1` writes each frame at once.
- A `showinfo` frame number that restarts at 0 is accepted: FFmpeg
  rebuilds its filters when the frame size or format changes.
- A caller that marks a packet `discard` (`Packet::discard`: decode it for
  reference, the frame is not wanted) has its frame read from the pipe
  into a scratch buffer and dropped, not copied to shared memory and the
  parent.

Rejected: elementary streams per codec (Annex B for H.264 and HEVC, IVF
for VP8, VP9 and AV1): one format per codec, timestamps lost, and no form
at all for ProRes. Y4M output: one pixel-format list for the whole stream,
and formats it lacks would be converted by FFmpeg.

## dav1d

dav1d (BSD-2) decodes AV1 through the `dav1d` crate, the Rust binding of
the rust-av project, so CoreAV writes no FFI and `coreav-backend-dav1d`
keeps `unsafe` forbidden.
- **Build.** The crate's `dav1d` feature links the system's libdav1d
  (BSD-2), and the CLI, the worker and the decode oracle each have a
  `dav1d` feature that turns it on.
  - The minimum is libdav1d 1.3.0, the version `dav1d-sys` 0.8 asks for.
  - The build finds it with `pkg-config` (through `system-deps`;
    `PKG_CONFIG_PATH` for an install elsewhere), and links it as
    `pkg-config` describes it, a shared library on the usual
    distributions. No C source is vendored or compiled.
  - A default build has none of it, so the workspace builds without
    libdav1d.
- **FFI.** All of it is in the `dav1d` crate, rust-av's binding (MIT,
  accepted by `deny.toml`). `coreav-backend-dav1d` is the decoder-backend
  crate that uses it, and it has no `unsafe` of its own: the workspace's
  `forbid` holds, and `docs/unsafe-sites.md` says so. Should a future
  version need CoreAV-authored FFI, it belongs in this crate only, under
  the decoder-backend `unsafe` exception, with each site listed in `docs/unsafe-sites.md`. The CLI's `dav1d`
  feature turns on `coreav thumbnail` like `ffmpeg-fallback` does; with
  both, AV1 goes to dav1d (the policy's order) and the other codecs to
  the fallback.
- **Isolation.** The CLI runs dav1d inside the sandboxed worker (section
  5.4), in the worker process itself: no further process, the same
  address-space limit, deadline and descriptor checks as the fallback.
  `Dav1dBackend` decodes in the calling process, for callers that ask for
  it, and for the worker-overhead bar (bar 5).
- **Packets.** A packet is one temporal unit (the AV1 ISOBMFF and
  Matroska mappings), sent with its pts as dav1d's timestamp, which comes
  back on the picture decoded from it. `av1C`'s OBUs (a sequence header)
  go to dav1d before the first packet. A sync sample carries its own, so
  a record dav1d rejects is ignored and the samples decide. A packet
  marked `discard` is decoded and its picture dropped by timestamp.
- **Feeding.** dav1d is asked for a picture once per packet it takes.
  Asked again with no new data, dav1d waits for its oldest frame in
  flight, as when draining at the end, which empties its frame pipeline.
  An earlier version asked until dav1d had nothing, once per packet. On a
  4K 10-bit AV1 file whose target is 172 frames after its keyframe, that
  kept about 2.4 of 8 cores busy, and the thumbnail took 5.6 s against
  FFmpeg's 3.9 s. Asking once per packet takes it to 4.0 s. After the
  last packet, dav1d is asked until it has nothing, which drains it.
- **Output.** One frame per temporal unit, the highest spatial layer of
  the operating point (`all_layers` off), as FFmpeg's libdav1d wrapper
  outputs; film grain applied (dav1d's default, as FFmpeg's). Each
  picture is copied at once into packed planes, so dav1d's buffers are
  never held by a frame. Samples above 8 bits are 16 bits in host order
  in dav1d and little-endian in the frame model (swapped on a big-endian
  host). An identity matrix (RGB coded as G, B, R) stays in the Y'CbCr
  layout, and the colour description says how to convert it.
- **Limits.** `max_pixels` is dav1d's frame-size limit. Every picture is
  checked against the frame limits before its planes are copied, and the
  decoded and pending bytes are counted as in the fallback. The deadline
  is checked on every call. A packet dav1d keeps refusing to take (`Again`
  more than 4096 times in a row, frames taken between) fails with
  `Backend`.
- **Memory.** dav1d holds AV1's 8 reference frames, the frame being
  output and one frame per frame decoded in parallel (its frame delay). Its
  estimate is 64 MiB plus one decoded frame at the declared size (as the
  fallback's frame, without alpha; 6 bytes per pixel when `av1C` gives no
  format) per frame held. A stream whose references and one frame in
  flight do not fit the budget is refused with `Limit` before dav1d
  starts. Otherwise the frame delay is 8, lowered until the estimate
  fits. Without a declared size it is dav1d's own choice. As for the fallback, this
  trusts the declared size, and the address-space limit is what holds: an
  allocation dav1d is refused fails with `Limit`.
- **Threads and frame delay.** `--threads`, or dav1d's own choice (0)
  by default. On 8 threads, dav1d's own frame delay is about 3, judging
  by the times: FFmpeg's libdav1d takes 4.2 s on the file above with a
  delay of 3, 3.9 s with its default and 2.7 s with 8. A delay of 8
  takes CoreAV to 2.9 s and leaves a single-frame thumbnail of the same
  file as it was (0.10 s). The cost is memory, which the budget fits.
- **Tests.** Frames equal to FFmpeg's libdav1d, byte for byte, for 4:2:0
  at 8 and 10 bits, 4:4:4, odd sizes and film grain; discarded packets
  and the frame and decoded-bytes limits; packets of pseudo-random bytes
  (errors, never a panic). In CI the fixtures go through it (bar 1), and
  the hostile suite and the flipped-byte copies of the AV1 fixtures reach
  it through `coreav thumbnail` (bar 2). The untrusted bytes are parsed by
  dav1d, inside the worker; the crate's own code reads only the geometry
  of dav1d's pictures, checked before any copy, so it adds no fuzz target
  of its own.

Rejected:
- Writing the FFI by hand. It would add CoreAV-authored `unsafe` that the
  binding already carries and that rust-av maintains.
- Decoding AV1 only through the fallback (FFmpeg's libdav1d in a second
  process). It pays for a process, the Matroska stream and the raw-frame
  pipe on every thumbnail. S3 missed for AV1 on that path (1.186 in round
  2), and dav1d is the same decoder with less around it.
- dav1d's own frame delay. It decodes `--exact`'s run of frames about
  1.5 times slower on 8 threads (above).
- dav1d's own frame-type filter (`decode_frame_type`: reference frames
  only). `--exact` needs the target frame, which may not be a reference
  frame, so the filter cannot be used to decode up to it.

## libvpx

libvpx (BSD-3, with Google's patent grant for the WebM codecs) decodes VP9
through CoreAV's own declarations of its decoder API, in
`crates/coreav-backend-libvpx/src/ffi.rs`, the crate's only module that may
use `unsafe`.
- **Build.** The crate's `libvpx` feature links the system's libvpx,
  found with `pkg-config` (`PKG_CONFIG_PATH` for an install elsewhere) and
  linked as it describes it, a shared library on the usual distributions.
  The CLI, the worker and the decode oracle each have a `libvpx` feature
  that turns it on. No C source is vendored or compiled, and a default
  build has none of it.
  - The declarations are those of decoder ABI version 12, which libvpx
    1.14's headers declare (built and tested with 1.14, Ubuntu 24.04's). libvpx
    refuses to open a decoder for any other version, so a library with
    other layouts fails at open, and selection moves on, instead of being
    misread. The structures' sizes are asserted at compile time.
- **FFI.** Ten functions and three structures (`vpx_codec_ctx_t`,
  `vpx_codec_dec_cfg_t`, `vpx_image_t`), each site listed in
  `docs/unsafe-sites.md`. The safe API is one decoder instance, `Vp9`, and
  `Vp9::decode`, which decodes one packet and lends each image it makes
  ready to a callback as a `View`. A `View`'s rows are slices computed from
  the image's own size, format, chroma shifts and strides, and checked;
  the decoder's own code (`src/decoder.rs`) never sees a pointer. The
  CLI's `libvpx` feature turns on `coreav thumbnail` and `coreav
  transcode` like `ffmpeg-fallback` does; with both, VP9 goes to libvpx
  (the policy's order) and the other codecs to the fallback.
- **Frame sizes before libvpx.** Untrusted bytes are parsed in safe Rust
  before a C library sees them. `src/header.rs` reads the uncompressed
  header of every frame of a packet, the frames of a superframe found
  through its index (the specification's Annex B), for the size it states
  (`frame_size()`). The decoder checks every stated size against
  `max_width`, `max_height` and `max_pixels` before the packet goes to
  libvpx, so libvpx never allocates a frame over them. A frame that
  states no size (one shown again, or one taking a reference frame's
  size) has a size an earlier frame stated and that was checked. A packet
  whose headers cannot be read (a frame marker or sync code that is
  wrong, a header cut short, an index larger or smaller than its frames)
  fails with `InvalidData`. The reader has a fuzz target (bar 4).
  - Every packet reaches libvpx with an index. The specification makes a
    packet without one a single frame, but libvpx reads such a packet as
    frames one after another until its bytes end, so a second frame
    there would reach libvpx unread. The decoder appends an index of one
    frame to such a packet (`indexed`), and libvpx then decodes the
    frames the index lists and only those: a test sends two key frames
    in one packet, the second over the limits, and gets the first.
  - Frames of a packet marked `discard` are decoded and not copied; their
    sizes are checked against the limits all the same.
- **Isolation.** The CLI runs libvpx inside the sandboxed worker, in the
  worker process itself, as dav1d (section 5.4). `LibvpxBackend` decodes
  in the calling process, for callers that ask for it.
- **Packets and output.** A packet is one frame or superframe (the VP9
  ISOBMFF and Matroska mappings). libvpx decodes it at once and makes
  ready the frame it shows, if any, which takes the packet's pts: VP9
  shows frames in decode order, so no timestamp goes through libvpx. A
  packet marked `discard` is decoded and its frame dropped. Formats:
  4:2:0, 4:2:2 and 4:4:4 at 8, 10 and 12 bits, profiles 0 to 3. Samples
  above 8 bits are 16 bits in host order in libvpx and little-endian in
  the frame model. Refused with `Unsupported`: 4:4:0 (profile 1 or 3), for
  which the frame model has no format, when its first frame decodes; and,
  at open, a stream whose configuration record says more than 8 bits when
  libvpx was built without high bit depth (`vpx_codec_get_caps`), so that
  selection moves to the fallback.
- **Limits.** Besides the stated sizes, every image is checked against
  the frame limits before its planes are copied, and the decoded and
  pending bytes are counted as in the fallback. The deadline is checked
  on every call.
- **Memory.** The estimate is 64 MiB plus 10 decoded frames at the
  declared size (VP9's 8 reference frames, the frame being decoded and
  the frame being output), with the same frame bytes as dav1d's estimate.
  A stream over the budget is refused with `Limit` at open. As for dav1d,
  this trusts the declared size; the stated sizes and, in the worker, the
  address-space limit are what hold.
- **Threads.** `--threads`, or one per CPU (0) by default, with libvpx's
  row-based multithreading on when there are several. This libvpx's VP9
  decoder offers no frame threading (`vpx_codec_get_caps` returns
  `0x440005`, without `VPX_CODEC_CAP_FRAME_THREADING`), so its threads work
  within one frame: on its tiles, and with row multithreading on its rows.
- **Tests.** Frames equal to FFmpeg's own VP9 decoder, byte for byte, for
  4:2:0 at 8, 10 and 12 bits, 4:2:2, 4:4:4 at 8 and 10 bits, an odd size,
  hidden alternate reference frames in superframes, and tiles; the sizes
  the reader states against the decoded ones; discarded packets and the
  frame, decoded-bytes and memory limits; a key frame header over the
  limits refused before libvpx; packets of pseudo-random bytes (errors,
  never a panic). In CI the fixtures go through it (bar 1), and the
  hostile suite and the flipped-byte copies of the VP9 fixtures reach it
  through `coreav thumbnail` and `coreav transcode` (bar 2).

Rejected:
- A Rust binding, as for dav1d. None carries the calls for CoreAV, which
  was dav1d's reason: `vpx-rs` (BSD-2) depends on `env-libvpx-sys`, whose
  MPL-2.0 is outside the licence policy (`deny.toml`); `vpx-sys` (MIT,
  2021) and `media-codec-vpx-sys` (MIT or Apache-2.0) are raw declarations
  that bindgen generates at build time, which needs libclang in every
  build, and CoreAV would still write the same `unsafe` calls. Declaring the
  ten functions by hand adds no `unsafe` beyond those calls, and
  libvpx's ABI check guards the layouts.
- Decoding VP9 only through the fallback (FFmpeg's VP9 decoder in a
  second process). It pays for a process, the Matroska stream and the
  raw-frame pipe on every thumbnail and every transcode, and runs FFmpeg's
  C code.
- libvpx's frame-based threading. Its VP9 decoder does not offer it
  (above).

## NVDEC

Not in this change; it comes next, behind the same trait, and the bars
below apply to it when it does. NVDEC decodes H.264 and HEVC on GPU
hosts, in-process: CoreAV parses the parameter sets and
slice headers in safe Rust and fills NVDEC's picture parameters itself,
with no CUVID parser and no FFmpeg on the path (spike B, H2).
The production code is written fresh, with the divergence
detection required before release.

## The decode oracle

`tools/decode-oracle` (test-only, never shipped) decodes the first packets
of a file's video stream through the decode boundary, exactly as
`coreav thumbnail` hands them to a decoder, and prints each frame's pts and
the MD5 of its planes packed without padding, `framemd5`'s layout.
`scripts/thumbnail_oracle.py` compares that with FFmpeg's `framemd5` of the
same stream (`-noautorotate`, since rotation is a filter, not decoding;
`-noautoscale`; and for a format with alpha the same format without it,
since the frame model drops alpha), for the fixtures in CI and for the
corpus in `docs/spikes/thumbnail-bench/`.

## Bars

Fixed before any measured run, and judged on the commit that adds the
measurement.

1. **Frames agree with FFmpeg `framemd5`.** For each stream: the first
   120 packets in decode order go through CoreAV's decoder; the frames it
   returns whose pts is below the smallest pts of the packets not sent form
   a complete presentation-order prefix; their MD5s (planes packed without
   padding, `framemd5`'s layout) must equal, in order, the first as many
   frames of `ffmpeg -ignore_editlist 1 -i FILE -map 0:v:0 -fps_mode
   passthrough -f framemd5` (`-ignore_editlist` for MP4/MOV only) decoding
   the same file with the same decoder. A stream CoreAV fails to decode
   counts as disagreeing in full.
   - Fixtures (every video fixture of `scripts/gen-fixtures.sh`,
     `scripts/gen-fixtures-mkv.sh` and the decode fixtures): 100% of
     frames, every backend built.
   - Corpus v1 `real` files whose first video stream's codec is on the
     default allowlist: at least **99% of streams** agree on every compared
     frame and at least **99.9% of compared frames** agree, per backend
     (fallback: every codec; dav1d: AV1; libvpx: VP9). Disagreements are investigated;
     intentional ones are recorded as known divergences.
   - How the two sides are made to hash the same frames, set after the
     preliminary runs and before the final one (no threshold changed):
     FFmpeg runs with `-noautorotate` (rotation is a filter, not decoding)
     and `-noautoscale` (frames keep their size when the stream changes
     it); for a format with alpha, with the same format without alpha
     (the frame model drops alpha); on the stream CoreAV decodes (FFmpeg's
     default: the highest resolution, the first on a tie); with
     `-c:v libdav1d` for the dav1d backend; with `-c:v vp9`, FFmpeg's own
     VP9 decoder and never its libvpx wrapper, for the libvpx backend. A stream CoreAV fails to
     decode, or gets no frame from, counts FFmpeg's first 120 frames of
     it as compared and unequal; a run that hangs, crashes or runs out of
     memory counts as a disagreeing stream.
   - Counting for the measurements after the first. It was set after the
     first run's failures were known, so each later run reports its
     result both ways: the verdict uses the counting above, and this one
     is reported beside it. The first run keeps the counting above.
     - A file CoreAV refuses by design is counted apart from both
       denominators and reported file by file, naming the
       container-structure rule of the parser it breaks. Such
       a refusal is a `coreav.error/1` of kind `truncated_structure` or
       `invalid_structure`. Any other failure, a decode limit included,
       or a refusal that names no rule, still counts as disagreeing in
       full.
     - A run the measuring host could not finish (its memory, disk or
       time, shown by the host's own logs, or by FFmpeg failing alone) is
       re-run on a larger host before the result is final. A run that
       still cannot finish counts as disagreeing.
   - NVDEC: the same, for the H.264 and HEVC streams it supports, with
     the rule for the accepted NVDEC divergence.
2. **Hostile input.** Every file of the hostile-input suite
   (`tools/hostile/generate.py`) through `coreav thumbnail`,
   and the decode fuzz cases below through each backend: exit 0, or 1 with
   a `coreav.error/1` object; no crash, signal, hang past the timeout or
   out-of-memory, under 1 GiB of address space for the parent.
3. **Hostile worker.** A test worker that sends malformed messages,
   descriptors that overflow, overlap or leave the slot, wrong sequence
   numbers, unknown formats, oversize geometry, garbage after `Done`,
   that exits, hangs or floods: every case ends with an error, never a
   panic, a fault or a hang past the deadline.
4. **Fuzz targets**: descriptor validation and plane copies
   (`coreav-worker/fuzz`: `descriptor`, `messages`); `showinfo` and raw
   frame parsing (`coreav-backend-ffmpeg/fuzz`); the configuration-record
   readers (`coreav-decode/fuzz`); the VP9 frame-size reader in front of
   libvpx (`coreav-backend-libvpx/fuzz`: `frame_sizes`); every frame operation
   (`coreav-frame/fuzz`).
5. **Speed.** The decode boundary is measured through its user-facing
   operation, thumbnails: the bars are in `docs/design/thumbnails.md`.
   The worker's own overhead bar: under 15% of
   thumbnail time against the same backend in-process, measured with dav1d,
   the backend that can run both ways.
   - How it is measured, for dav1d. On corpus v1's development-set AV1
     files, `decode-oracle thumbnail FILE --at T` (the thumbnail library,
     as `coreav thumbnail` calls it) is timed as a process with dav1d in
     the worker and with dav1d in-process (`--in-process`). T and the
     rounds are as in S1, keyframe mode, and the two runs alternate. The
     overhead of a file is its in-worker time over its in-process time,
     minus 1, and the bar is a median overhead under 0.15. The bar is
     about the worker, not about FFmpeg, so it uses the development set
     and leaves the held-out files for speed claims.
   - The thumbnail speed bars for AV1 with dav1d (S1 to S5 of
     `docs/design/thumbnails.md`). The held-out AV1 files were timed in
     both earlier rounds through the fallback. Timing them again is
     therefore a development measurement, reported with its verdicts and
     beside the development set's, with no claim of being faster than
     FFmpeg. That claim waits for a fresh held-out draw.
   - Results: `docs/spikes/thumbnail-bench/`, "AV1 through dav1d".
   - For VP9 with libvpx (`docs/measurements/vp9-libvpx.md`), fixed
     before its runs. One run each, alone on its VM, on a Compute Engine
     `c4-standard-8` with simultaneous multithreading off (4 cores, one
     thread each; `scripts/gcp_vm_job.py`); a run is repeated only for a
     host fault (a void gate, a preempted or failed VM).
     - Frames (bar 1 for the libvpx backend): every corpus v1 `real` file
       whose first video stream is VP9, through `decode-oracle --backend
       libvpx` against `framemd5` with `-c:v vp9`: at least 99% of streams
       and 99.9% of compared frames agree, counted as above.
     - Thumbnails: S1 to S5 of `docs/design/thumbnails.md` for the VP9
       class, `coreav thumbnail` built with `libvpx` and `ffmpeg-fallback`
       (VP9 through libvpx) against the `ffmpeg` command line, whose VP9
       decoder is FFmpeg's own (`vp9 (native)`, recorded per run), by
       `scripts/vp9_speed.py`: one warm-up and 5 rounds in rotated order, as
       `docs/spikes/thumbnail-bench/` times them. Bars (Decision 34): the
       median per-file ratio is at most 1.00 for each of S1 to S5. The
       files are both partitions of corpus v1's VP9 files. Earlier rounds
       timed the held-out ones through the fallback, so this is a
       development measurement: it decides, and backs no claim of being
       faster than FFmpeg. Reported beside, with no bar: the same
       commands built with `ffmpeg-fallback` alone (VP9 through FFmpeg's
       decoder in the worker), each partition apart, and the split by
       FFmpeg's own time.
     - Transcode: B5 of `docs/design/transcode.md` for the VP9 class
       (its bar, 1.05, each arm), on the VP9 files of its speed selection,
       built with `native-h264`, `dav1d`, `libvpx` and `ffmpeg-fallback`,
       by `scripts/transcode_speed_gcp.py` on the host above. Reported
       beside: the same medians against 1.00.
     - Decoder alone, no bar: per file, FFmpeg decoding the stream with
       `-c:v libvpx-vp9` (FFmpeg's libvpx wrapper) over the same with `-c:v
       vp9`, both `-f null`, which compares the two decoders with nothing
       else around them.
     - The read on a native VP9 decoder, fixed with the bars. If the
       libvpx build meets S1 to S5 and B5, no native VP9 decoder is needed
       for these bars. If it misses S3 (decoding from the keyframe to the
       frame asked for) or B5, and the decoder-alone median is above 1.10,
       libvpx itself is the limit, and reaching the bars needs another
       decoder: a native one, or FFmpeg's in the fallback. If it misses
       with the decoder-alone median at or below 1.10, the cost is around
       the decoder, and a native decoder is not the first change to make.
     - Results (`docs/measurements/vp9-libvpx.md`): bar 1 met (74 of 74
       streams, 8,408 of 8,408 frames); S1 to S5 met (S3 0.994, 0.895 to
       1.053, 51% of files below 1.00); B5 missed on both arms (1.075 and
       1.074). The decoder-alone median is 1.036, so by the rule above
       these bars need no native VP9 decoder, and it is not the first
       change for B5.
     S3 for AV1 is 0.927 on the development set and 0.934 on the held-out
     AV1 files. Only 60% and 62% of those files are faster than FFmpeg,
     short of the 80% of files the project aims for beside the median
     bar (an aim, not a bar).
   - Where the AV1 gap is. Split by FFmpeg's own time for S3, over all 88
     files:

     | FFmpeg's time | Files | Median ratio | Files below 1.00 |
     |---|---:|---:|---:|
     | under 50 ms | 27 | 1.609 | 19% |
     | 50 to 300 ms | 34 | 0.843 | 76% |
     | over 300 ms | 27 | 0.762 | 81% |

     Long decodes are faster since the polling fix and the frame delay of
     8. The short ones lose about 8 ms, a fixed cost per thumbnail. On a
     224×224 file whose target is 24 minutes in, CoreAV in-process takes
     25 ms against FFmpeg's 18 ms, and executes 106M instructions:
     - 39% planning (`coreav_thumbnail::mp4::plan`), which here covers
       a 24-minute sample table to find one keyframe and the target;
     - 24% conversion to RGB and the resize;
     - 17% the JPEG encoder;
     - about 20% dav1d.

     The worker adds 2 to 3 ms to that (bar 5). What is left for AV1
     speed, roughly in order of what it would gain:
     1. cheaper planning on long files (done for MP4 and MOV:
        `docs/design/seek.md`);
     2. faster conversion and resizing (`coreav-frame`) and JPEG encoding
        (conversion and JPEG done: `docs/design/thumbnails.md`, "Faster
        rendering and JPEG"; resampling remains);
     3. starting the worker sooner, or keeping one for several
        thumbnails;
     4. dav1d's threads: its own count, one per CPU, is kept, since tile
        and frame threads are dav1d's own; it was not tuned beyond the
        frame delay.

     Items 1 and 2 are not specific to AV1 and would shorten every
     codec's short thumbnails.
   - Measured again after items 1 and 2, on `main` at `55d126b`, with
     bars committed before the run
     (`docs/measurements/av1-thumbnail-fixed-cost.md`). The fixed cost is
     gone: the files under 50 ms are at 0.673 for S3 (97% faster, a median
     5.3 ms less than FFmpeg), 50 to 300 ms at 0.827 and over 300 ms at
     0.744. Item 3 would now gain about 1 ms on a short thumbnail, the
     worker's share there. Item 4 gains about 1 ms in keyframe mode with 2
     or 4 threads and costs far more with `--exact`, so the default stays.
     The worker's overhead was 0.156 on that run, over this bar's 0.15:
     it grows with the frame (18.5 ms above 2.5 megapixels), so it is the
     frame's copy, not the worker's start.
